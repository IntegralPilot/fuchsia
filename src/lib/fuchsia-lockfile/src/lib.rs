// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use log::{error, info, warn};
use nix::unistd::{self, Pid};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs::{File, Metadata, OpenOptions, remove_file};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Opens and controls a lockfile against a specific filename using create-only
/// file modes, deleting the file on drop. Writes the current pid to the file
/// so it can be checked for if the process that created it is still around
/// (manually for now).
#[derive(Debug)]
pub struct Lockfile {
    path: PathBuf,
    handle: Option<File>,
}

#[derive(Debug)]
/// An error while creating a lockfile, including the underlying file io error
/// and if possible the error
pub struct LockfileCreateError {
    /// The underlying error attempting to obtain the lockfile
    pub kind: LockfileCreateErrorKind,
    /// The filename of the lockfile being attempted
    pub lock_path: PathBuf,
    /// Details about the lockfile's original owner, from the file.
    pub owner: Option<LockContext>,
    /// Metadata about the lockfile if it was available
    pub metadata: Option<Metadata>,
}

#[derive(Debug)]
pub enum LockfileCreateErrorKind {
    Io(std::io::Error),
    TimedOut,
}

impl LockfileCreateError {
    fn new(lock_path: &Path, error: std::io::Error) -> Self {
        // if it was an already-exists error and the file is there, try to read it and set
        let owner = match error.kind() {
            std::io::ErrorKind::AlreadyExists => {
                File::open(&lock_path).ok().and_then(|f| serde_json::from_reader(f).ok())
            }
            _ => None,
        };
        let metadata = std::fs::metadata(lock_path).ok(); // purely advisory, ignore the error.
        let lock_path = lock_path.to_owned();
        Self { kind: LockfileCreateErrorKind::Io(error), lock_path, owner, metadata }
    }

    /// Validates if a lockfile's info is inherently invalid and should be removed or ignored.
    pub fn is_valid(&self, at: SystemTime, my_pid: u32) -> bool {
        let ctime = self.metadata.as_ref().and_then(|metadata| metadata.created().ok());
        // just warnings/info
        match self.owner.as_ref() {
            Some(ctx) if ctx.pid == my_pid => {
                info!(
                    "Overlapping access to the lockfile at {path} from the same process, \
                       this may be a sign of other issues.",
                    path = self.lock_path.display()
                );
            }
            _ => (),
        };
        // actual errors
        match (self.owner.as_ref(), ctime) {
            (_, Some(ctime)) if ctime > at + Duration::from_secs(2) => {
                warn!(
                    "Lockfile {path} is in the future somehow, considering it invalid \
                       (ctime: {ctime:?}, now: {at:?})",
                    path = self.lock_path.display()
                );
                false
            }
            (None, Some(ctime)) if ctime + Duration::from_secs(10) < at => {
                warn!(
                    "Lockfile {path} is older than 10s, so is probably stale, considering it \
                       invalid (ctime {ctime:?}, now: {at:?})",
                    path = self.lock_path.display()
                );
                false
            }
            (Some(ctx), _)
                if unistd::getpgid(Some(Pid::from_raw(ctx.pid as i32)))
                    == Err(nix::Error::ESRCH) =>
            {
                warn!(
                    "Lockfile {path} was created by a pid that no longer exists ({pid}), \
                       considering it invalid.",
                    path = self.lock_path.display(),
                    pid = ctx.pid
                );
                false
            }
            _ => true,
        }
    }

    /// Returns if the error timed out.
    pub fn is_timeout(&self) -> bool {
        matches!(self.kind, LockfileCreateErrorKind::TimedOut)
    }

    /// Removes the lockfile if it exists, consuming the error if it was removed
    /// or returning the error back if it failed in any other way than already not-existing.
    #[allow(clippy::result_large_err)]
    pub fn remove_lock(self) -> Result<(), Self> {
        match remove_file(&self.lock_path) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => {
                error!(
                    "Error trying to remove lockfile {lockfile}: {e:?}",
                    lockfile = self.lock_path.display()
                );
                Err(self)
            }
        }
    }
}

impl std::error::Error for LockfileCreateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.kind {
            LockfileCreateErrorKind::Io(err) => Some(err),
            LockfileCreateErrorKind::TimedOut => None,
        }
    }
}

impl std::fmt::Display for LockfileCreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            LockfileCreateErrorKind::Io(_) => {
                write!(
                    f,
                    "Error obtaining lock on {:?} (existing owner if present: '{:?}', our pid is {pid})",
                    self.lock_path,
                    self.owner,
                    pid = std::process::id(),
                )
            }
            LockfileCreateErrorKind::TimedOut => {
                write!(
                    f,
                    "Timed out obtaining lock on {:?} (existing owner if present: '{:?}', our pid is {pid})",
                    self.lock_path,
                    self.owner,
                    pid = std::process::id(),
                )
            }
        }
    }
}

/// A context identity used for the current process that can be used to identify where
/// a lockfile came from. A serialized version of this will be printed to the lockfile
/// when it's created.
///
/// It is not *guaranteed* to be unique across time, so it's not safe to assume that
/// `LockContext::current() == some_other_context` means it *was* the current process
/// or thread, but it should mean it wasn't if they're different.
#[derive(Clone, Serialize, Deserialize, Debug, Hash, PartialOrd, PartialEq)]
pub struct LockContext {
    pub pid: u32,
}

impl LockContext {
    pub fn current() -> Self {
        let pid = std::process::id();
        Self { pid }
    }

    pub fn write_to<W: Write>(&self, mut handle: W) -> Result<(), std::io::Error> {
        let context_str = serde_json::to_string(self).map_err(|e| std::io::Error::other(e))?;
        handle.write_all(context_str.as_bytes())
    }
}

impl Lockfile {
    /// Creates a lockfile at `filename` if possible. Returns the underlying error
    /// from the file create call. Note that this won't retry. Use [`Lockfile::lock`]
    /// or [`Lockfile::lock_for`] to do that.
    pub fn new(lock_path: &Path, context: LockContext) -> Result<Self, Box<LockfileCreateError>> {
        // create the lock file -- if this succeeds the file exists now and is brand new
        // so we can write our details to it.
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path)
            .and_then(|mut handle| {
                // but then once we've got the file, if we write to it and fail somehow
                // we have to clean up after ourselves, so immediately create the Lockfile
                // object, that way if we fail to write to it it will clean up after itself
                // in drop().
                let path = lock_path.to_owned();
                context.write_to(&handle)?;
                handle.flush()?;
                Ok(Self { handle: Some(handle), path })
            })
            .map_err(|e| Box::new(LockfileCreateError::new(lock_path, e)))
    }

    /// Creates a lockfile at `filename`.lock if possible. See [`Lockfile::new`] for details on
    /// the return value.
    pub fn new_for(
        filename: &Path,
        context: LockContext,
    ) -> Result<Self, Box<LockfileCreateError>> {
        let mut lock_path = filename.to_owned();
        let filename = lock_path.file_name().map_or_else(
            || ".lock".to_string(),
            |name| format!("{filename}.lock", filename = name.to_string_lossy()),
        );

        lock_path.set_file_name(&filename);
        Self::new(&lock_path, context)
    }

    /// Creates a lockfile at `filename` if possible, retrying until it succeeds or times out.
    ///
    /// If unable to lock within the constraints, will return the error from the last attempt.
    /// It will try with an increasing sleep between attempts up to `timeout`.
    async fn lock(lock_path: &Path, timeout: Duration) -> Result<Self, Box<LockfileCreateError>> {
        let end_time = Instant::now() + timeout;
        let context = LockContext::current();
        let mut sleep_time = 10;
        loop {
            let lock_result = Self::new(lock_path, context.clone());
            match lock_result {
                Ok(lockfile) => return Ok(lockfile),
                Err(e) if !e.is_valid(SystemTime::now(), context.pid) => {
                    // try to remove the invalid lockfile and immediately continue. If we get
                    // any error other than ENOTFOUND, there's likely an underlying issue
                    // with the filesystem and we should just bail immediately.
                    info!("Removing invalid lockfile {lockfile}", lockfile = lock_path.display());
                    e.remove_lock()?;
                }
                Err(e) if Instant::now() > end_time => {
                    return Err(Box::new(LockfileCreateError {
                        kind: LockfileCreateErrorKind::TimedOut,
                        lock_path: e.lock_path,
                        owner: e.owner,
                        metadata: e.metadata,
                    }));
                }
                _ => {
                    fuchsia_async::Timer::new(Duration::from_millis(sleep_time)).await;
                    // next time, retry with a longer wait, but not exponential, and not
                    // longer than it'll take to finish out the timeout.
                    let max_wait = end_time
                        .checked_duration_since(Instant::now())
                        .unwrap_or_default()
                        .as_millis()
                        .try_into()
                        .expect("Impossibly large wait time on lock");
                    sleep_time = (sleep_time * 2).clamp(0, max_wait);
                }
            };
        }
    }

    /// Creates a lockfile at `filename`.lock if possible, retrying until it succeeds or times out.
    ///
    /// See [`Lockfile::lock`] for more details.
    pub async fn lock_for(
        filename: &Path,
        timeout: Duration,
    ) -> Result<Self, Box<LockfileCreateError>> {
        let mut lock_path = filename.to_owned();
        let filename = lock_path.file_name().map_or_else(
            || ".lock".to_string(),
            |name| format!("{filename}.lock", filename = name.to_string_lossy()),
        );

        lock_path.set_file_name(&filename);
        Self::lock(&lock_path, timeout).await
    }

    /// Internal function used by both [`Self::unlock`] and [`Self::drop`]
    fn raw_unlock(lock_path: &Path) -> Result<(), std::io::Error> {
        // return an error unless it was just that the lockfile had already
        // been removed somehow (not a great sign but also not itself really
        // an error)
        match remove_file(lock_path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Explicitly remove the lockfile and consume the lockfile object
    pub fn unlock(mut self) -> Result<(), std::io::Error> {
        // Explicitly close the file handle so that the drop impl won't also try to remove the
        // lockfile.
        drop(self.handle.take());

        Self::raw_unlock(&self.path)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Lockfile {
    fn drop(&mut self) {
        // Only delete the lockfile if we haven't been explicitly unlocked.
        if let Some(handle) = self.handle.take() {
            drop(handle);

            if let Err(e) = Self::raw_unlock(&self.path) {
                // in Drop we can't really do much about this, so just warn about it
                warn!("Error removing lockfile {name}: {e:#?}", name = self.path.display());
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::io::{Read, Seek, Write};

    #[test]
    fn create_lockfile_works() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lockedfile");
        let lock = Lockfile::new(&path, LockContext::current()).unwrap();
        assert!(path.is_file(), "Lockfile {path:?} should exist");

        assert!(
            Lockfile::new(&path, LockContext::current()).is_err(),
            "Should not be able to create lockfile at {path:?} while one already exists."
        );
        lock.unlock().unwrap();
        assert!(!path.is_file(), "Lockfile {path:?} shouldn't exist");

        assert!(
            Lockfile::new(&path, LockContext::current()).is_ok(),
            "Should be able to make a new lockfile when the old one is unlocked."
        );
    }

    #[test]
    fn create_lockfile_for_other_file_works() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut path = dir.path().join("lockedfile");
        let lock = Lockfile::new_for(&path, LockContext::current()).unwrap();
        path.set_file_name("lockedfile.lock");
        assert!(path.is_file(), "Lockfile {path:?} should exist");

        assert!(
            Lockfile::new(&path, LockContext::current()).is_err(),
            "Should not be able to create lockfile at {path:?} while one already exists."
        );
        lock.unlock().unwrap();
        assert!(!path.is_file(), "Lockfile {path:?} shouldn't exist");

        assert!(
            Lockfile::new(&path, LockContext::current()).is_ok(),
            "Should be able to make a new lockfile when the old one is unlocked."
        );
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn lock_with_timeout() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut path = dir.path().join("lockedfile");
        let lock = Lockfile::lock_for(&path, Duration::from_secs(1)).await.unwrap();
        path.set_file_name("lockedfile.lock");
        assert!(path.is_file(), "Lockfile {path:?} should exist");

        Lockfile::lock(&path, Duration::from_secs(1))
            .await
            .err()
            .expect("Shouldn't be able to re-lock lock file with timeout");

        lock.unlock().unwrap();
        assert!(!path.is_file(), "Lockfile {path:?} shouldn't exist");

        Lockfile::lock(&path, Duration::from_secs(1)).await.unwrap();
    }

    #[test]
    fn lock_file_validity() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lockedfile.lock");
        let _lock = Lockfile::new(&path, LockContext::current()).unwrap();
        assert!(path.is_file(), "Lockfile {path:?} should exist");

        let err = Lockfile::new(&path, LockContext::current())
            .err()
            .expect("Should not be able to re-create lockfile");

        let now = SystemTime::now();
        let real_pid = std::process::id();
        assert!(
            err.is_valid(now, real_pid),
            "A just created lockfile should be valid from the same process (but may warn)"
        );
        assert!(
            err.is_valid(now, real_pid + 1),
            "A just created lockfile should be valid from another process"
        );
        assert!(
            !err.is_valid(now - Duration::from_secs(9999), real_pid),
            "A lockfile from the far future should be valid"
        );
    }

    #[test]
    fn ownerless_lockfile_validity() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lockedfile.lock");
        let _lock = File::create(&path).expect("Creating empty lock file");
        assert!(path.is_file(), "Lockfile {path:?} should exist");

        let err = Lockfile::new(&path, LockContext::current())
            .err()
            .expect("Should not be able to re-create lockfile");

        let now = SystemTime::now();
        let real_pid = std::process::id();
        assert!(
            err.is_valid(now, real_pid),
            "An ownerless lockfile that's fresh should be considered valid"
        );

        assert!(
            !err.is_valid(now + Duration::from_secs(9999), real_pid),
            "An ownerless lockfile from a long time ago should be invalid"
        );
    }

    #[test]
    fn non_running_pid_lockfile_validity() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lockedfile.lock");
        let _lock = Lockfile::new(&path, LockContext { pid: u32::MAX }).unwrap();
        assert!(path.is_file(), "Lockfile {path:?} should exist");

        let err = Lockfile::new(&path, LockContext::current())
            .err()
            .expect("Should not be able to re-create lockfile");

        let now = SystemTime::now();
        let real_pid = std::process::id();
        assert!(
            !err.is_valid(now, real_pid),
            "A lockfile owned by a pid that isn't running should be invalid"
        );
    }

    #[test]
    fn force_delete_nonexistent_lockfile_ok() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lockedfile.lock");
        let bogus_error = LockfileCreateError::new(&path, std::io::Error::other("stuff"));
        bogus_error.remove_lock().expect("Removing non-existent lock file");
    }

    #[test]
    fn force_delete_real_lockfile_ok() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut path = dir.path().join("lockedfile");
        let lock = Lockfile::new_for(&path, LockContext::current()).unwrap();
        path.set_file_name("lockedfile.lock");
        assert!(path.is_file(), "Lockfile {path:?} should exist");

        let err = Lockfile::new(&path, LockContext::current())
            .err()
            .expect("Should not be able to re-create lockfile");
        err.remove_lock().expect("Should be able to remove lock file from error");

        assert!(!path.is_file(), "Lockfile {path:?} should have been deleted");

        lock.unlock().expect("Unlock should have succeeded even though lock file was already gone");
    }

    #[fuchsia::test]
    async fn unlock_does_not_cause_a_race() {
        let dir = tempfile::TempDir::new().unwrap();
        let lockfile_path = dir.path().join("lock");
        let counter_path = dir.path().join("counter");

        std::fs::write(&counter_path, "0").unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let rx = std::sync::Arc::new(fuchsia_sync::Mutex::new(rx));

        let lock_count = 128;
        let thread_count = 128;
        let mut threads = vec![];

        for _thread_num in 0..thread_count {
            let lockfile_path = lockfile_path.clone();
            let counter_path = counter_path.clone();
            let rx = rx.clone();

            let thread = std::thread::spawn(move || {
                loop {
                    let rx = rx.lock();
                    let Ok(lock_num) = rx.recv() else {
                        break;
                    };

                    // Release the rx lock so other threads can run.
                    drop(rx);

                    // This implementation of lockfiles is not guaranteed to be fair, so it's
                    // possible to starve one or more threads. So loop again around our timeout
                    // until we get our lock.
                    let mut attempt = 0;
                    let lockfile = loop {
                        let mut executor = fuchsia_async::LocalExecutor::default();
                        let lockfile = executor.run_singlethreaded(async {
                            Lockfile::lock_for(&lockfile_path, Duration::from_secs(10)).await
                        });

                        match lockfile {
                            Ok(lockfile) => {
                                break lockfile;
                            }
                            Err(err) if err.is_timeout() => {
                                println!(
                                    "timed out getting lockfile, trying again. lock_num: {lock_num} attempt: {attempt}"
                                );

                                if attempt > 10 {
                                    panic!("failed to get a lockfile after 10 attempts");
                                }
                                attempt += 1;
                            }
                            Err(err) => {
                                panic!("unexpected error locking file: {:#?}", err);
                            }
                        }
                    };

                    let mut file =
                        OpenOptions::new().read(true).write(true).open(&counter_path).unwrap();

                    // Read the contents.
                    let mut contents = String::new();
                    file.read_to_string(&mut contents).unwrap();
                    let counter: u64 = contents.parse().unwrap();

                    // Increment the contents.
                    file.rewind().unwrap();
                    file.write_all((counter + 1).to_string().as_bytes()).unwrap();

                    // Explicitly release the lock.
                    lockfile.unlock().unwrap();
                }
            });

            threads.push(thread);
        }

        for lock_num in 0..lock_count {
            tx.send(lock_num).unwrap();
        }
        drop(tx);

        for thread in threads {
            thread.join().unwrap();
        }

        let contents = std::fs::read_to_string(&counter_path).unwrap();
        let counter: u64 = contents.parse().unwrap();

        assert_eq!(counter, lock_count);
    }
}
