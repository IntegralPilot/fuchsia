// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fdio::{SpawnAction, SpawnOptions};
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{Error as IOError, Read, Write};
use std::os::fd::AsFd as _;
use std::sync::Arc;
use zx::{MonotonicInstant, Signals, Socket, Status, StatusExt};

pub struct BlockingSocket {
    // Holding a reference to File so it lives as long as the associated socket. If the FD is closed
    // the socket closes with it. This way the process object can be dropped before the blocking
    // socket.
    _file: Arc<File>,
    socket: Arc<Socket>,
}

impl Read for BlockingSocket {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, IOError> {
        if self.socket.outstanding_read_bytes().map_err(|s| s.into_io_error())? == 0 {
            let wait_sigs = Signals::SOCKET_READABLE | Signals::SOCKET_PEER_CLOSED;
            let signals = self
                .socket
                .wait_one(wait_sigs, MonotonicInstant::INFINITE)
                .to_result()
                .map_err(|s| s.into_io_error())?;
            if signals.contains(Signals::SOCKET_PEER_CLOSED) {
                return Err(Status::PEER_CLOSED.into_io_error());
            }
        }
        self.socket.read(buf).or_else(|status| match status {
            Status::SHOULD_WAIT => Ok(0),
            _ => Err(status.into_io_error()),
        })
    }
}

impl Write for BlockingSocket {
    fn write(&mut self, buf: &[u8]) -> Result<usize, IOError> {
        self.socket.write(buf).map_err(|s| s.into_io_error())
    }

    fn flush(&mut self) -> Result<(), IOError> {
        Ok(())
    }
}

fn cstr(orig: &str) -> CString {
    CString::new(orig).expect("CString::new failed")
}

/// Spawns and owns a child process and provides access to its stdio.
pub struct TestProcess {
    stdin_file: Arc<File>,
    stdin: Arc<Socket>,
    stdout_file: Arc<File>,
    stdout: Arc<Socket>,
    stderr_file: Arc<File>,
    stderr: Arc<Socket>,
    process: zx::Process,
}

impl TestProcess {
    pub fn spawn(path: &str, args: &[&str]) -> Result<TestProcess, Error> {
        let job = zx::Job::from(zx::NullableHandle::invalid());
        let cpath = cstr(path);
        let (stdin_file, stdin_sock) = fdio::pipe_half().expect("Failed to make pipe");
        let (stdout_file, stdout_sock) = fdio::pipe_half().expect("Failed to make pipe");
        let (stderr_file, stderr_sock) = fdio::pipe_half().expect("Failed to make pipe");
        let mut spawn_actions = [
            SpawnAction::clone_fd(stdin_file.as_fd(), 0),
            SpawnAction::clone_fd(stdout_file.as_fd(), 1),
            SpawnAction::clone_fd(stderr_file.as_fd(), 2),
        ];

        let cstrags: Vec<CString> = args.iter().map(|x| cstr(x)).collect();
        let mut cargs: Vec<&CStr> = cstrags.iter().map(|x| x.as_c_str()).collect();
        cargs.insert(0, cpath.as_c_str());
        let process = fdio::spawn_etc(
            &job,
            SpawnOptions::CLONE_ALL,
            cpath.as_c_str(),
            cargs.as_slice(),
            None,
            &mut spawn_actions,
        )
        .map_err(|(status, msg)| format_err!("Unable to spawn process {} {}", status, msg))?;
        Ok(TestProcess {
            stdin_file: Arc::new(stdin_file),
            stdin: Arc::new(stdin_sock),
            stdout_file: Arc::new(stdout_file),
            stdout: Arc::new(stdout_sock),
            stderr_file: Arc::new(stderr_file),
            stderr: Arc::new(stderr_sock),
            process,
        })
    }

    pub fn stdin_blocking(&self) -> BlockingSocket {
        BlockingSocket { _file: self.stdin_file.clone(), socket: self.stdin.clone() }
    }

    pub fn stdout_blocking(&self) -> BlockingSocket {
        BlockingSocket { _file: self.stdout_file.clone(), socket: self.stdout.clone() }
    }

    pub fn stderr_blocking(&self) -> BlockingSocket {
        BlockingSocket { _file: self.stderr_file.clone(), socket: self.stderr.clone() }
    }

    pub fn process(&self) -> &zx::Process {
        &self.process
    }
}

#[cfg(test)]
use std::io::{BufRead, BufReader};

#[test]
fn avrcp_controller_help_string() {
    let child = TestProcess::spawn("/pkg/bin/bt-avrcp-controller", &["--help"])
        .expect("unable to run AVRCP tool");
    let mut ret = String::new();
    let mut reader = BufReader::new(child.stdout_blocking());
    for _ in 0..3 {
        let _ = reader.read_line(&mut ret).expect("Unable to read stdout");
    }

    assert!(ret.contains("Usage: "), "found: {}", ret);
    assert!(ret.contains("Bluetooth AVRCP Controller CLI"));
}

#[test]
fn bt_snoop_cli_help_string() {
    let child = TestProcess::spawn("/pkg/bin/bt-snoop-cli", &["--help"])
        .expect("unable to run bt-snoop-cli tool");
    let mut ret = String::new();
    let mut reader = BufReader::new(child.stdout_blocking());
    for _ in 0..3 {
        let _ = reader.read_line(&mut ret).expect("Unable to read stdout");
    }

    assert!(ret.contains("Usage: "), "found: {}", ret);
    assert!(ret.contains("Snoop Bluetooth controller packets"), "found: {}", ret);
    // Ensure that the doc comment on the type isn't leaking into the help docs.
    assert!(!ret.contains("command line arguments"), "found: {}", ret);
}
