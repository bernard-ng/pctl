// Compile the assigned module without requiring changes to the library's module registry.
pub use pctl::{Result, model};
#[path = "../src/generation.rs"]
mod generation;

use generation::{generate, render};
use model::{Manifest, ValueType, Variable, Visibility};
use serde_json::{Value, json};
use std::fs;

fn manifest() -> Manifest {
    let mut manifest = Manifest::default();
    for (name, kind, visibility, exposed) in [
        ("TOKEN", ValueType::String, Visibility::Secret, false),
        ("COUNT", ValueType::Integer, Visibility::Internal, false),
        ("ENABLED", ValueType::Boolean, Visibility::Public, true),
        ("MODE", ValueType::Enum, Visibility::Public, true),
        ("SERVER", ValueType::String, Visibility::Public, false),
        ("URL", ValueType::Url, Visibility::Public, true),
    ] {
        manifest.variables.insert(
            name.into(),
            Variable {
                kind,
                visibility,
                browser_exposed: exposed,
                consumers: vec!["api".into()],
                required_in: vec!["production".into()],
                values: if name == "MODE" {
                    vec!["b".into(), "a".into()]
                } else {
                    vec![]
                },
            },
        );
    }
    manifest
}

#[test]
fn schemas_validate_strings_and_allowlist_browser_variables() {
    let output = render(&manifest(), "production").unwrap();
    let schema: Value = serde_json::from_str(&output["environment.schema.json"]).unwrap();
    let public: Value = serde_json::from_str(&output["public-runtime.schema.json"]).unwrap();
    for property in schema["properties"].as_object().unwrap().values() {
        assert_eq!(property["type"], "string");
    }
    assert_eq!(
        schema["properties"]["ENABLED"]["enum"],
        json!(["true", "false"])
    );
    assert_eq!(
        schema["properties"]["COUNT"]["pattern"],
        "^(0|-?[1-9][0-9]*)(?![\\s\\S])"
    );
    assert_eq!(schema["properties"]["MODE"]["enum"], json!(["a", "b"]));
    assert!(schema.get("additionalProperties").is_none());
    assert_eq!(public["additionalProperties"], false);
    assert_eq!(schema["properties"]["URL"]["format"], "uri");
    assert_eq!(public["properties"]["URL"], schema["properties"]["URL"]);
    assert_eq!(public["properties"].as_object().unwrap().len(), 3);
    assert_eq!(public["required"], json!(["ENABLED", "MODE", "URL"]));
    assert_eq!(
        schema["required"],
        json!(["COUNT", "ENABLED", "MODE", "SERVER", "TOKEN", "URL"])
    );
    let local = render(&manifest(), "local").unwrap();
    for name in ["environment.schema.json", "public-runtime.schema.json"] {
        let schema: Value = serde_json::from_str(&local[name]).unwrap();
        assert_eq!(schema["required"], json!([]));
    }
}

#[test]
fn examples_never_supply_values_and_secrets_are_not_published() {
    let mut manifest = manifest();
    manifest.variables.get_mut("TOKEN").unwrap().values = vec!["sensitive-sentinel".into()];
    let output = render(&manifest, "production").unwrap();
    assert_eq!(output.len(), 4);
    for text in output.values() {
        assert!(!text.contains("sensitive-sentinel"));
    }
    assert!(
        output[".env.example"]
            .lines()
            .all(|line| line.is_empty() || line.starts_with('#'))
    );
    assert!(output[".env.example"].contains("# TOKEN=\n"));
}

#[test]
fn rejects_browser_secrets_and_injected_variable_names() {
    let mut manifest = manifest();
    manifest.variables.get_mut("TOKEN").unwrap().browser_exposed = true;
    assert!(render(&manifest, "local").is_err());
    manifest.variables.get_mut("TOKEN").unwrap().browser_exposed = false;
    let variable = manifest.variables.remove("TOKEN").unwrap();
    manifest
        .variables
        .insert("TOKEN\nACTIVE=yes".into(), variable);
    assert!(render(&manifest, "local").is_err());
}

#[test]
fn deterministic_generation_and_exact_read_only_drift_checks() {
    let manifest = manifest();
    assert_eq!(
        render(&manifest, "production"),
        render(&manifest, "production")
    );
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().canonicalize().unwrap().join("generated");
    assert!(generate(&manifest, "production", &output, true).is_err());
    assert!(!output.exists());
    generate(&manifest, "production", &output, false).unwrap();
    fs::write(output.join("keep.txt"), "preserve").unwrap();
    generate(&manifest, "production", &output, true).unwrap();
    for (name, expected) in render(&manifest, "production").unwrap() {
        assert_eq!(fs::read(output.join(&name)).unwrap(), expected.as_bytes());
        fs::write(output.join(&name), expected + "\n").unwrap();
        assert!(
            generate(&manifest, "production", &output, true)
                .unwrap_err()
                .contains(&name)
        );
        generate(&manifest, "production", &output, false).unwrap();
        fs::remove_file(output.join(&name)).unwrap();
        assert!(
            generate(&manifest, "production", &output, true)
                .unwrap_err()
                .contains(&name)
        );
        assert!(!output.join(&name).exists());
        generate(&manifest, "production", &output, false).unwrap();
    }
    assert_eq!(
        fs::read_to_string(output.join("keep.txt")).unwrap(),
        "preserve"
    );
    assert_eq!(fs::read_dir(&output).unwrap().count(), 5);
    assert!(generate(&manifest, "local", &output, true).is_err());
}

#[cfg(unix)]
#[test]
fn refuses_symlink_directories_and_artifacts_before_writing() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let output = root.join("generated");
    fs::create_dir(&output).unwrap();
    let target = root.join("private");
    fs::write(&target, "untouched").unwrap();
    symlink(&target, output.join("environment.md")).unwrap();
    for check in [true, false] {
        assert!(generate(&manifest(), "local", &output, check).is_err());
    }
    assert!(!output.join(".env.example").exists());
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
    let linked = root.join("linked");
    symlink(&output, &linked).unwrap();
    assert!(generate(&manifest(), "local", &linked, false).is_err());
    assert!(generate(&manifest(), "local", &linked.join("child"), false).is_err());
    fs::remove_file(output.join("environment.md")).unwrap();
    symlink(root.join("missing"), output.join("environment.md")).unwrap();
    assert!(generate(&manifest(), "local", &output, false).is_err());
}
