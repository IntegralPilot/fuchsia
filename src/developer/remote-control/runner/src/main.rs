// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use argh::FromArgs;
use compat_info::{CompatibilityState, ConnectionInfo};
use fidl_fuchsia_developer_remotecontrol_connector::ConnectorMarker;
use fuchsia_component::client::connect_to_protocol;
use futures::future::{poll_fn, select};
use futures::io::BufReader;
use futures::prelude::*;
use logging_util::FfxLogGuard;
use std::io::{self, Write as _};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::io::AsRawFd;
use std::pin::pin;
use std::task::{Poll, ready};
use version_history::AbiRevision;
use version_history_data::HISTORY;

const BUFFER_SIZE: usize = 65536;

#[derive(Copy, Clone)]
enum CopyDirection {
    StdIn,
    StdOut,
}

impl CopyDirection {
    fn log_read_fail(&self, err: io::Error) {
        match self {
            CopyDirection::StdIn => log::warn!("Failed receiving data from host: {err:?}"),
            CopyDirection::StdOut => log::warn!("Failed receiving data from RCS: {err:?}"),
        }
    }

    fn log_write_fail(&self, err: io::Error) {
        match self {
            CopyDirection::StdIn => log::warn!("Failed sending data to RCS: {err:?}"),
            CopyDirection::StdOut => log::warn!("Failed sending data to host: {err:?}"),
        }
    }

    fn log_write_zero(&self) {
        match self {
            CopyDirection::StdIn => log::error!(
                "Writing to RCS socket returned zero-byte success. This should be impossible?!"
            ),
            CopyDirection::StdOut => log::error!(
                "Writing to RCS socket returned zero-byte success. This should be impossible?!"
            ),
        }
    }

    fn log_flush_error(&self, err: io::Error) {
        match self {
            CopyDirection::StdIn => {
                log::warn!("Flushing data toward RCS after shutdown gave {err:?}")
            }
            CopyDirection::StdOut => {
                log::warn!("Flushing data toward the host after shutdown gave {err:?}")
            }
        }
    }

    fn log_closed(&self) {
        match self {
            CopyDirection::StdIn => {
                log::debug!("Stream from the host toward RCS terminated normally")
            }
            CopyDirection::StdOut => {
                log::debug!("Stream from RCS toward the host terminated normally")
            }
        }
    }
}

async fn buffered_copy<R, W>(mut from: R, to: W, dir: CopyDirection)
where
    R: AsyncRead + std::marker::Unpin,
    W: AsyncWrite + std::marker::Unpin,
{
    let mut from = pin!(BufReader::with_capacity(BUFFER_SIZE, &mut from));
    let mut to = pin!(to);
    poll_fn(move |cx| {
        loop {
            let buffer = match ready!(from.as_mut().poll_fill_buf(cx)) {
                Ok(x) => x,
                Err(e) => {
                    dir.log_read_fail(e);
                    return Poll::Ready(());
                }
            };
            if buffer.is_empty() {
                if let Err(e) = ready!(to.as_mut().poll_flush(cx)) {
                    dir.log_flush_error(e)
                }
                dir.log_closed();
                return Poll::Ready(());
            }

            let i = match ready!(to.as_mut().poll_write(cx, buffer)) {
                Ok(x) => x,
                Err(e) => {
                    dir.log_write_fail(e);
                    return Poll::Ready(());
                }
            };
            if i == 0 {
                dir.log_write_zero();
                return Poll::Ready(());
            }
            from.as_mut().consume(i);
        }
    })
    .await
}

fn print_prelude_info(
    message: String,
    status: CompatibilityState,
    platform_abi: AbiRevision,
    overnet_id: u64,
) -> Result<()> {
    let ssh_connection = std::env::var("SSH_CONNECTION")?;
    let info = ConnectionInfo {
        ssh_connection,
        connect_info: compat_info::DeviceConnectionInfo {
            status,
            platform_abi: platform_abi.as_u64(),
            message,
            overnet_id: Some(overnet_id),
        },
    };

    let encoded_message = serde_json::to_string(&info)?;
    // println!() can panic when it fails to write, which is undesirable.
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{encoded_message}")?;
    Ok(())
}

/// Utility to bridge an overnet/RCS connection via SSH. If you're running this manually, you are
/// probably doing something wrong.
#[derive(FromArgs)]
struct Args {
    /// use circuit-switched connection
    #[argh(switch)]
    circuit: bool,

    /// flag indicating compatibility checking should be performed.
    /// This also has the side effect of returning the $SSH_CONNECTION value in the response.
    /// This is done to keep the remote side response parsing simple and backwards compatible.
    #[argh(option)]
    abi_revision: Option<u64>,

    /// ID number. RCS will reproduce this number once you connect to it. This allows us to
    /// associate an Overnet connection with an RCS connection, in spite of the fuzziness of
    /// Overnet's mesh.
    #[argh(positional)]
    id: Option<u64>,

    /// log ID number from ffx to bookend connection logs.
    #[argh(option)]
    log_id: Option<String>,
}

#[fuchsia::main(logging_tags = ["remote_control_runner"])]
async fn main() -> Result<()> {
    let args: Args = argh::from_env();
    // Perform the compatibility checking between the caller (an ffx command)
    // and the platform (this program).
    let rcs_proxy = connect_to_protocol::<ConnectorMarker>()?;
    let (local_socket, remote_socket) = fidl::Socket::create_stream();

    let overnet_id = if args.circuit {
        rcs_proxy.establish_circuit(args.id.unwrap_or(0), remote_socket).await?
    } else {
        return Err(anyhow::format_err!("Legacy overnet is no longer supported."));
    };

    if let Some(abi) = args.abi_revision {
        let host_overnet_revision = AbiRevision::from_u64(abi);
        let platform_abi = HISTORY.get_misleading_version_for_ffx().abi_revision;
        let status: CompatibilityState;
        let message = match HISTORY.check_abi_revision_for_runtime(host_overnet_revision) {
            Ok(_) => {
                log::debug!("Host overnet is running supported revision: {host_overnet_revision}");
                status = CompatibilityState::Supported;
                "Host overnet is running supported revision".to_string()
            }
            Err(e) => {
                status = e.clone().into();
                let warning = format!("abi revision {host_overnet_revision} not supported: {e}");
                log::warn!("{warning}");
                warning
            }
        };
        print_prelude_info(message, status, platform_abi, overnet_id)?;
    } else {
        // This is the legacy caller that does not support compatibility checking. Do not write anything
        // to stdout.
        // messages to stderr will cause the pipe to close as well.
        log::warn!("--abi-revision not present. Compatibility checks are disabled.");
    }

    let _log_guard = FfxLogGuard::new(&args.log_id);

    let local_socket = fidl::AsyncSocket::from_socket(local_socket);
    let (mut rx_socket, mut tx_socket) = futures::AsyncReadExt::split(local_socket);

    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();

    // SAFETY: In order to remove the overhead of FDIO, we want to extract out the underlying
    // handles of STDIN and STDOUT and forward them to our sockets. That requires us to transfer
    // the sockets out of fdio, but unfortunately Rust doesn't allow us to take ownership from
    // `std::io::stdin()` and `std::io::stdout()`. To work around that, we grab the STDIN and
    // STDOUT locks to prevent any other thread from accessing them while we're streaming traffic.
    let (stdin_fd, stdout_fd) = unsafe {
        (OwnedFd::from_raw_fd(stdin.as_raw_fd()), OwnedFd::from_raw_fd(stdout.as_raw_fd()))
    };

    let stdin = fdio::transfer_fd(stdin_fd)?;
    let stdout = fdio::transfer_fd(stdout_fd)?;

    let mut stdin = fidl::AsyncSocket::from_socket(fidl::Socket::from(stdin));
    let mut stdout = fidl::AsyncSocket::from_socket(fidl::Socket::from(stdout));

    let in_fut = buffered_copy(&mut stdin, &mut tx_socket, CopyDirection::StdIn);
    let out_fut = buffered_copy(&mut rx_socket, &mut stdout, CopyDirection::StdOut);
    select(pin!(in_fut), pin!(out_fut)).await;
    Ok(())
}

#[cfg(test)]
mod test {

    use super::*;
    use anyhow::Error;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_developer_remotecontrol::{
        IdentifyHostResponse, RemoteControlMarker, RemoteControlProxy, RemoteControlRequest,
    };
    use fidl_fuchsia_developer_remotecontrol_connector::{ConnectorProxy, ConnectorRequest};
    use fuchsia_async as fasync;
    use std::cell::RefCell;
    use std::rc::Rc;

    async fn send_request(proxy: &RemoteControlProxy) -> Result<()> {
        // We just need to make a request to the RCS - it doesn't really matter
        // what we choose here so long as there are no side effects.
        let _ = proxy.identify_host().await?;
        Ok(())
    }

    fn setup_fake_rcs(handle_stream: bool) -> (RemoteControlProxy, ConnectorProxy) {
        let (proxy, mut stream) = create_proxy_and_stream::<RemoteControlMarker>();
        let (runner_proxy, mut runner_stream) = create_proxy_and_stream::<ConnectorMarker>();

        if !handle_stream {
            return (proxy, runner_proxy);
        }
        let last_id = Rc::new(RefCell::new(0));

        let last_id_clone = Rc::clone(&last_id);
        fasync::Task::local(async move {
            while let Ok(req) = stream.try_next().await {
                match req {
                    Some(RemoteControlRequest::IdentifyHost { responder }) => {
                        let _ = responder
                            .send(Ok(&IdentifyHostResponse {
                                nodename: Some("".to_string()),
                                addresses: Some(vec![]),
                                ids: Some(vec![last_id_clone.borrow().clone()]),
                                ..Default::default()
                            }))
                            .unwrap();
                    }
                    _ => assert!(false),
                }
            }
        })
        .detach();

        fasync::Task::local(async move {
            while let Ok(Some(ConnectorRequest::EstablishCircuit { id, socket: _, responder })) =
                runner_stream.try_next().await
            {
                last_id.replace(id);
                responder.send(0u64).unwrap();
            }
        })
        .detach();

        (proxy, runner_proxy)
    }

    #[fuchsia::test]
    async fn test_handles_successful_response() -> Result<(), Error> {
        let (rcs_proxy, _runner_proxy) = setup_fake_rcs(true);
        assert!(send_request(&rcs_proxy).await.is_ok());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_handles_failed_response() -> Result<(), Error> {
        let (rcs_proxy, _runner_proxy) = setup_fake_rcs(false);
        assert!(send_request(&rcs_proxy).await.is_err());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_sends_id_if_given() -> Result<(), Error> {
        let (rcs_proxy, runner_proxy) = setup_fake_rcs(true);
        let (pumpkin, _) = fidl::Socket::create_stream();
        runner_proxy.establish_circuit(34u64, pumpkin).await?;
        let ident = rcs_proxy.identify_host().await?.unwrap();
        assert_eq!(34u64, ident.ids.unwrap()[0]);
        Ok(())
    }
}
