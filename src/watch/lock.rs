//! Advisory file locking for cross-process writer detection.
//!
//! Provides a lightweight mechanism for reader processes to detect whether
//! a writer process currently has the database open. This is complementary
//! to redb's own lock — it uses a separate lock file specifically for
//! cross-process watch coordination.
//!
//! # Lock File
//!
//! The lock file is created at `{db_path}.watch.lock`. The writer holds
//! an exclusive lock; readers hold shared locks.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

// Import lock methods but not unlock — unlock requires Rust 1.89+ (above our MSRV).
// File drop releases the lock on all platforms.
use fs2::FileExt;

/// Advisory file lock for cross-process watch coordination.
///
/// The writer process acquires an exclusive lock, and reader processes
/// can check whether a writer is active before polling for changes.
///
/// The lock is automatically released when this struct is dropped.
///
/// # Example
///
/// ```rust
/// # fn main() -> pulsedb::Result<()> {
/// # let dir = tempfile::tempdir().unwrap();
/// # let db_path = dir.path().join("test.db");
/// # let db = pulsedb::PulseDB::open(&db_path, pulsedb::Config::default())?;
/// use pulsedb::WatchLock;
///
/// // Writer process
/// let lock = WatchLock::acquire_exclusive(&db_path)?;
/// // ... write experiences ...
/// drop(lock); // releases lock
///
/// // Reader process
/// if WatchLock::is_writer_active(&db_path) {
///     let seq = db.get_current_sequence()?;
///     let (events, _new_seq) = db.poll_changes(seq)?;
///     # let _ = events;
/// }
/// # Ok(())
/// # }
/// ```
pub struct WatchLock {
    /// The locked file handle. Lock is held as long as this exists.
    _file: File,

    /// Path to the lock file (for Display/Debug).
    path: PathBuf,
}

impl WatchLock {
    /// Returns the lock file path for a given database path.
    fn lock_path(db_path: &Path) -> PathBuf {
        let mut lock_path = db_path.as_os_str().to_owned();
        lock_path.push(".watch.lock");
        PathBuf::from(lock_path)
    }

    /// Opens or creates the lock file.
    fn open_lock_file(db_path: &Path) -> io::Result<(File, PathBuf)> {
        let path = Self::lock_path(db_path);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        Ok((file, path))
    }

    /// Acquires an exclusive lock (for the writer process).
    ///
    /// Blocks until the lock is available. Only one exclusive lock can
    /// be held at a time.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock file cannot be created or locked.
    pub fn acquire_exclusive(db_path: &Path) -> io::Result<Self> {
        let (file, path) = Self::open_lock_file(db_path)?;
        file.lock_exclusive()?;
        Ok(Self { _file: file, path })
    }

    /// Acquires a shared lock (for reader processes).
    ///
    /// Multiple shared locks can coexist. A shared lock blocks exclusive
    /// locks from being acquired.
    ///
    /// # Errors
    ///
    /// Returns an error if the lock file cannot be created or locked.
    pub fn acquire_shared(db_path: &Path) -> io::Result<Self> {
        let (file, path) = Self::open_lock_file(db_path)?;
        file.lock_shared()?;
        Ok(Self { _file: file, path })
    }

    /// Tries to acquire an exclusive lock without blocking.
    ///
    /// Returns `Ok(Some(lock))` if acquired, `Ok(None)` if another
    /// process holds the lock.
    pub fn try_exclusive(db_path: &Path) -> io::Result<Option<Self>> {
        let (file, path) = Self::open_lock_file(db_path)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { _file: file, path })),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            // fs2 on some platforms returns Other instead of WouldBlock
            Err(e) if e.raw_os_error().is_some() => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Checks whether a writer currently holds the exclusive lock.
    ///
    /// This is a non-blocking check. Returns `true` if an exclusive lock
    /// is held (writer is active), `false` otherwise.
    ///
    /// Note: This has a TOCTOU race — the writer could start or stop
    /// between this check and any subsequent action. Use it as a hint,
    /// not a guarantee.
    pub fn is_writer_active(db_path: &Path) -> bool {
        let (file, _path) = match Self::open_lock_file(db_path) {
            Ok(f) => f,
            Err(_) => return false,
        };
        // If we can get an exclusive lock, no writer is active.
        // Dropping the file handle releases the lock.
        match file.try_lock_exclusive() {
            Ok(()) => {
                // We got it — no writer. Drop releases the lock.
                drop(file);
                false
            }
            Err(_) => {
                // Lock is held — writer is active
                true
            }
        }
    }

    /// Returns the path to the lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

// No explicit Drop needed — dropping the File handle closes it, which
// releases the advisory lock on all platforms (POSIX and Windows).

impl std::fmt::Debug for WatchLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchLock")
            .field("path", &self.path)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_db_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        (dir, db_path)
    }

    #[test]
    fn test_exclusive_lock_acquired() {
        let (_dir, db_path) = test_db_path();
        let lock = WatchLock::acquire_exclusive(&db_path).unwrap();
        assert!(lock.path().exists());
    }

    #[test]
    fn test_shared_lock_acquired() {
        let (_dir, db_path) = test_db_path();
        let lock = WatchLock::acquire_shared(&db_path).unwrap();
        assert!(lock.path().exists());
    }

    #[test]
    fn test_multiple_shared_locks() {
        let (_dir, db_path) = test_db_path();
        let _lock1 = WatchLock::acquire_shared(&db_path).unwrap();
        let _lock2 = WatchLock::acquire_shared(&db_path).unwrap();
        // Both held simultaneously — should not deadlock
    }

    #[test]
    fn test_exclusive_blocks_second_exclusive() {
        let (_dir, db_path) = test_db_path();
        let _lock = WatchLock::acquire_exclusive(&db_path).unwrap();
        // try_exclusive should fail (lock is held)
        let result = WatchLock::try_exclusive(&db_path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_is_writer_active_when_locked() {
        let (_dir, db_path) = test_db_path();
        let _lock = WatchLock::acquire_exclusive(&db_path).unwrap();
        assert!(WatchLock::is_writer_active(&db_path));
    }

    #[test]
    fn test_is_writer_not_active_when_unlocked() {
        let (_dir, db_path) = test_db_path();
        // Create then drop lock
        {
            let _lock = WatchLock::acquire_exclusive(&db_path).unwrap();
        }
        assert!(!WatchLock::is_writer_active(&db_path));
    }

    #[test]
    fn test_is_writer_active_no_lock_file() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("nonexistent.db");
        // No lock file exists — should return false (not panic)
        assert!(!WatchLock::is_writer_active(&db_path));
    }

    #[test]
    fn test_lock_released_on_drop() {
        let (_dir, db_path) = test_db_path();
        {
            let _lock = WatchLock::acquire_exclusive(&db_path).unwrap();
        }
        // After drop, we should be able to acquire again
        let lock = WatchLock::try_exclusive(&db_path).unwrap();
        assert!(lock.is_some());
    }
}
