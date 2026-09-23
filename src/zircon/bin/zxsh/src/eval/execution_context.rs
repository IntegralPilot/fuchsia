// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::File;
use std::os::fd::BorrowedFd;
use std::sync::Arc;

use crate::collections::{FlatMap, FlatSet};
use crate::errors::io_err_str;
use crate::fd::Fd;
use crate::tty::{PtyControl, ShellSignalState, get_pty_control};
use bstr::BString;

/// A reader implementation that always fails with `EBADF` (Bad file descriptor), representing a
/// closed file descriptor.
pub struct ClosedReader;
impl std::io::Read for ClosedReader {
    fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from_raw_os_error(libc::EBADF))
    }
}

/// A writer implementation that always fails with `EBADF` (Bad file descriptor), representing a
/// closed file descriptor.
pub struct ClosedWriter;
impl std::io::Write for ClosedWriter {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::from_raw_os_error(libc::EBADF))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(libc::EBADF))
    }
}

/// Maintains the runtime I/O and environment execution context for a command or subshell.
pub struct ExecutionContext {
    /// Standard input file descriptor (FD 0), if open.
    stdin: Option<File>,
    /// Standard output file descriptor (FD 1), if open.
    stdout: Option<File>,
    /// Standard error file descriptor (FD 2), if open.
    stderr: Option<File>,
    /// Additional open file descriptors beyond standard I/O (FDs 3 and higher).
    extra_fds: FlatMap<Fd, File>,
    /// Set of alias names currently undergoing expansion, used to prevent infinite recursion.
    pub active_aliases: FlatSet<BString>,
    /// Handle to the terminal PTY control interface, if connected to a TTY.
    pty_control: Option<Arc<PtyControl>>,
    /// Shared state tracking pending shell signals (such as `SIGINT`).
    pub signal_state: ShellSignalState,
}

impl ExecutionContext {
    /// Initializes the root execution context by duplicating the standard process file descriptors
    /// (0, 1, 2).
    pub fn initial() -> Result<Self, String> {
        let stdin = dup_fd_to_file(Fd::STDIN)?;
        let pty_control = get_pty_control(&stdin).map(Arc::new);
        let signal_state = ShellSignalState::new();
        Ok(Self {
            stdin: Some(stdin),
            stdout: Some(dup_fd_to_file(Fd::STDOUT)?),
            stderr: Some(dup_fd_to_file(Fd::STDERR)?),
            extra_fds: FlatMap::new(),
            active_aliases: FlatSet::new(),
            pty_control,
            signal_state,
        })
    }

    /// Returns a reference to the open standard input file, if available.
    pub fn stdin(&self) -> Option<&File> {
        self.stdin.as_ref()
    }

    /// Returns a reference to the open standard output file, if available.
    pub fn stdout(&self) -> Option<&File> {
        self.stdout.as_ref()
    }

    /// Returns a reference to the open standard error file, if available.
    pub fn stderr(&self) -> Option<&File> {
        self.stderr.as_ref()
    }

    /// Returns a shared handle to the terminal PTY control interface, if available.
    pub fn pty_control(&self) -> Option<Arc<PtyControl>> {
        self.pty_control.clone()
    }

    /// Creates an independent clone of this execution context by duplicating all open file
    /// descriptors.
    pub fn try_clone(&self) -> Result<Self, String> {
        let stdin = self.stdin.as_ref().map(|f| f.try_clone().map_err(io_err_str)).transpose()?;
        let stdout = self.stdout.as_ref().map(|f| f.try_clone().map_err(io_err_str)).transpose()?;
        let stderr = self.stderr.as_ref().map(|f| f.try_clone().map_err(io_err_str)).transpose()?;
        let mut extra_fds = FlatMap::new();
        for &(k, ref v) in self.extra_fds.iter() {
            extra_fds.insert(k, v.try_clone().map_err(io_err_str)?);
        }
        let active_aliases = self.active_aliases.clone();
        let pty_control = self.pty_control.clone();
        let signal_state = self.signal_state.clone();
        Ok(Self { stdin, stdout, stderr, extra_fds, active_aliases, pty_control, signal_state })
    }

    /// Assigns or replaces the open file associated with the given file descriptor number.
    pub fn set_fd(&mut self, fd: Fd, file: File) {
        match fd {
            Fd::STDIN => self.stdin = Some(file),
            Fd::STDOUT => self.stdout = Some(file),
            Fd::STDERR => self.stderr = Some(file),
            _ => {
                self.extra_fds.insert(fd, file);
            }
        }
    }

    /// Duplicates the open file associated with the specified file descriptor number.
    pub fn dup_fd(&self, fd: Fd) -> Result<File, String> {
        let file_opt = match fd {
            Fd::STDIN => self.stdin.as_ref(),
            Fd::STDOUT => self.stdout.as_ref(),
            Fd::STDERR => self.stderr.as_ref(),
            _ => self.extra_fds.get(&fd),
        };
        match file_opt {
            Some(file) => file.try_clone().map_err(io_err_str),
            None => Err(io_err_str(std::io::Error::from_raw_os_error(libc::EBADF))),
        }
    }

    /// Closes the file descriptor associated with the given number.
    pub fn close_fd(&mut self, fd: Fd) {
        match fd {
            Fd::STDIN => self.stdin = None,
            Fd::STDOUT => self.stdout = None,
            Fd::STDERR => self.stderr = None,
            _ => {
                self.extra_fds.remove(&fd);
            }
        }
    }

    /// Prints a diagnostic message prefixed with `zxsh: ` to standard error, if open.
    pub fn print_err(&self, msg: &str) -> Result<(), String> {
        use std::io::Write;
        if let Some(ref file) = self.stderr {
            let mut writer = file;
            writeln!(&mut writer, "zxsh: {}", msg).map_err(io_err_str)
        } else {
            Ok(())
        }
    }
}

fn dup_fd_to_file(fd: Fd) -> Result<File, String> {
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd.raw()) };
    borrowed
        .try_clone_to_owned()
        .map(File::from)
        .map_err(|e| format!("dup_fd failed for {}: {}", fd, io_err_str(e)))
}
