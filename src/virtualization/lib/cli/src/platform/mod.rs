// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use blocking::Unblock;
use fidl_fuchsia_virtualization::{GuestManagerProxy, GuestMarker, GuestProxy};
use fuchsia_async as fasync;
use guest_cli_args::GuestType;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, RawFd};

#[cfg(target_os = "fuchsia")]
mod fuchsia;
#[cfg(target_os = "fuchsia")]
pub use fuchsia::*;

#[cfg(not(target_os = "fuchsia"))]
mod host;
#[cfg(not(target_os = "fuchsia"))]
pub use host::*;

// NOTE: This logic is partially duplicated in //src/virtualization/bin/guest/src/services.rs,
// but that will be removed once this migration to a common codebase is finished.
// TODO(https://fxbug.dev/42067874): Remove other implementation.
pub enum Stdio {
    Stdin,
    Stdout,
    Stderr,
}

impl AsRawFd for Stdio {
    fn as_raw_fd(&self) -> RawFd {
        match self {
            Stdio::Stdin => std::io::stdin().as_raw_fd(),
            Stdio::Stdout => std::io::stdout().as_raw_fd(),
            Stdio::Stderr => std::io::stderr().as_raw_fd(),
        }
    }
}

pub struct UnbufferedStdio(Option<std::fs::File>);

impl UnbufferedStdio {
    // Unbuffer stdio by creating a File from the raw FD, but without taking ownership of the FD.
    // This allows for using interactive commands, and the custom Drop implementation prevents
    // the file from closing the stdio FD upon destruction.
    fn new(stdio: Stdio) -> Self {
        // A note about safety: Using from_raw_fd requires that the fd remains valid. As this is
        // only using the process stdio handles, validity can be assumed for the life of th
        // process.
        unsafe { Self { 0: Some(std::fs::File::from_raw_fd(stdio.as_raw_fd())) } }
    }
}

impl AsRawFd for UnbufferedStdio {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_ref().unwrap().as_raw_fd()
    }
}

impl AsFd for UnbufferedStdio {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_ref().unwrap().as_fd()
    }
}

impl Drop for UnbufferedStdio {
    fn drop(&mut self) {
        // Prevent the file from closing the fd (which it doesn't own).
        _ = self.0.take().unwrap().into_raw_fd();
    }
}

impl Write for UnbufferedStdio {
    fn write(&mut self, buf: &[u8]) -> Result<usize, std::io::Error> {
        self.0.as_mut().unwrap().write(buf)
    }

    fn flush(&mut self) -> Result<(), std::io::Error> {
        self.0.as_mut().unwrap().flush()
    }
}

impl Read for UnbufferedStdio {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, std::io::Error> {
        self.0.as_mut().unwrap().read(buf)
    }
}

pub struct GuestConsole {
    input: Option<fasync::Socket>,
    output: Option<fasync::Socket>,
}

impl GuestConsole {
    pub fn new(input: fidl::Socket, output: fidl::Socket) -> Result<Self> {
        Ok(GuestConsole {
            input: Some(fasync::Socket::from_socket(input)),
            output: Some(fasync::Socket::from_socket(output)),
        })
    }

    pub fn get_unblocked_stdio(stdio: Stdio) -> Unblock<UnbufferedStdio> {
        Unblock::new(UnbufferedStdio::new(stdio))
    }

    pub async fn run<R: futures::io::AsyncRead + Unpin, W: futures::io::AsyncWrite + Unpin>(
        mut self,
        host_tx: R,
        mut host_rx: W,
    ) -> Result<()> {
        let mut input = self.input.take().expect("run can only be called once");
        let output = self.output.take().expect("run can only be called once");

        let guest_input = futures::io::copy(host_tx, &mut input);
        let guest_output = futures::io::copy(output, &mut host_rx);

        futures::future::try_select(guest_input, guest_output)
            .await
            .map(|_| ())
            .map_err(|e| e.factor_first().0.into())
    }

    pub async fn run_with_stdio(self) -> Result<()> {
        self.run(
            GuestConsole::get_unblocked_stdio(Stdio::Stdin),
            GuestConsole::get_unblocked_stdio(Stdio::Stdout),
        )
        .await
    }
}

#[async_trait(?Send)]
pub trait PlatformServices {
    async fn connect_to_manager(&self, guest_type: GuestType) -> Result<GuestManagerProxy>;

    async fn connect_to_guest(&self, guest_type: GuestType) -> Result<GuestProxy> {
        let guest_manager = self.connect_to_manager(guest_type).await?;
        let (guest, guest_server_end) = fidl::endpoints::create_proxy::<GuestMarker>();
        guest_manager.connect(guest_server_end).await?.map_err(|err| anyhow!("{:?}", err))?;
        Ok(guest)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[::fuchsia::test]
    async fn guest_console_copies_async_stream() {
        // Wire up a mock guest console, mocking out the virtio-console on a guest.
        let (guest_console_socket, guest_console_tx) = fidl::Socket::create_stream();
        let guest_console_rx =
            guest_console_tx.duplicate_handle(fidl::Rights::SAME_RIGHTS).unwrap();
        let guest_console = GuestConsole::new(guest_console_rx, guest_console_tx)
            .expect("failed to make guest console");

        // This represents a host's stdio. While stdout and stdin are unidirectional, sockets
        // are bidirectional so we can duplicate one and split it into both in and out.
        let (host_stdio, host_stdin_sock) = fidl::Socket::create_stream();
        let host_stdout_sock = host_stdin_sock.duplicate_handle(fidl::Rights::SAME_RIGHTS).unwrap();
        let host_stdout = fasync::Socket::from_socket(host_stdout_sock);
        let host_stdin = fasync::Socket::from_socket(host_stdin_sock);

        // Have the "guest" write this command to its console stdout, which should be pushed to our
        // stdout.
        let test_string = "Test Command";
        guest_console_socket.write(format!("{test_string}").as_bytes()).unwrap();

        // Drop the endpoint, which looks like the guest terminating (and sends an EOF).
        drop(guest_console_socket);

        guest_console.run(host_stdin, host_stdout).await.expect("failed to complete!");

        let mut buffer = [0; 1024];
        let n = host_stdio.read(&mut buffer[..]).expect("failed to read from socket");

        assert_eq!(n, test_string.len());
        assert_eq!(String::from_utf8(buffer[..n].to_vec()).unwrap(), test_string);
    }
}
