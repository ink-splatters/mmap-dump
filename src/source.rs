use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("failed to open {}", .path.display())]
    Open {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("{} is exclusively locked by another process", .path.display())]
    Locked { path: PathBuf },

    #[error("failed to acquire a shared lock on {}", .path.display())]
    Lock {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("failed to inspect {}", .path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("{} is not a regular file", .path.display())]
    NotRegular { path: PathBuf },
}

/// A read-only regular file with a shared lock and a length snapshot.
///
/// The lock prevents modifications by writers using the same locking protocol.
/// On Linux, `File` locks use `flock` and remain advisory.
#[derive(Debug)]
pub struct LockedFile {
    file: File,
    len: u64,
}

impl LockedFile {
    pub fn open(path: &Path) -> Result<Self, SourceError> {
        let file = File::open(path).map_err(|source| SourceError::Open {
            path: path.to_owned(),
            source,
        })?;

        match file.try_lock_shared() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(SourceError::Locked {
                    path: path.to_owned(),
                });
            }
            Err(TryLockError::Error(error)) => {
                return Err(SourceError::Lock {
                    path: path.to_owned(),
                    source: error,
                });
            }
        }

        let metadata = file.metadata().map_err(|source| SourceError::Inspect {
            path: path.to_owned(),
            source,
        })?;
        if !metadata.is_file() {
            return Err(SourceError::NotRegular {
                path: path.to_owned(),
            });
        }

        Ok(Self {
            file,
            len: metadata.len(),
        })
    }

    pub const fn len(&self) -> u64 {
        self.len
    }

    pub const fn handle(&self) -> &File {
        &self.file
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use tempfile::TempDir;

    use super::{LockedFile, SourceError};

    #[test]
    fn classifies_a_missing_file() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let path = directory.path().join("missing");

        let error = LockedFile::open(&path).expect_err("the missing file should fail");

        match error {
            SourceError::Open {
                path: error_path,
                source,
            } => {
                assert_eq!(error_path, path);
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("expected an open error, got {other:?}"),
        }
    }

    #[test]
    fn classifies_a_non_regular_file() {
        let directory = TempDir::new().expect("temporary directory should be created");

        let error = LockedFile::open(directory.path()).expect_err("a directory should be rejected");

        assert!(matches!(error, SourceError::NotRegular { path } if path == directory.path()));
    }
}
