//! One running copy per configuration folder.
//!
//! The document names a mutex for this. A lock on a file next to the configuration does the
//! same with the standard library alone (no Win32 binding): the lock is taken for the life of
//! the process and the system releases it when the process ends, however it ends. A second
//! copy that starts finds the file locked and says so.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

/// The name of the lock file in the configuration folder.
pub(crate) const LOCK_FILE: &str = "homelink.lock";

/// Held while this copy runs; dropping it (or ending the process) frees the lock.
#[derive(Debug)]
pub(crate) struct InstanceGuard {
    _file: File,
}

/// The result of asking for the lock.
#[derive(Debug)]
pub(crate) enum Acquired {
    /// This is the only copy.
    Yes(InstanceGuard),
    /// Another copy holds the lock.
    AlreadyRunning,
}

/// Takes the lock on `path`, creating the file if needed.
pub(crate) fn acquire(path: &Path) -> io::Result<Acquired> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Acquired::Yes(InstanceGuard { _file: file })),
        Err(TryLockError::WouldBlock) => Ok(Acquired::AlreadyRunning),
        Err(TryLockError::Error(err)) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("homelink-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(LOCK_FILE)
    }

    #[test]
    fn a_second_copy_is_refused_while_the_first_runs_and_admitted_after() {
        let path = temp_file("twice");
        let first = acquire(&path).unwrap();
        let Acquired::Yes(guard) = first else {
            panic!("the first copy must get the lock");
        };
        assert!(matches!(acquire(&path).unwrap(), Acquired::AlreadyRunning));
        drop(guard);
        assert!(matches!(acquire(&path).unwrap(), Acquired::Yes(_)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn different_folders_do_not_conflict() {
        let a = temp_file("a");
        let b = temp_file("b");
        let _first = acquire(&a).unwrap();
        assert!(matches!(acquire(&b).unwrap(), Acquired::Yes(_)));
        let _ = std::fs::remove_dir_all(a.parent().unwrap());
        let _ = std::fs::remove_dir_all(b.parent().unwrap());
    }

    #[test]
    fn an_unwritable_place_is_an_error() {
        let path = std::env::temp_dir()
            .join("homelink-no-such-dir-7f3a")
            .join(LOCK_FILE);
        assert!(acquire(&path).is_err());
    }
}
