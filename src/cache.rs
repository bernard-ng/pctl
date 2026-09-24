//! Content fingerprints and verified output receipts. Never cache failures.
use crate::{Result, model::PlannedTask, paths};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn hash_path(root: &Path, relative: &Path, hasher: &mut Sha256) -> Result<()> {
    let path = paths::inside(root, relative)?;
    let metadata = path.metadata().map_err(|_| {
        crate::error::Error::from(format!("Missing cache path: {}", relative.display()))
    })?;
    let name = relative.to_string_lossy();
    hasher.update((name.len() as u64).to_le_bytes());
    hasher.update(name.as_bytes());

    if metadata.is_file() {
        hasher.update(b"file");
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    } else if metadata.is_dir() {
        hasher.update(b"dir");
        let mut entries = std::fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        entries.sort();
        for entry in entries {
            hash_path(root, &relative.join(entry), hasher)?;
        }
    } else {
        return Err("Cache inputs must be regular files or directories".into());
    }

    Ok(())
}

pub fn fingerprint(
    root: &Path,
    task: &PlannedTask,
    profile: &str,
    environment: &BTreeMap<String, String>,
    tools: &BTreeMap<String, String>,
    dependencies: &[String],
) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(
        serde_json::to_vec(&(
            env!("CARGO_PKG_VERSION"),
            task,
            profile,
            environment,
            tools,
            dependencies,
        ))
        .map_err(|e| e.to_string())?,
    );

    for input in &task.cache.as_ref().ok_or("No cache declaration")?.inputs {
        hash_path(root, Path::new(input), &mut hasher)?;
    }
    Ok(hex::encode(hasher.finalize()))
}

fn outputs(root: &Path, task: &PlannedTask) -> Result<String> {
    let mut hasher = Sha256::new();
    for output in &task.cache.as_ref().ok_or("No cache declaration")?.outputs {
        hash_path(root, Path::new(output), &mut hasher)?;
    }
    Ok(hex::encode(hasher.finalize()))
}

fn receipt(root: &Path, task: &PlannedTask) -> Result<PathBuf> {
    Ok(paths::state_dir(root, "cache")?.join(format!("{}.json", hash(task.id.as_bytes()))))
}

pub fn hit(root: &Path, task: &PlannedTask, fingerprint: &str) -> Result<bool> {
    let path = receipt(root, task)?;
    let Ok(bytes) = std::fs::read(path) else {
        return Ok(false);
    };
    let Ok(saved) = serde_json::from_slice::<(String, String)>(&bytes) else {
        return Ok(false);
    };
    Ok(saved.0 == fingerprint && outputs(root, task).is_ok_and(|current| current == saved.1))
}

pub fn save(root: &Path, task: &PlannedTask, fingerprint: &str) -> Result<()> {
    let output = outputs(root, task)?;
    let path = receipt(root, task)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().unwrap()).map_err(|e| e.to_string())?;
    temporary
        .write_all(&serde_json::to_vec(&(fingerprint, output)).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    temporary.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
