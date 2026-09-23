// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use fidl::endpoints;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_testing_harness::{OperationError, RealmProxy_Request, RealmProxy_RequestStream};
use fuchsia_async as fasync;
use fuchsia_component::runtime::ConnectorReceiver;
use fuchsia_component_test::RealmInstance;
use futures::{Future, StreamExt};
use log::{debug, error, warn};

// RealmProxy mediates a test suite's access to the services in a test realm.
pub trait RealmProxy {
    // Connects to the named service in this proxy's realm.
    //
    // If the connection fails, the resulting [OperationError] is determined
    // by the [RealmProxy] implementation.
    fn connect_to_named_protocol(
        &mut self,
        protocol: &str,
        server_end: zx::Channel,
    ) -> Result<(), OperationError>;

    // Opens the service directory in this proxy's realm.
    //
    // If the connection fails, the resulting [OperationError] is determined
    // by the [RealmProxy] implementation.
    fn open_service(&self, service: &str, server_end: zx::Channel) -> Result<(), OperationError>;

    // Connects to the service instance in this proxy's realm.
    //
    // If the connection fails, the resulting [OperationError] is determined
    // by the [RealmProxy] implementation.
    fn connect_to_service_instance(
        &self,
        service: &str,
        instance: &str,
        server_end: zx::Channel,
    ) -> Result<(), OperationError>;
}

// A default [RealmProxy] implementation that mediates access to a specific [RealmInstance].
struct RealmInstanceProxy(RealmInstance);

impl RealmProxy for RealmInstanceProxy {
    fn connect_to_named_protocol(
        &mut self,
        protocol: &str,
        server_end: zx::Channel,
    ) -> Result<(), OperationError> {
        let res =
            self.0.root.connect_request_to_named_protocol_at_exposed_dir(protocol, server_end);

        if let Some(err) = res.err() {
            error!("{err:?}");
            return Err(OperationError::Failed);
        }

        Ok(())
    }

    fn open_service(&self, service: &str, server_end: zx::Channel) -> Result<(), OperationError> {
        self.0
            .root
            .get_exposed_dir()
            .open(
                service,
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                &Default::default(),
                server_end,
            )
            .map_err(|e| {
                warn!("Failed to open service directory for {service}: {e:?}");
                OperationError::Failed
            })
    }

    fn connect_to_service_instance(
        &self,
        service: &str,
        instance: &str,
        server_end: zx::Channel,
    ) -> Result<(), OperationError> {
        self.0
            .root
            .get_exposed_dir()
            .open(
                format!("{service}/{instance}").as_str(),
                fio::PERM_READABLE | fio::Flags::PROTOCOL_DIRECTORY,
                &Default::default(),
                server_end,
            )
            .map_err(|e| {
                warn!("Failed to open service instance directory for {service}/{instance}: {e:?}");
                OperationError::Failed
            })
    }
}

// serve_with_proxy uses [proxy] to handle all requests from [stream].
//
// This function is useful when implementing a custom test harness that serves
// other protocols in addition to the RealmProxy protocol.
//
// # Example Usage
//
// ```
// #[fuchsia::main(logging = true)]
// async fn main() -> Result<(), Error> {
//   let mut fs = ServiceFs::new();
//
//   fs.dir("svc").add_fidl_service(|stream| {
//     fasync::Task::spawn(async move {
//       let realm = build_realm().await.unwrap();
//       let realm_proxy = MyCustomRealmProxy(realm);
//       realm_proxy::service::serve_with_proxy(realm_proxy, stream).await.unwrap();
//     }).detach();
//   });
//
//   fs.take_and_serve_directory_handle()?;
//   fs.collect::<()>().await;
//   Ok(())
// }
// ```
pub async fn serve_with_proxy<P: RealmProxy>(
    mut proxy: P,
    mut stream: RealmProxy_RequestStream,
) -> Result<(), crate::Error> {
    while let Some(option) = stream.next().await {
        match option {
            Ok(request) => match request {
                RealmProxy_Request::ConnectToNamedProtocol {
                    protocol,
                    server_end,
                    responder,
                    ..
                } => {
                    let res = proxy.connect_to_named_protocol(protocol.as_str(), server_end);
                    responder.send(res)?;
                }
                RealmProxy_Request::OpenService { service, server_end, responder } => {
                    let res = proxy.open_service(service.as_str(), server_end);
                    responder.send(res)?;
                }
                RealmProxy_Request::ConnectToServiceInstance {
                    service,
                    instance,
                    server_end,
                    responder,
                } => {
                    let res = proxy.connect_to_service_instance(
                        service.as_str(),
                        instance.as_str(),
                        server_end,
                    );
                    responder.send(res)?;
                }
            },
            // Tell the user if we failed to read from the channel. These errors occur during
            // testing and ignoring them can make it difficult to root cause test failures.
            Err(e) => return Err(crate::error::Error::Fidl(e)),
        }
    }

    // Tell the user we're disconnecting in case this is a premature shutdown.
    debug!("done serving the RealmProxy connection");
    Ok(())
}

// serve proxies all requests in [stream] to [realm].
//
// # Example Usage
//
// ```
// #[fuchsia::main(logging = true)]
// async fn main() -> Result<(), Error> {
//   let mut fs = ServiceFs::new();
//
//   fs.dir("svc").add_fidl_service(|stream| {
//     fasync::Task::spawn(async move {
//       let realm = build_realm().await.unwrap();
//       realm_proxy::service::serve(realm, stream).await.unwrap();
//     }).detach();
//   });
//
//   fs.take_and_serve_directory_handle()?;
//   fs.collect::<()>().await;
//   Ok(())
// }
// ```
pub async fn serve(realm: RealmInstance, stream: RealmProxy_RequestStream) -> Result<(), Error> {
    let proxy = RealmInstanceProxy(realm);
    serve_with_proxy(proxy, stream).await?;
    Ok(())
}

/// Dispatches incoming connections on `receiver to `request_stream_handler`.
/// `receiver` is a component framework runtime receiver channel.
///
/// Example:
///
/// async fn handle_echo_request_stream(mut stream: fecho::EchoRequestStream) {
///     while let Ok(Some(_request)) = stream.try_next().await {
///         // ... handle request ...
///     }
/// }
/// ...
/// task_group.spawn(async move {
///     let _ = realm_proxy::service::handle_receiver::<fecho::EchoMarker, _, _>(
///         echo_receiver,
///         handle_echo_request_stream,
///     )
///     .await
///     .map_err(|e| {
///         error!("Failed to serve echo stream: {}", e);
///     });
/// });
pub async fn handle_receiver<T, Fut, F>(
    mut receiver: ConnectorReceiver,
    request_stream_handler: F,
) -> Result<(), Error>
where
    T: endpoints::ProtocolMarker,
    Fut: Future<Output = ()> + Send,
    F: Fn(T::RequestStream) -> Fut + Send + Sync + Copy + 'static,
{
    let scope = fasync::Scope::new();
    while let Some(channel) = receiver.next().await {
        scope.spawn(async move {
            let server_end = endpoints::ServerEnd::<T>::new(channel.into());
            let stream: T::RequestStream = server_end.into_stream();
            request_stream_handler(stream).await;
        });
    }
    scope.join().await;
    Ok(())
}
