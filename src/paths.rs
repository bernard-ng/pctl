//! Keep generated state and declared files within a canonical project root.
use crate::{Result, error::Error};
use std::path::{Component, Path, PathBuf};

pub fn inside(root: &Path, path: &Path) -> Result<PathBuf> {
    let root = root
        .canonicalize()
        .map_err(Error::io("Cannot resolve", root))?;
    inside_canonical(&root, path)
}

/// Like [`inside`], for a root that is already canonical. Hot paths that check
/// many entries canonicalize once and call this to avoid a syscall per check.
pub(crate) fn inside_canonical(root: &Path, path: &Path) -> Result<PathBuf> {
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err("Path must be relative and cannot contain parent traversal".into());
    }
    let mut current = root.to_path_buf();
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

/// Location of a named state directory without creating it.
pub fn state_path(root: &Path, name: &str) -> Result<PathBuf> {
    inside(root, &PathBuf::from(".pctl").join(name))
}

pub fn state_dir(root: &Path, name: &str) -> Result<PathBuf> {
    let path = state_path(root, name)?;
    std::fs::create_dir_all(&path).map_err(Error::io("Cannot create", &path))?;
    Ok(path)
}
