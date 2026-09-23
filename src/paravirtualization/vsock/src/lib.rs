// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]

mod addr;
mod port;
mod service;

pub use self::service::Vsock;

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{self, Proxy};
    use fidl_fuchsia_hardware_vsock::{
        CallbacksProxy, DeviceMarker, DeviceRequest, DeviceRequestStream, VMADDR_CID_HOST,
        VMADDR_CID_LOCAL,
    };
    use fidl_fuchsia_vsock::{
        AcceptorMarker, AcceptorRequest, ConnectionMarker, ConnectionProxy, ConnectionTransport,
        ConnectorMarker, ConnectorProxy, ListenerMarker,
    };
    use fuchsia_async as fasync;
    use futures::{FutureExt, StreamExt, TryFutureExt, channel, future};
    struct MockDriver {
        client: DeviceRequestStream,
        callbacks: CallbacksProxy,
    }

    impl MockDriver {
        fn new(client: DeviceRequestStream, callbacks: CallbacksProxy) -> Self {
            MockDriver { client, callbacks }
        }
    }

    macro_rules! unwrap_msg {
        ($msg:path{$($bindings:tt)*} from $stream:expr) => {
            if let Some(Ok($msg{$($bindings)*})) = $stream.next().await {
                ($($bindings)*)
            } else {
                panic!("Expected msg {}", stringify!($msg));
            }
        }
    }

    async fn common_setup() -> Result<(MockDriver, Vsock), anyhow::Error> {
        let (driver_client, driver_server) = endpoints::create_endpoints::<DeviceMarker>();
        let mut driver_server = driver_server.into_stream();

        // Vsock::new expects to be able to communication with a running driver instance.
        // As we don't have a driver instance we spin up an asynchronous thread that will
        // momentarily pretend to be the driver to receive the callbacks, and then send
        // those callbacks over the below oneshot channel that we can then receive after
        // Vsock::new completes.
        let (tx, rx) = channel::oneshot::channel();
        fasync::Task::spawn(async move {
            let (cb, responder) =
                unwrap_msg!(DeviceRequest::Start{cb, responder} from driver_server);
            let driver_callbacks = cb.into_proxy();
            responder.send(Ok(())).unwrap();
            let _ = tx.send((driver_server, driver_callbacks));
        })
        .detach();

        let (service, event_loop) = Vsock::new(None, Some(driver_client.into_proxy())).await?;
        fasync::Task::local(event_loop.map_err(|x| panic!("Event loop stopped {}", x)).map(|_| ()))
            .detach();
        let (driver_server, driver_callbacks) = rx.await?;
        let driver = MockDriver::new(driver_server, driver_callbacks);
        Ok((driver, service))
    }

    fn make_con() -> Result<(zx::Socket, ConnectionProxy, ConnectionTransport), anyhow::Error> {
        let (client_socket, server_socket) = zx::Socket::create_stream();
        let (client_end, server_end) = endpoints::create_endpoints::<ConnectionMarker>();
        let client_end = client_end.into_proxy();
        let con = ConnectionTransport { data: server_socket, con: server_end };
        Ok((client_socket, client_end, con))
    }

    fn make_client(service: &Vsock) -> Result<ConnectorProxy, anyhow::Error> {
        let (app_client, app_remote) = endpoints::create_endpoints::<ConnectorMarker>();
        let app_client = app_client.into_proxy();
        // Run the client
        fasync::Task::local(
            Vsock::run_client_connection(service.clone(), app_remote.into_stream())
                .then(|_| future::ready(())),
        )
        .detach();
        Ok(app_client)
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn basic_listen() -> Result<(), anyhow::Error> {
        let (mut driver, service) = common_setup().await?;

        let app_client = make_client(&service)?;

        // Should reject listening at the ephemeral port ranges.
        {
            let (acceptor_remote, _acceptor_client) =
                endpoints::create_endpoints::<AcceptorMarker>();
            assert_eq!(
                app_client.listen(49152, acceptor_remote).await?,
                Err(zx::sys::ZX_ERR_UNAVAILABLE)
            );
        }

        // Listen on a reasonable value.
        let (acceptor_remote, acceptor_client) = endpoints::create_endpoints::<AcceptorMarker>();
        assert_eq!(app_client.listen(8000, acceptor_remote).await?, Ok(()));
        let mut acceptor_client = acceptor_client.into_stream();

        // Validate that we cannot listen twice
        {
            let (acceptor_remote, _acceptor_client) =
                endpoints::create_endpoints::<AcceptorMarker>();
            assert_eq!(
                app_client.listen(8000, acceptor_remote).await?,
                Err(zx::sys::ZX_ERR_ALREADY_BOUND)
            );
        }

        // Create a connection from the driver
        driver.callbacks.request(&*addr::Vsock::new(8000, 80, VMADDR_CID_HOST))?;
        let (_data_socket, _client_end, con) = make_con()?;

        let (_, responder) =
            unwrap_msg!(AcceptorRequest::Accept{addr, responder} from acceptor_client);
        responder.send(Some(con))?;

        // expect a response
        let (_, _server_data_socket, responder) =
            unwrap_msg!(DeviceRequest::SendResponse{addr, data, responder} from driver.client);
        responder.send(Ok(()))?;

        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn basic_bind_and_listen() -> Result<(), anyhow::Error> {
        let (mut driver, service) = common_setup().await?;
        let app_client = make_client(&service)?;
        let (_, listener_remote) = endpoints::create_endpoints::<ListenerMarker>();

        // Should reject listening at the ephemeral port ranges.
        assert_eq!(
            app_client.bind(VMADDR_CID_LOCAL, 49152, listener_remote).await?,
            Err(zx::sys::ZX_ERR_UNAVAILABLE)
        );

        let (listener_client, listener_remote) = endpoints::create_endpoints::<ListenerMarker>();
        // Listen on a reasonable value.
        assert_eq!(app_client.bind(VMADDR_CID_LOCAL, 8000, listener_remote).await?, Ok(()));

        // Validate that we cannot listen twice
        let (_, listener_remote2) = endpoints::create_endpoints::<ListenerMarker>();
        assert_eq!(
            app_client.bind(VMADDR_CID_LOCAL, 8000, listener_remote2).await?,
            Err(zx::sys::ZX_ERR_ALREADY_BOUND)
        );

        let listener_client = listener_client.into_proxy();
        assert_eq!(listener_client.listen(1).await?, Ok(()));

        // Create a connection from the driver
        driver.callbacks.request(&*addr::Vsock::new(8000, 80, VMADDR_CID_LOCAL))?;
        let (_data_socket, _client_end, con) = make_con()?;

        let accept_fut = listener_client.accept(con);

        // expect a response
        let (_, _server_data_socket, responder) =
            unwrap_msg!(DeviceRequest::SendResponse{addr, data, responder} from driver.client);
        responder.send(Ok(()))?;

        assert_eq!(accept_fut.await?, Ok(*addr::Vsock::new(8000, 80, VMADDR_CID_LOCAL)));

        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn reject_connection() -> Result<(), anyhow::Error> {
        let (mut driver, service) = common_setup().await?;

        let app_client = make_client(&service)?;

        // Send a connection request
        let (_data_socket, _client_end, con) = make_con()?;
        let request = app_client.connect(VMADDR_CID_HOST, 8000, con);

        // Expect a driver message
        {
            let (addr, _server_data_socket, responder) =
                unwrap_msg!(DeviceRequest::SendRequest{addr, data, responder} from driver.client);
            responder.send(Ok(()))?;
            // Now simulate an incoming RST for a rejected connection
            driver.callbacks.rst(&addr)?;
            // Leave this scope to drop the server_data_socket
        }
        // Request should resolve to an error
        assert_eq!(request.await?, Err(zx::sys::ZX_ERR_UNAVAILABLE));
        Ok(())
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn transport_reset() -> Result<(), anyhow::Error> {
        let (mut driver, service) = common_setup().await?;

        let app_client = make_client(&service)?;

        // Create a connection.
        let (_data_socket_request, client_end_request, con) = make_con()?;
        let request = app_client.connect(VMADDR_CID_HOST, 8000, con);
        let (addr, server_data_socket_request, responder) =
            unwrap_msg!(DeviceRequest::SendRequest{addr, data, responder} from driver.client);
        responder.send(Ok(()))?;
        driver.callbacks.response(&addr)?;
        let _ = request.await?.map_err(zx::Status::err_from_raw)?;

        // Start a listener
        let (acceptor_remote, acceptor_client) = endpoints::create_endpoints::<AcceptorMarker>();
        assert_eq!(app_client.listen(9000, acceptor_remote).await?, Ok(()));
        let mut acceptor_client = acceptor_client.into_stream();

        // Perform a transport reset
        drop(server_data_socket_request);
        driver.callbacks.transport_reset(7).await?;

        // Connection should be closed
        client_end_request.on_closed().await?;

        // Listener should still be active and receive a connection
        driver.callbacks.request(&*addr::Vsock::new(9000, 80, VMADDR_CID_HOST))?;
        let (_data_socket, _client_end, _con) = make_con()?;

        let (_addr, responder) =
            unwrap_msg!(AcceptorRequest::Accept{addr, responder} from acceptor_client);
        drop(responder); // don't bother responding, we're done

        Ok(())
    }
}
