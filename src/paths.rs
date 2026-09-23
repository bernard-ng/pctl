//! Keep generated state and declared files within a canonical project root.
use crate::Result;
use std::path::{Component, Path, PathBuf};

pub fn inside(root: &Path, path: &Path) -> Result<PathBuf> {
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err("Path must be relative and cannot contain parent traversal".into());
    }
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let mut current = root.clone();
    for component in path.components() {
        current.push(component);
        if let Ok(metadata) = current.symlink_metadata()
            && metadata.file_type().is_symlink()
        {
            return Err("Managed paths cannot contain symlinks".into());
        }
    }
    Ok(current)
}

pub fn state_dir(root: &Path, name: &str) -> Result<PathBuf> {
    let path = inside(root, &PathBuf::from(".pctl").join(name))?;
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    Ok(path)
}
