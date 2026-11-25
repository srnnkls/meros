use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// The file operations `apply` performs, so a test can interleave a foreign write.
pub(crate) trait Fs {
    /// The file's bytes, or `None` when it does not exist.
    fn read(&mut self, path: &Path) -> io::Result<Option<Vec<u8>>> {
        match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn create_dir_all(&mut self, path: &Path) -> io::Result<()> {
        fs::create_dir_all(path)
    }

    /// Creates `path` readable by its owner only, failing when it exists, and syncs `bytes`
    /// to it; a failed write removes it again.
    fn write_new(&mut self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        if written.is_err() {
            let _ = fs::remove_file(path);
        }
        written
    }

    /// The permissions a file created at `path` gets by default, read from an empty probe
    /// there, failing when `path` exists.
    fn default_permissions(&mut self, path: &Path) -> io::Result<Permissions> {
        let probe = OpenOptions::new().write(true).create_new(true).open(path)?;
        let permissions = probe.metadata().map(|metadata| metadata.permissions());
        fs::remove_file(path)?;
        permissions
    }

    fn set_permissions(&mut self, path: &Path, permissions: Permissions) -> io::Result<()> {
        fs::set_permissions(path, permissions)
    }

    fn rename(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }

    /// Links `to` to `from`'s file, failing when `to` exists.
    fn hard_link(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        fs::hard_link(from, to)
    }

    fn remove_file(&mut self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
}

/// The real filesystem.
pub(crate) struct Real;

impl Fs for Real {}
