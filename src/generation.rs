//! Deterministic environment contracts and filesystem drift checks.
use crate::Result;
use crate::model::{Manifest, ValueType, Visibility};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Render declarations only; process environment and task overrides are never read.
pub fn render(manifest: &Manifest, profile: &str) -> Result<BTreeMap<String, String>> {
    let mut properties = BTreeMap::new();
    let mut public = BTreeMap::new();
    let mut required = Vec::new();
    let mut public_required = Vec::new();
    let mut example =
        String::from("# Set values in your environment. No defaults are supplied here.\n");
    let mut documentation = String::from(
        "# Environment contract\n\nAll environment values are strings. Required status applies to the selected profile.\n\n| Variable | Type | Visibility | Required | Browser | Consumers |\n| --- | --- | --- | --- | --- | --- |\n",
    );

    for (name, variable) in &manifest.variables {
        if !valid_name(name) {
            return Err("Environment variable names must match [A-Za-z_][A-Za-z0-9_]*".into());
        }
        if variable.browser_exposed && variable.visibility != Visibility::Public {
            return Err(format!("{name}: browser exposure requires public visibility").into());
        }
        let mut schema = json!({ "type": "string" });
        let kind = match variable.kind {
            ValueType::String => "string",
            ValueType::Url => {
                schema["format"] = json!("uri");
                "url"
            }
            ValueType::Boolean => {
                schema["enum"] = json!(["true", "false"]);
                "boolean"
            }
            ValueType::Integer => {
                // JSON Schema uses ECMAScript regexes; `$` alone permits a final newline.
                schema["pattern"] = json!("^(0|-?[1-9][0-9]*)(?![\\s\\S])");
                "integer"
            }
            ValueType::Enum => {
                if variable.values.is_empty() {
                    return Err(format!("{name}: enum requires declared values").into());
                }
                let mut values = variable.values.clone();
                values.sort();
                values.dedup();
                schema["enum"] = json!(values);
                "enum"
            }
        };
        let is_required = variable.required_in.iter().any(|value| value == profile);
        if is_required {
            required.push(name);
        }
        if variable.browser_exposed {
            public.insert(name, schema.clone());
            if is_required {
                public_required.push(name);
            }
        }
        properties.insert(name, schema);
        let visibility = match variable.visibility {
            Visibility::Public => "public",
            Visibility::Internal => "internal",
            Visibility::Secret => "secret",
        };
        let required_label = if is_required { "yes" } else { "no" };
        example.push_str(&format!(
            "\n# {name}: {kind}; {visibility}; required: {required_label}\n# {name}=\n"
        ));
        let mut consumers = variable.consumers.clone();
        consumers.sort();
        consumers.dedup();
        documentation.push_str(&format!(
            "| {name} | {kind} | {visibility} | {required_label} | {} | {} |\n",
            if variable.browser_exposed {
                "yes"
            } else {
                "no"
            },
            markdown_cell(&consumers.join(", ")),
        ));
    }

    let environment = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object", "properties": properties, "required": required,
    });
    let runtime = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object", "properties": public, "required": public_required,
        "additionalProperties": false,
    });
    Ok(BTreeMap::from([
        ("environment.schema.json".into(), pretty_json(&environment)?),
        ("public-runtime.schema.json".into(), pretty_json(&runtime)?),
        (".env.example".into(), example),
        ("environment.md".into(), documentation),
    ]))
}

fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn markdown_cell(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('|', "&#124;")
        .replace(['\r', '\n'], " ")
}

fn pretty_json(value: &Value) -> Result<String> {
    Ok(serde_json::to_string_pretty(value)
        .map(|text| text + "\n")
        .map_err(|error| format!("Cannot render environment schema: {error}"))?)
}

/// Check exact bytes or replace each generated file atomically. Other files are preserved.
/// Callers must prevent concurrent changes to the output directory and its ancestors.
pub fn generate(manifest: &Manifest, profile: &str, output: &Path, check: bool) -> Result<()> {
    let artifacts = render(manifest, profile)?;
    inspect_directory(output)?;
    for name in artifacts.keys() {
        inspect_file(&output.join(name))?;
    }
    if check {
        for (name, expected) in artifacts {
            let path = output.join(&name);
            let actual = fs::read(&path).map_err(|error| {
                format!("Generated artifact missing or unreadable: {name}: {error}")
            })?;
            if actual != expected.as_bytes() {
                return Err(format!("Generated artifact drift: {name}").into());
            }
        }
        return Ok(());
    }
    fs::create_dir_all(output)
        .map_err(|error| format!("Cannot create output directory: {error}"))?;
    for (name, content) in artifacts {
        atomic_write(&output.join(name), content.as_bytes())?;
    }
    Ok(())
}

fn inspect_directory(output: &Path) -> Result<()> {
    if output.as_os_str().is_empty() {
        return Err("Output directory cannot be empty".into());
    }
    let mut path = PathBuf::new();
    for component in output.components() {
        if component == Component::ParentDir {
            return Err("Output directory cannot contain parent traversal".into());
        }
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(format!("Unsafe output directory: {}", path.display()).into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot inspect output directory: {error}").into()),
        }
    }
    Ok(())
}

fn inspect_file(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            Err(format!("Unsafe generated artifact: {}", path.display()).into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot inspect generated artifact: {error}").into()),
    }
}

static TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

struct TemporaryFile(PathBuf);

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let (temporary, mut file) = loop {
        let id = TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let temporary = path.with_file_name(format!(".pctl-{}-{id}.tmp", std::process::id()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (TemporaryFile(temporary), file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Cannot stage generated artifact: {error}").into()),
        }
    };
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Cannot write generated artifact: {error}"))?;
    drop(file);
    inspect_file(path)?;
    Ok(fs::rename(&temporary.0, path)
        .map_err(|error| format!("Cannot replace generated artifact: {error}"))?)
}
