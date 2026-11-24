// SPDX-License-Identifier: AGPL-3.0-or-later
//! One AlwyWatch at a time on a computer: a second launch asks the running one to come to the
//! front, then quits (two would share the database and play over each other).
//!
//! The lock is an OS file lock (released by the OS when the process ends, even on a crash); the
//! request is a file the running app looks for once a second.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

/// Held for the app's lifetime.
pub struct Instance {
    _lock: Option<File>,
    raise: PathBuf,
}

/// Another AlwyWatch runs.
pub struct AlreadyRunning;

/// Takes the lock in `dir`. When it can't be taken for another reason than another AlwyWatch
/// (read-only folder…), the app runs anyway.
pub fn acquire(dir: &Path) -> Result<Instance, AlreadyRunning> {
    let raise = dir.join("instance.raise");
    let _ = std::fs::create_dir_all(dir);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("instance.lock"));
    let lock = match file {
        Ok(f) => match f.try_lock() {
            Ok(()) => Some(f),
            Err(TryLockError::WouldBlock) => return Err(AlreadyRunning),
            Err(TryLockError::Error(_)) => None,
        },
        Err(_) => None,
    };
    // A request left from before (the app wasn't running to see it).
    let _ = std::fs::remove_file(&raise);
    Ok(Instance { _lock: lock, raise })
}

/// From the second launch: asks the running app to come forward.
pub fn ask_to_raise(dir: &Path) {
    let _ = std::fs::write(dir.join("instance.raise"), b"");
}

impl Instance {
    /// True once per request from another launch.
    pub fn raise_requested(&self) -> bool {
        std::fs::remove_file(&self.raise).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_launch_is_refused_and_raises_the_first() {
        let dir = std::env::temp_dir().join(format!("alwywatch-instance-{}", std::process::id()));
        let first = acquire(&dir).ok().expect("first launch runs");
        assert!(acquire(&dir).is_err(), "second launch must not run");
        assert!(!first.raise_requested());
        ask_to_raise(&dir);
        assert!(first.raise_requested());
        assert!(!first.raise_requested(), "once per request");
        drop(first);
        assert!(acquire(&dir).is_ok(), "the lock goes with the app");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
