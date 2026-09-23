// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth as bt;
use fidl_fuchsia_bluetooth_host as fidl_host;
use fidl_fuchsia_bluetooth_sys as sys;
use futures_util::stream::StreamExt as _;
use tracing::{info, warn};

pub struct HostServer {
    stream: fidl_host::HostRequestStream,
    host_info: sys::HostInfo,
    watch_state_changed: bool,
    watch_state_responder: Option<fidl_host::HostWatchStateResponder>,
}

impl HostServer {
    pub fn new(stream: fidl_host::HostRequestStream) -> Self {
        // TODO(https://fxbug.dev/538185448): Read Bluetooth address from HCI device.
        let host_info = sys::HostInfo {
            id: Some(bt::HostId { value: 1 }),
            technology: Some(sys::TechnologyType::DualMode),
            addresses: Some(vec![bt::Address {
                type_: bt::AddressType::Public,
                bytes: [0, 0, 0, 0, 0, 0],
            }]),
            active: Some(true),
            discoverable: Some(false),
            discovering: Some(false),
            local_name: None,
            ..Default::default()
        };
        Self { stream, host_info, watch_state_changed: true, watch_state_responder: None }
    }

    pub async fn run(&mut self) -> Result<(), fidl::Error> {
        while let Some(req) = self.stream.next().await {
            match req? {
                fidl_host::HostRequest::WatchState { responder } => {
                    self.on_watch_state(responder);
                }
                fidl_host::HostRequest::Shutdown { .. } => {
                    info!("Received Shutdown request; shutting down bt-host");
                    break;
                }
                _ => {
                    // Ignore unhandled requests for now
                }
            }
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn set_discoverable(&mut self, discoverable: bool) {
        self.host_info.discoverable = Some(discoverable);
        self.on_host_info_changed();
    }

    #[allow(dead_code)]
    pub fn set_discovering(&mut self, discovering: bool) {
        self.host_info.discovering = Some(discovering);
        self.on_host_info_changed();
    }

    #[allow(dead_code)]
    pub fn set_addresses(&mut self, addresses: Vec<bt::Address>) {
        self.host_info.addresses = Some(addresses);
        self.on_host_info_changed();
    }

    #[allow(dead_code)]
    pub fn set_local_name(&mut self, local_name: Option<String>) {
        self.host_info.local_name = local_name;
        self.on_host_info_changed();
    }

    #[allow(dead_code)]
    fn on_host_info_changed(&mut self) {
        if let Some(responder) = self.watch_state_responder.take() {
            if let Err(e) = responder.send(&self.host_info) {
                warn!("Failed to send WatchState response: {e:?}");
            }
            self.watch_state_changed = false;
        } else {
            self.watch_state_changed = true;
        }
    }

    fn on_watch_state(&mut self, responder: fidl_host::HostWatchStateResponder) {
        if let Some(old_responder) = self.watch_state_responder.take() {
            warn!("Client called WatchState while a request was already pending");
            if let Err(e) = old_responder.send(&self.host_info) {
                warn!("Failed to send WatchState response: {e:?}");
            }
        }

        if self.watch_state_changed {
            if let Err(e) = responder.send(&self.host_info) {
                warn!("Failed to send WatchState response: {e:?}");
            }
            self.watch_state_changed = false;
        } else {
            // Hanging get: defer response until state changes.
            self.watch_state_responder = Some(responder);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::join;

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_watch_state_and_shutdown() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fidl_host::HostMarker>();
        let mut server = HostServer::new(stream);

        let server_fut = server.run();

        let client_fut = async move {
            let info = proxy.watch_state().await.unwrap();
            assert_eq!(info.id, Some(bt::HostId { value: 1 }));
            assert_eq!(info.local_name, None);
            proxy.shutdown().unwrap();
        };

        let (server_res, ()) = join(server_fut, client_fut).await;
        assert!(server_res.is_ok());
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_host_info_setters_update_hanging_get() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fidl_host::HostMarker>();
        let mut server = HostServer::new(stream);

        // First watch_state request gets initial state immediately
        let first_watch = proxy.watch_state();
        let server_fut = async {
            if let Some(Ok(fidl_host::HostRequest::WatchState { responder })) =
                server.stream.next().await
            {
                server.on_watch_state(responder);
            }
        };
        let (first_info, ()) = join(first_watch, server_fut).await;
        assert_eq!(first_info.unwrap().discoverable, Some(false));

        // Test set_discoverable
        let watch_fut = proxy.watch_state();
        if let Some(Ok(fidl_host::HostRequest::WatchState { responder })) =
            server.stream.next().await
        {
            server.on_watch_state(responder);
        }
        server.set_discoverable(true);
        assert_eq!(watch_fut.await.unwrap().discoverable, Some(true));

        // Test set_discovering
        let watch_fut = proxy.watch_state();
        if let Some(Ok(fidl_host::HostRequest::WatchState { responder })) =
            server.stream.next().await
        {
            server.on_watch_state(responder);
        }
        server.set_discovering(true);
        assert_eq!(watch_fut.await.unwrap().discovering, Some(true));

        // Test set_addresses
        let watch_fut = proxy.watch_state();
        if let Some(Ok(fidl_host::HostRequest::WatchState { responder })) =
            server.stream.next().await
        {
            server.on_watch_state(responder);
        }
        let new_addrs =
            vec![bt::Address { type_: bt::AddressType::Public, bytes: [1, 2, 3, 4, 5, 6] }];
        server.set_addresses(new_addrs.clone());
        assert_eq!(watch_fut.await.unwrap().addresses, Some(new_addrs));

        // Test set_local_name
        let watch_fut = proxy.watch_state();
        if let Some(Ok(fidl_host::HostRequest::WatchState { responder })) =
            server.stream.next().await
        {
            server.on_watch_state(responder);
        }
        server.set_local_name(Some("fuchsia-bt".to_string()));
        assert_eq!(watch_fut.await.unwrap().local_name, Some("fuchsia-bt".to_string()));
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_watch_state_duplicate_request_contract_violation() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fidl_host::HostMarker>();
        let mut server = HostServer::new(stream);

        // First watch_state request gets initial state immediately
        let first_watch = proxy.watch_state();
        if let Some(Ok(fidl_host::HostRequest::WatchState { responder })) =
            server.stream.next().await
        {
            server.on_watch_state(responder);
        }
        let first_info = first_watch.await.unwrap();
        assert_eq!(first_info.discoverable, Some(false));

        // Send watch_state call 1 (will be stored in watch_state_responder)
        let watch_fut1 = proxy.watch_state();
        if let Some(Ok(fidl_host::HostRequest::WatchState { responder })) =
            server.stream.next().await
        {
            server.on_watch_state(responder);
        }
        assert!(server.watch_state_responder.is_some());

        // Send watch_state call 2 while watch_state_responder is Some (FIDL contract violation)
        let watch_fut2 = proxy.watch_state();
        if let Some(Ok(fidl_host::HostRequest::WatchState { responder })) =
            server.stream.next().await
        {
            server.on_watch_state(responder);
        }

        // The first call should resolve immediately (notified upon replacement).
        let info1 = watch_fut1.await.unwrap();
        assert_eq!(info1.discoverable, Some(false));

        // The second call is now the pending watch_state_responder since watch_state_changed was false.
        assert!(server.watch_state_responder.is_some());

        // When state changes, the second call resolves.
        server.set_discoverable(true);
        let info2 = watch_fut2.await.unwrap();
        assert_eq!(info2.discoverable, Some(true));
        assert!(server.watch_state_responder.is_none());
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_state_change_without_pending_watch_state() {
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fidl_host::HostMarker>();
        let mut server = HostServer::new(stream);

        // First watch_state gets initial state
        let first_watch = proxy.watch_state();
        if let Some(Ok(fidl_host::HostRequest::WatchState { responder })) =
            server.stream.next().await
        {
            server.on_watch_state(responder);
        }
        let _ = first_watch.await.unwrap();

        // Change state while no watch_state call is hanging
        server.set_discoverable(true);
        assert!(server.watch_state_changed);

        // Now client calls watch_state, should get updated state immediately
        let watch_fut = proxy.watch_state();
        if let Some(Ok(fidl_host::HostRequest::WatchState { responder })) =
            server.stream.next().await
        {
            server.on_watch_state(responder);
        }
        assert_eq!(watch_fut.await.unwrap().discoverable, Some(true));
    }
}
