// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This library provides a runtime harness for the TRF (Test Realm Factory) codegen tool.
//! It offers utilities for interacting with test realms, connecting to mock control
//! proxies, and managing component lifecycle events in test environments.

use anyhow::Context as _;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_testing_harness::RealmProxy_Proxy;
use fidl_fuchsia_trf_factory::{ConfigOverride, CreateRealmRequest, FactoryMarker};
use futures::channel::mpsc;
use futures::stream::Stream;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};

/// Trait implemented by mock control markers to connect to mock control proxies.
pub trait MockControlMarker {
    /// The control proxy type associated with this mock.
    type ControlProxy;

    /// Connects to the mock control proxy from the incoming namespace.
    fn connect_from_namespace() -> Result<Self::ControlProxy, anyhow::Error>;
}

impl<P> MockControlMarker for P
where
    P: DiscoverableProtocolMarker,
{
    type ControlProxy = P::Proxy;

    fn connect_from_namespace() -> Result<Self::ControlProxy, anyhow::Error> {
        fuchsia_component::client::connect_to_protocol::<P>()
            .context("failed to connect to mock control proxy from namespace")
    }
}

/// Represents component lifecycle events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleEvent {
    Started(String),
    Stopped(String),
}

/// Stream of component lifecycle events.
pub struct LifecycleStream {
    receiver: mpsc::UnboundedReceiver<LifecycleEvent>,
}

impl LifecycleStream {
    /// Creates a new `LifecycleStream` backed by an unbounded receiver.
    pub fn new(receiver: mpsc::UnboundedReceiver<LifecycleEvent>) -> Self {
        Self { receiver }
    }

    /// Asynchronously receives the next lifecycle event.
    pub async fn next_event(&mut self) -> Option<LifecycleEvent> {
        use futures::StreamExt as _;
        self.receiver.next().await
    }
}

impl Stream for LifecycleStream {
    type Item = LifecycleEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_next(cx)
    }
}

/// Runtime harness wrapper providing helper operations for test realms.
pub struct TestRealm {
    realm_proxy: Option<RealmProxy_Proxy>,
    lifecycle_senders: Mutex<Vec<mpsc::UnboundedSender<LifecycleEvent>>>,
}

impl Default for TestRealm {
    fn default() -> Self {
        Self { realm_proxy: None, lifecycle_senders: Mutex::new(Vec::new()) }
    }
}

impl TestRealm {
    /// Constructs a new `TestRealm` using a newly created realm factory instance.
    pub async fn create(overrides: Vec<ConfigOverride>) -> Result<Self, anyhow::Error> {
        let factory = fuchsia_component::client::connect_to_protocol::<FactoryMarker>()
            .context("failed to connect to Factory")?;
        let proxy_client_end = factory
            .create_realm(CreateRealmRequest { overrides: Some(overrides), ..Default::default() })
            .await?
            .map_err(|e| anyhow::format_err!("Factory error: {:?}", e))?;
        let proxy = proxy_client_end.into_proxy();
        Ok(Self { realm_proxy: Some(proxy), lifecycle_senders: Mutex::new(Vec::new()) })
    }

    /// Connects to a FIDL protocol exposed by the component under test in the test realm.
    pub async fn connect_to_protocol<P: DiscoverableProtocolMarker>(
        &self,
    ) -> Result<P::Proxy, anyhow::Error> {
        if let Some(proxy) = &self.realm_proxy {
            let (client_end, server_end) = fidl::endpoints::create_proxy::<P>();
            proxy
                .connect_to_named_protocol(P::PROTOCOL_NAME, server_end.into_channel())
                .await
                .context("failed to send connect_to_named_protocol request")?
                .map_err(|e| anyhow::anyhow!("OperationError: {:?}", e))?;
            Ok(client_end)
        } else {
            fuchsia_component::client::connect_to_protocol::<P>()
                .context("failed to connect to protocol from incoming namespace")
        }
    }

    /// Connects to a mock control proxy.
    pub fn get_control<M: MockControlMarker>(&self) -> Result<M::ControlProxy, anyhow::Error> {
        M::connect_from_namespace()
    }

    /// Opens an isolated directory at the given path within the exposed directory of the realm.
    pub async fn get_directory(&self, path: &str) -> Result<fio::DirectoryProxy, anyhow::Error> {
        let canonical_path = fuchsia_fs::canonicalize_path(path);
        if let Some(proxy) = &self.realm_proxy {
            let (client_end, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
            proxy
                .open_service(canonical_path, server_end.into_channel())
                .await
                .context("failed to send open_service request")?
                .map_err(|e| anyhow::anyhow!("OperationError: {:?}", e))?;
            Ok(client_end)
        } else {
            let directory_proxy = fuchsia_fs::directory::open_in_namespace(
                canonical_path,
                fuchsia_fs::PERM_READABLE | fuchsia_fs::PERM_WRITABLE,
            )
            .context("failed to open directory in namespace")?;
            Ok(directory_proxy)
        }
    }

    /// Subscribes to lifecycle events emitted by the test environment.
    pub fn subscribe_to_lifecycle(&self) -> Result<LifecycleStream, anyhow::Error> {
        let (sender, receiver) = mpsc::unbounded();
        self.lifecycle_senders.lock().unwrap().push(sender);
        Ok(LifecycleStream::new(receiver))
    }

    /// Emits a lifecycle event to all subscribed lifecycle streams.
    pub fn emit_lifecycle_event(&self, event: LifecycleEvent) {
        let mut senders = self.lifecycle_senders.lock().unwrap();
        senders.retain(|sender| sender.unbounded_send(event.clone()).is_ok());
    }

    pub async fn stop_component(&self, name: &str) -> Result<(), anyhow::Error> {
        let lifecycle =
            self.connect_to_protocol::<fidl_fuchsia_sys2::LifecycleControllerMarker>().await?;
        lifecycle
            .stop_instance(&format!("./{}", name))
            .await?
            .map_err(|e| anyhow::format_err!("{:?}", e))?;
        self.emit_lifecycle_event(LifecycleEvent::Stopped(name.to_string()));
        Ok(())
    }

    pub async fn is_running(&self, name: &str) -> Result<bool, anyhow::Error> {
        let query = self.connect_to_protocol::<fidl_fuchsia_sys2::RealmQueryMarker>().await?;
        let info = query
            .get_instance(&format!("./{}", name))
            .await?
            .map_err(|e| anyhow::format_err!("{:?}", e))?;
        if let Some(resolved) = info.resolved_info {
            Ok(resolved.execution_info.is_some())
        } else {
            Ok(false)
        }
    }

    pub async fn start_component(&self, name: &str) -> Result<(), anyhow::Error> {
        let lifecycle =
            self.connect_to_protocol::<fidl_fuchsia_sys2::LifecycleControllerMarker>().await?;
        let (_, binder) = fidl::endpoints::create_endpoints();
        lifecycle
            .start_instance(&format!("./{}", name), binder)
            .await?
            .map_err(|e| anyhow::format_err!("{:?}", e))?;
        self.emit_lifecycle_event(LifecycleEvent::Started(name.to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{ServerEnd, create_proxy_and_stream};
    use fidl_fuchsia_sys2 as fsys2;
    use fidl_fuchsia_testing_harness::{RealmProxy_Marker, RealmProxy_Request};
    use fidl_fuchsia_trf_factory::FactoryMarker;
    use futures::StreamExt;

    #[fuchsia::test]
    async fn test_lifecycle_stream_and_realm_emit() {
        let realm = TestRealm::default();
        let mut stream = realm.subscribe_to_lifecycle().unwrap();

        realm.emit_lifecycle_event(LifecycleEvent::Started("hello".to_string()));
        realm.emit_lifecycle_event(LifecycleEvent::Stopped("hello".to_string()));

        assert_eq!(stream.next_event().await, Some(LifecycleEvent::Started("hello".to_string())));
        assert_eq!(stream.next_event().await, Some(LifecycleEvent::Stopped("hello".to_string())));
    }

    #[fuchsia::test]
    async fn test_create_failure_no_namespace() {
        let res = TestRealm::create(vec![]).await;
        assert!(res.is_err());
    }

    #[fuchsia::test]
    async fn test_get_control_success_lazy() {
        let realm = TestRealm::default();
        let control = realm.get_control::<FactoryMarker>();
        assert!(control.is_ok());
    }

    #[fuchsia::test]
    async fn test_connect_to_protocol_no_proxy() {
        let realm = TestRealm::default();
        let res = realm.connect_to_protocol::<FactoryMarker>().await;
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_get_directory_no_proxy() {
        let realm = TestRealm::default();
        let dir = realm.get_directory("/pkg").await;
        assert!(dir.is_err()); // /pkg is not writable
    }

    #[fuchsia::test]
    async fn test_connect_to_protocol_with_proxy() {
        let (proxy, mut stream) = create_proxy_and_stream::<RealmProxy_Marker>();
        let realm =
            TestRealm { realm_proxy: Some(proxy), lifecycle_senders: Mutex::new(Vec::new()) };

        let task = fuchsia_async::Task::local(async move {
            if let Some(Ok(RealmProxy_Request::ConnectToNamedProtocol {
                protocol,
                responder,
                ..
            })) = stream.next().await
            {
                assert_eq!(protocol, fsys2::LifecycleControllerMarker::PROTOCOL_NAME);
                let _ = responder.send(Ok(()));
            }
        });

        let res = realm.connect_to_protocol::<fsys2::LifecycleControllerMarker>().await;
        assert!(res.is_ok());
        task.await;
    }

    #[fuchsia::test]
    async fn test_get_directory_with_proxy() {
        let (proxy, mut stream) = create_proxy_and_stream::<RealmProxy_Marker>();
        let realm =
            TestRealm { realm_proxy: Some(proxy), lifecycle_senders: Mutex::new(Vec::new()) };

        let task = fuchsia_async::Task::local(async move {
            if let Some(Ok(RealmProxy_Request::OpenService { responder, .. })) = stream.next().await
            {
                let _ = responder.send(Ok(()));
            }
        });

        let res = realm.get_directory("/some/path").await;
        assert!(res.is_ok());
        task.await;
    }

    #[fuchsia::test]
    async fn test_start_and_stop_component() {
        let (proxy, mut stream) = create_proxy_and_stream::<RealmProxy_Marker>();
        let realm =
            TestRealm { realm_proxy: Some(proxy), lifecycle_senders: Mutex::new(Vec::new()) };

        let task = fuchsia_async::Task::local(async move {
            if let Some(Ok(RealmProxy_Request::ConnectToNamedProtocol {
                protocol,
                server_end,
                responder,
            })) = stream.next().await
            {
                assert_eq!(protocol, fsys2::LifecycleControllerMarker::PROTOCOL_NAME);
                let _ = responder.send(Ok(()));
                let mut lc_stream =
                    ServerEnd::<fsys2::LifecycleControllerMarker>::new(server_end).into_stream();
                if let Some(Ok(fsys2::LifecycleControllerRequest::StartInstance {
                    moniker,
                    responder,
                    ..
                })) = lc_stream.next().await
                {
                    assert_eq!(moniker, "./test_start");
                    let _ = responder.send(Ok(()));
                }
            }
            if let Some(Ok(RealmProxy_Request::ConnectToNamedProtocol {
                protocol,
                server_end,
                responder,
            })) = stream.next().await
            {
                assert_eq!(protocol, fsys2::LifecycleControllerMarker::PROTOCOL_NAME);
                let _ = responder.send(Ok(()));
                let mut lc_stream =
                    ServerEnd::<fsys2::LifecycleControllerMarker>::new(server_end).into_stream();
                if let Some(Ok(fsys2::LifecycleControllerRequest::StopInstance {
                    moniker,
                    responder,
                    ..
                })) = lc_stream.next().await
                {
                    assert_eq!(moniker, "./test_stop");
                    let _ = responder.send(Ok(()));
                }
            }
        });

        realm.start_component("test_start").await.expect("start failed");
        realm.stop_component("test_stop").await.expect("stop failed");
        task.await;
    }
}
