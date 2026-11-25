use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::error::ReadError;
use crate::format;
use crate::{FileState, Snapshot, Target, TargetSnapshot};

/// Resolves, reads and parses every target and validates its schema and declaration.
///
/// # Errors
///
/// Fails when a target cannot be read or parsed, two targets resolve to one real file,
/// or a schema or declaration is invalid.
pub fn read(targets: Vec<Target>) -> Result<Snapshot, ReadError> {
    let mut snapshots: Vec<TargetSnapshot> = Vec::with_capacity(targets.len());
    let mut identities = Vec::with_capacity(targets.len());
    for target in targets {
        target
            .schema
            .validate()
            .map_err(|source| ReadError::Schema {
                file: target.file.clone(),
                source,
            })?;
        target
            .declaration
            .validate(&target.schema, target.format)
            .map_err(|source| ReadError::Declaration {
                file: target.file.clone(),
                source,
            })?;
        let (real, file, identity) = load(&target)?;
        if let Some((earlier, _)) = snapshots.iter().zip(&identities).find(|(earlier, other)| {
            earlier.real == real || identity.is_some() && **other == identity
        }) {
            return Err(ReadError::SameFile {
                file: earlier.target.file.clone(),
                other: target.file,
                real,
            });
        }
        snapshots.push(TargetSnapshot { target, real, file });
        identities.push(identity);
    }
    Ok(Snapshot { targets: snapshots })
}

/// A present file's device and inode.
type FileId = (u64, u64);

/// The target's resolved real file, its state and, when present, its identity.
fn load(target: &Target) -> Result<(PathBuf, FileState, Option<FileId>), ReadError> {
    let io = |source| ReadError::Io {
        file: target.file.clone(),
        source,
    };
    let real = resolve(&target.file).map_err(io)?;
    let bytes = match fs::read(&real) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok((real, FileState::Absent, None));
        }
        Err(error) => return Err(io(error)),
    };
    let metadata = fs::metadata(&real).map_err(io)?;
    let document = format::parse(target.format, &bytes).map_err(|source| ReadError::Parse {
        file: target.file.clone(),
        source,
    })?;
    Ok((
        real,
        FileState::Present {
            bytes,
            permissions: metadata.permissions(),
            document,
        },
        Some((metadata.dev(), metadata.ino())),
    ))
}

/// The real file `path` names, or for an absent file the real path it would be created at.
fn resolve(path: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        resolved => return resolved,
    }
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()) {
        return resolve(&parent.join(fs::read_link(path)?));
    }
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::from(ErrorKind::NotFound))?;
    Ok(resolve(parent)?.join(name))
}
