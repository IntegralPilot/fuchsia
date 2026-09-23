// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::io;
use std::process::{Child, ExitStatus};

/// A guard that wraps a running `std::process::Child` and ensures it is terminated
/// and reaped if the guard is dropped before `wait` is explicitly called.
///
/// This prevents leaking orphan subprocesses when returning early (e.g. via `?` operator)
/// or panicking during I/O streaming.
#[derive(Debug)]
pub struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    /// Creates a new `ChildGuard` wrapping the provided `Child`.
    pub fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    /// Provides a mutable reference to the underlying `Child` if still managed.
    pub fn as_mut(&mut self) -> Option<&mut Child> {
        self.child.as_mut()
    }

    /// Waits for the subprocess to finish naturally and returns its `ExitStatus`.
    ///
    /// This consumes the guard and disarms it so `kill` will not be called on drop.
    pub fn wait(mut self) -> io::Result<ExitStatus> {
        let mut child = self.child.take().expect("child is present");
        child.wait()
    }

    /// Disarms the guard and returns the underlying `Child`.
    pub fn into_inner(mut self) -> Option<Child> {
        self.child.take()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Kill the child process to prevent it from leaking in the background.
            let _ = child.kill();
            // Wait for the child to be reaped by the OS to avoid zombie processes.
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn test_child_guard_normal_wait() {
        let child = Command::new("true")
            .spawn()
            .or_else(|_| Command::new("/bin/true").spawn())
            .expect("failed to spawn true");
        let guard = ChildGuard::new(child);
        let status = guard.wait().expect("wait failed");
        assert!(status.success());
    }

    #[test]
    fn test_child_guard_drop_kills_process() {
        let child = Command::new("sleep")
            .arg("60")
            .spawn()
            .or_else(|_| Command::new("/bin/sleep").arg("60").spawn())
            .expect("failed to spawn sleep");
        let pid = child.id();
        let guard = ChildGuard::new(child);

        // Dropping guard must kill and reap the child process.
        drop(guard);

        // Verify that the child process is dead.
        #[cfg(unix)]
        {
            let res = Command::new("kill").args(["-0", &pid.to_string()]).status();
            assert!(!res.map(|s| s.success()).unwrap_or(false));
        }
    }

    #[test]
    fn test_child_guard_into_inner() {
        let child = Command::new("true")
            .spawn()
            .or_else(|_| Command::new("/bin/true").spawn())
            .expect("failed to spawn true");
        let guard = ChildGuard::new(child);
        let mut inner = guard.into_inner().expect("expected inner child");
        let status = inner.wait().expect("wait failed");
        assert!(status.success());
    }
}
