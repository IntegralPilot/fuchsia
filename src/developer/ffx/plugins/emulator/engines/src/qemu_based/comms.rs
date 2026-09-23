// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Provides an abstract interface for connecting and interacting with emulator consoles.
//! Currently, this is built around the consoles we have available in Qemu-based emulators,
//! as exposed by Linux sockets, but should be easily extendable to similar console types.
//!
//! Currently the interface is pretty sparse, as we're only beginning to explore the
//! capabilities of these consoles. The emulator "console" command can create a new socket,
//! and attach threads to read and write on that socket. This enables manual experimentation
//! on an emulator launched with `ffx emu`. As we shift other functionality to the consoles
//! (e.g. status queries, shutdown commands, etc.), this is where that functionality will be
//! exposed.
use fho::{Result, bug, return_bug};
use nix::NixPath;
use std::env;
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread::{JoinHandle, spawn};

/// Maximum path length for Linux. This is used to check the length of the socket addresses, if
/// needed.
const MAX_PATH: u64 = 4095;

trait CommsBackend<T>
where
    T: Read + Write,
{
    /// Establishes a connection with the emulator's socket specified by the 'socket_path'
    /// parameter. Stores the connection as part of this object for future use. Returns a
    /// Result containing the connection.
    fn connect(socket_path: &PathBuf) -> Result<T>;
}

pub struct QemuSocket {
    socket: Option<UnixStream>,
    socket_path: PathBuf,
}

impl QemuSocket {
    // Constructs a new instance of the socket. Requires a single parameter, which is the absolute
    // path to a Linux socket which exposes a console attached to the emulator. The path is stored
    // internally for future reference.
    pub fn new(path: &PathBuf) -> Self {
        Self { socket_path: path.clone(), socket: None }
    }

    // Establishes a connection with the socket indicated during construction. If the connection is
    // already established, this is a no-op; otherwise, a UnixStream object is created and
    // connected to, then stored for later use. Any errors are returned to the caller.
    pub fn connect(&mut self) -> Result<()> {
        if self.socket.is_none() {
            match <Self as CommsBackend<UnixStream>>::connect(&self.socket_path) {
                Ok(s) => self.socket = Some(s),
                Err(e) => return_bug!("Error connecting to the backend: {e}"),
            }
        }
        Ok(())
    }

    // Returns a clone of the socket wrapped by this object, if one exists, or None if no
    // connection has been established. This provides direct access to the underlying
    // UnixStream, rather than wrapping any particular command or query.
    pub fn stream(&self) -> Option<UnixStream> {
        match &self.socket {
            Some(s) => s.try_clone().ok(),
            None => None,
        }
    }
}

impl CommsBackend<UnixStream> for QemuSocket {
    fn connect(absolute_socket_path: &PathBuf) -> Result<UnixStream> {
        // On Linux, the path length to a socket file is severely limited. To work around this,
        // we temporarily change the working directory to the same location as the socket, then
        // access it with a relative "./socket" path instead of the absolute path stored in the
        // configuration.

        let stream_result = if absolute_socket_path.len()
            > MAX_PATH.try_into().map_err(|e| bug!("Getting Max Path: {e}."))?
        {
            let cwd =
                env::current_dir().map_err(|e| bug!("Getting current working directory: {e}."))?;

            // This ensures we are running in the right location relative to the socket file. If the
            // socket_path has no parent, we stay in the current directory.
            //
            // The downside is changing the current working directory for the process while creating the
            // socket which potentially could confuse other parts of the code that are doing cwd()
            // relative paths.
            //
            // Print a warning and log the fact that we did this risky thing.
            let msg_start = "Warning: path to emulator socket is too long, temporaily changing the working directory.";
            let msg_restored = "Current directory restored";
            eprintln!("{msg_start}");
            log::warn!("{msg_start}");
            let path = absolute_socket_path.parent().unwrap_or(&cwd);
            env::set_current_dir(path).map_err(|e| {
                bug!("Error changing to instance directory {}: {e}.", path.display())
            })?;

            // Connect to the indicated socket. If the path has no filename component, the connect call
            // will fail and the error will be returned to the caller along with the invalid path.
            let console = absolute_socket_path.file_name().unwrap_or_else(|| OsStr::new(""));
            let result = UnixStream::connect(console);

            // Reset the working directory before dealing with the result of the connect() call. This
            // way, even if we failed to connect to the socket, we revert the CWD every time.
            env::set_current_dir(cwd)
                .map_err(|e| bug!("Returning to original working directory: {e}"))?;

            eprintln!("{msg_restored}");
            log::warn!("{msg_restored}");
            result
        } else {
            UnixStream::connect(absolute_socket_path)
        };

        let stream = stream_result.map_err(|e| {
            bug!("Connecting to socket backend at {}: {e}.", absolute_socket_path.display())
        })?;

        Ok(stream)
    }
}

// Given an input (reader) and an output (writer), this function spawns a thread to route the data
// between them. For example, if reader is stdin and writer is a socket, this will pipe all data
// entered to stdin directly to the socket. The third parameter, tx, is the Sender side of a
// Channel; a message will be written to the channel if the pipe is ever broken, allowing the
// parent thread to monitor the health of the pipe.
pub(crate) fn spawn_pipe_thread<R, W>(
    mut reader: R,
    mut writer: W,
    tx: Sender<String>,
) -> JoinHandle<()>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    spawn(move || {
        let mut buf = [0; 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    tx.send("Connection lost.".to_string()).expect("Couldn't write to channel.");
                    break;
                }
                Ok(len) => {
                    if let Err(e) = writer.write_all(&buf[..len]) {
                        tx.send(format!("{:?}", e)).expect("Couldn't write to channel.");
                        break;
                    }
                }
                Err(e) => {
                    tx.send(format!("{:?}", e)).expect("Couldn't write to channel.");
                    break;
                }
            }
            writer.flush().expect("Flushing socket.");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fho::bug;
    use std::fs::create_dir_all;
    use std::os::unix::net::UnixListener;
    use tempfile::tempdir;

    const TEST_CONSOLE: &'static str = "./test";

    #[fuchsia::test]
    fn test_socket_connect() -> Result<()> {
        // Set up a temporary directory.
        let temp = tempdir().expect("tempdir").path().to_path_buf();
        create_dir_all(&temp).expect("create temp");

        let socket_path = temp.join(TEST_CONSOLE);
        assert!(socket_path.len() < MAX_PATH.try_into().expect("type conversion"));

        // Set up a structure to hold the connection.
        let mut socket = QemuSocket::new(&socket_path);

        // Attempt to connect, when there's nothing to connect to, expect failure.
        assert!(socket.connect().is_err());
        assert!(socket.stream().is_none());

        let listener = UnixListener::bind(&socket_path).expect("bind socket");
        // Set up a side thread that will accept an incoming connection and then exit.
        std::thread::spawn(move || -> Result<()> {
            let _ = listener.accept().map_err(|e| bug!("{e}"));
            Ok(())
        });

        // Use the main thread to connect to the socket, then assert we got it.
        let result = socket.connect();
        assert!(result.is_ok(), "{:?}", result.err());
        assert!(socket.stream().is_some());

        // Connect again, knowing that the backing thread is not accepting, but there's already a
        // connection established, should still be Ok(());
        let result = socket.connect();
        assert!(result.is_ok(), "{:?}", result.err());

        Ok(())
    }
}
