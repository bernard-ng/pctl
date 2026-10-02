//! Content fingerprints and verified output receipts. Never cache failures.
use crate::{Result, error::Error, model::PlannedTask, paths};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
};

pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// Hash one declared path. The declared path itself must not traverse symlinks
/// (it is the project boundary), but entries found *inside* a declared directory
/// are hashed as they are: regular files by content, nested symlinks by their
/// target text (never followed), so trees such as `node_modules` are cacheable.
/// `root` must already be canonical.
fn hash_path(root: &Path, relative: &Path, hasher: &mut Sha256) -> Result<()> {
    let path = paths::inside_canonical(root, relative)?;
    match path.symlink_metadata() {
        Ok(_) => hash_entry(&path, &relative.to_string_lossy(), hasher),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("Missing cache path: {}", relative.display()).into())
        }
        Err(error) => Err(Error::io("Cannot inspect", &path)(error)),
    }
}

fn hash_entry(path: &Path, label: &str, hasher: &mut Sha256) -> Result<()> {
    hasher.update((label.len() as u64).to_le_bytes());
    hasher.update(label.as_bytes());
    let metadata = path
        .symlink_metadata()
        .map_err(Error::io("Cannot inspect", path))?;
    let kind = metadata.file_type();

    if kind.is_symlink() {
        hasher.update(b"link");
        let target = std::fs::read_link(path).map_err(Error::io("Cannot read link", path))?;
        let target = target.to_string_lossy();
        hasher.update((target.len() as u64).to_le_bytes());
        hasher.update(target.as_bytes());
    } else if kind.is_file() {
        hasher.update(b"file");
        let mut file = std::fs::File::open(path).map_err(Error::io("Cannot read", path))?;
        // Stream rather than buffer; the count taken from the stream frames the
        // content even if the file changes size while being read.
        let mut buffer = vec![0; 64 * 1024];
        let mut length = 0u64;
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    hasher.update(&buffer[..count]);
                    length += count as u64;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(Error::io("Cannot read", path)(error)),
            }
        }
        hasher.update(length.to_le_bytes());
    } else if kind.is_dir() {
        hasher.update(b"dir");
        let mut entries = std::fs::read_dir(path)
            .map_err(Error::io("Cannot list", path))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(Error::io("Cannot list", path))?;
        entries.sort();
        for entry in entries {
            let child = format!("{label}/{}", entry.to_string_lossy());
            hash_entry(&path.join(&entry), &child, hasher)?;
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
    hasher.update(serde_json::to_vec(&(
        env!("CARGO_PKG_VERSION"),
        task,
        profile,
        environment,
        tools,
        dependencies,
    ))?);

    let root = root
        .canonicalize()
        .map_err(Error::io("Cannot resolve", root))?;
    for input in &task.cache.as_ref().ok_or("No cache declaration")?.inputs {
        hash_path(&root, Path::new(input), &mut hasher)?;
    }
    Ok(hex::encode(hasher.finalize()))
}

fn outputs(root: &Path, task: &PlannedTask) -> Result<String> {
    let mut hasher = Sha256::new();
    let root = root
        .canonicalize()
        .map_err(Error::io("Cannot resolve", root))?;
    for output in &task.cache.as_ref().ok_or("No cache declaration")?.outputs {
        hash_path(&root, Path::new(output), &mut hasher)?;
    }
    Ok(hex::encode(hasher.finalize()))
}

fn receipt_name(task: &PlannedTask) -> String {
    format!("{}.json", hash(task.id.as_bytes()))
}

pub fn hit(root: &Path, task: &PlannedTask, fingerprint: &str) -> Result<bool> {
    let path = paths::state_path(root, "cache")?.join(receipt_name(task));
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
    let directory = paths::state_dir(root, "cache")?;
    let path = directory.join(receipt_name(task));
    let mut temporary =
        tempfile::NamedTempFile::new_in(&directory).map_err(Error::io("Cannot stage", &path))?;
    temporary
        .write_all(&serde_json::to_vec(&(fingerprint, output))?)
        .map_err(Error::io("Cannot write", &path))?;
    temporary
        .persist(&path)
        .map_err(|e| Error::io("Cannot replace", &path)(e.error))?;
    Ok(())
}
