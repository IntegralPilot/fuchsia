// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::platform::{GuestConsole, PlatformServices};
use fidl_fuchsia_virtualization::{
    GuestManagerProxy, GuestMarker, GuestStatus, HostVsockAcceptorMarker, HostVsockEndpointMarker,
    HostVsockEndpointProxy,
};
use futures::TryStreamExt;
use guest_cli_args as arguments;
use std::fmt;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SocatResult {
    SocatSuccess(SocatSuccess),
    SocatError(SocatError),
}

impl fmt::Display for SocatResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SocatResult::SocatSuccess(v) => write!(f, "{}", v),
            SocatResult::SocatError(v) => write!(f, "{}", v),
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SocatSuccess {
    Connected,
    Listened(u32),
}

impl fmt::Display for SocatSuccess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SocatSuccess::Connected => write!(f, "Disconnected after a successful connect"),
            SocatSuccess::Listened(port) => write!(f, "Stopped listening on port {}", port),
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SocatError {
    NotRunning,
    NoVsockDevice,
    NoListener(u32),
    FailedToListen(u32),
    InternalFailure(String),
}

impl fmt::Display for SocatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SocatError::NotRunning => write!(f, "Can't attach to a non-running guest"),
            SocatError::NoListener(port) => write!(f, "No listener on port {}", port),
            SocatError::FailedToListen(port) => write!(f, "Already a listener on port {}", port),
            SocatError::InternalFailure(err) => write!(f, "Internal error: {}", err),
            SocatError::NoVsockDevice => write!(f, "Guest lacks the required vsock device"),
        }
    }
}

impl std::convert::From<fidl::Status> for SocatError {
    fn from(err: fidl::Status) -> SocatError {
        SocatError::InternalFailure(format!("FIDL status - {}", err))
    }
}

impl std::convert::From<fidl::Error> for SocatError {
    fn from(err: fidl::Error) -> SocatError {
        SocatError::InternalFailure(format!("FIDL error - {}", err))
    }
}

fn duplicate_socket(_socket: fidl::Socket) -> Result<(fidl::Socket, fidl::Socket), SocatError> {
    #[cfg(target_os = "fuchsia")]
    {
        let other = _socket.duplicate_handle(fidl::Rights::SAME_RIGHTS)?;
        Ok((_socket, other))
    }

    // TODO(https://fxbug.dev/42068091): Remove when overnet supports duplicated socket handles.
    #[cfg(not(target_os = "fuchsia"))]
    unimplemented!()
}

async fn handle_socat_listen(
    vsock_endpoint: HostVsockEndpointProxy,
    host_port: u32,
) -> Result<SocatSuccess, SocatError> {
    let (vsock_accept_client, mut vsock_acceptor_stream) =
        fidl::endpoints::create_request_stream::<HostVsockAcceptorMarker>();

    vsock_endpoint
        .listen(host_port, vsock_accept_client)
        .await?
        .map_err(|_| SocatError::FailedToListen(host_port))?;

    // Try_next returns a Result<Option<T>, Err>, hence we need to unwrap our value
    let connection = vsock_acceptor_stream
        .try_next()
        .await?
        .ok_or_else(|| SocatError::InternalFailure("unexpected end of stream".to_string()))?;

    let (_src_cid, _src_port, port, responder) = connection
        .into_accept()
        .ok_or_else(|| SocatError::InternalFailure("unexpected message on stream".to_string()))?;

    if port != host_port {
        responder.send(Err(zx_status::Status::CONNECTION_REFUSED.into_raw()))?;
        return Err(SocatError::InternalFailure(
            "connection attempt on unexpected port".to_string(),
        ));
    }

    let (socket, remote_socket) = fidl::Socket::create_stream();
    responder.send(Ok(remote_socket))?;

    let (input, output) = duplicate_socket(socket)?;

    let console = GuestConsole::new(input, output)
        .map_err(|err| SocatError::InternalFailure(format!("failed to create console: {}", err)))?;
    if let Err(err) = console.run_with_stdio().await {
        Err(SocatError::InternalFailure(format!("failed to run console: {}", err)))
    } else {
        Ok(SocatSuccess::Listened(host_port))
    }
}

async fn handle_socat_connect(
    vsock_endpoint: HostVsockEndpointProxy,
    port: u32,
) -> Result<SocatSuccess, SocatError> {
    let Ok(socket) = vsock_endpoint.connect(port).await? else {
        return Err(SocatError::NoListener(port));
    };

    let (input, output) = duplicate_socket(socket)?;
    let console = GuestConsole::new(input, output)
        .map_err(|err| SocatError::InternalFailure(format!("failed to create console: {}", err)))?;

    if let Err(err) = console.run_with_stdio().await {
        Err(SocatError::InternalFailure(format!("failed to run console: {}", err)))
    } else {
        Ok(SocatSuccess::Connected)
    }
}

async fn connect_to_vsock_endpoint(
    manager: GuestManagerProxy,
) -> Result<HostVsockEndpointProxy, SocatError> {
    let (guest_endpoint, guest_server_end) = fidl::endpoints::create_proxy::<GuestMarker>();
    manager
        .connect(guest_server_end)
        .await?
        .map_err(|err| SocatError::InternalFailure(format!("failed to connect: {:?}", err)))?;

    let (vsock_endpoint, vsock_server_end) =
        fidl::endpoints::create_proxy::<HostVsockEndpointMarker>();

    guest_endpoint
        .get_host_vsock_endpoint(vsock_server_end)
        .await?
        .map_err(|_| SocatError::NoVsockDevice)?;

    Ok(vsock_endpoint)
}

async fn get_manager<P: PlatformServices>(
    services: &P,
    guest_type: arguments::GuestType,
) -> Result<GuestManagerProxy, SocatError> {
    if guest_type == arguments::GuestType::Zircon {
        // We don't have a Zircon vsock device.
        return Err(SocatError::NoVsockDevice);
    }

    let guest_manager = services.connect_to_manager(guest_type).await.map_err(|err| {
        SocatError::InternalFailure(format!("failed to connect to manager: {}", err))
    })?;

    let guest_info = guest_manager.get_info().await?;
    let status = guest_info.guest_status.expect("guest status should always be set");
    if status != GuestStatus::Starting && status != GuestStatus::Running {
        return Err(SocatError::NotRunning);
    }

    Ok(guest_manager)
}

async fn handle_impl<P: PlatformServices>(
    services: &P,
    args: &arguments::socat_args::SocatArgs,
) -> Result<SocatSuccess, SocatError> {
    match &args.socat_cmd {
        arguments::socat_args::SocatCommands::Listen(args) => {
            let manager = get_manager(services, args.guest_type).await?;
            let endpoint = connect_to_vsock_endpoint(manager).await?;
            handle_socat_listen(endpoint, args.host_port).await
        }
        arguments::socat_args::SocatCommands::Connect(args) => {
            let manager = get_manager(services, args.guest_type).await?;
            let endpoint = connect_to_vsock_endpoint(manager).await?;
            handle_socat_connect(endpoint, args.guest_port).await
        }
    }
}

pub async fn handle_socat<P: PlatformServices>(
    services: &P,
    args: &arguments::socat_args::SocatArgs,
) -> SocatResult {
    match handle_impl(services, args).await {
        Err(err) => SocatResult::SocatError(err),
        Ok(ok) => SocatResult::SocatSuccess(ok),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl::endpoints::create_proxy_and_stream;
    use futures::StreamExt;
    use futures::future::join;

    #[fuchsia::test(allow_stalls = false)]
    async fn socat_listen_invalid_host_returns_err() {
        let (proxy, mut stream) = create_proxy_and_stream::<HostVsockEndpointMarker>();
        let server = async move {
            let (port, _acceptor, responder) = stream
                .next()
                .await
                .expect("Failed to read from stream")
                .expect("Failed to parse request")
                .into_listen()
                .expect("Unexpected call to Guest Proxy");
            assert_eq!(port, 0);
            responder
                .send(Err(zx_status::Status::CONNECTION_REFUSED.into_raw()))
                .expect("Failed to send status code to client");
        };

        let client = handle_socat_listen(proxy, 0);
        let (_server_res, client_res) = join(server, client).await;
        assert_eq!(client_res.unwrap_err().to_string(), "Already a listener on port 0");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn socat_listen_mismatched_ports_returns_err() {
        let (proxy, mut stream) = create_proxy_and_stream::<HostVsockEndpointMarker>();
        let server = async move {
            let (port, acceptor, responder) = stream
                .next()
                .await
                .expect("Failed to read from stream")
                .expect("Failed to parse request")
                .into_listen()
                .expect("Unexpected call to Guest Proxy");
            assert_eq!(port, 0);
            responder.send(Ok(())).expect("Failed to send status code to client");
            let _ = acceptor.into_proxy().accept(0, 0, 1).await.expect("Failed to accept listener");
        };

        let client = handle_socat_listen(proxy, 0);
        let (_server_res, client_res) = join(server, client).await;
        assert_eq!(
            client_res.unwrap_err().to_string(),
            "Internal error: connection attempt on unexpected port"
        );
    }
}
