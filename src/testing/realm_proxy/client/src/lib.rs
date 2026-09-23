// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{bail, Result};
use fidl::endpoints::{
    create_endpoints, ClientEnd, DiscoverableProtocolMarker, ServerEnd, ServiceMarker, ServiceProxy,
};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_testing_harness::{RealmProxy_Marker, RealmProxy_Proxy};
use fuchsia_component::client::connect_to_protocol;

pub mod error;
pub use error::Error;

// RealmProxyClient is a client for fuchsia.testing.harness.RealmProxy.
//
// The calling component must have a handle to the RealmProxy protocol in
// order to use this struct. Once the caller has connected to the RealmProxy
// service, they can access the other services in the proxied test realm by
// calling [connect_to_protocol].
//
// # Example Usage
//
// ```
// let realm_proxy = RealmProxyClient::connect()?;
// let echo = realm_proxy.connect_to_protocol::<EchoMarker>().await?;
// ```
pub struct RealmProxyClient {
    inner: RealmProxy_Proxy,
}

impl From<RealmProxy_Proxy> for RealmProxyClient {
    fn from(value: RealmProxy_Proxy) -> Self {
        Self { inner: value }
    }
}

impl From<ClientEnd<RealmProxy_Marker>> for RealmProxyClient {
    fn from(value: ClientEnd<RealmProxy_Marker>) -> Self {
        let inner = value.into_proxy();
        Self { inner }
    }
}

impl RealmProxyClient {
    // Connects to the RealmProxy service.
    pub fn connect() -> Result<Self, anyhow::Error> {
        let inner = connect_to_protocol::<RealmProxy_Marker>()?;
        Ok(Self { inner })
    }

    // Connects to the protocol marked by [T] via the proxy.
    //
    // Returns an error if the connection fails.
    pub async fn connect_to_protocol<T: DiscoverableProtocolMarker>(
        &self,
    ) -> Result<T::Proxy, anyhow::Error> {
        self.connect_to_named_protocol::<T>(T::PROTOCOL_NAME).await
    }

    // Connects the `sever_end` to the protocol marked by [T] via the proxy.
    //
    // Returns an error if the connection fails.
    pub async fn connect_server_end_to_protocol<T: DiscoverableProtocolMarker>(
        &self,
        server_end: ServerEnd<T>,
    ) -> Result<(), anyhow::Error> {
        self.connect_server_end_to_named_protocol::<T>(T::PROTOCOL_NAME, server_end).await
    }

    // Connects to the protocol with the given name, via the proxy.
    //
    // Returns an error if the connection fails.
    pub async fn connect_to_named_protocol<T: DiscoverableProtocolMarker>(
        &self,
        protocol_name: &str,
    ) -> Result<T::Proxy, anyhow::Error> {
        let (client, server) = create_endpoints::<T>();
        self.connect_server_end_to_named_protocol(protocol_name, server).await?;
        Ok(client.into_proxy())
    }

    // Connects the `server_end` to the protocol with the given name, via the proxy.
    //
    // Returns an error if the connection fails.
    pub async fn connect_server_end_to_named_protocol<T: DiscoverableProtocolMarker>(
        &self,
        protocol_name: &str,
        server_end: ServerEnd<T>,
    ) -> Result<(), anyhow::Error> {
        let res =
            self.inner.connect_to_named_protocol(protocol_name, server_end.into_channel()).await?;

        if let Some(op_err) = res.err() {
            bail!("{:?}", op_err);
        }

        Ok(())
    }

    // Opens the given service capability, via the proxy.
    //
    // See https://fuchsia.dev/fuchsia-src/concepts/components/v2/capabilities/service
    // for more information about service capabilities.
    //
    // Returns an error if the connection fails.
    pub async fn open_service<T: ServiceMarker>(
        &self,
    ) -> Result<fio::DirectoryProxy, anyhow::Error> {
        let (client, server) = create_endpoints::<fio::DirectoryMarker>();
        let res = self.inner.open_service(T::SERVICE_NAME, server.into_channel()).await?;
        if let Some(op_err) = res.err() {
            bail!("{:?}", op_err);
        }

        Ok(client.into_proxy())
    }

    // Connects to the given service instance, via the proxy.
    //
    // See https://fuchsia.dev/fuchsia-src/concepts/components/v2/capabilities/service
    // for more information about service capabilities.
    //
    // Returns an error if the connection fails.
    pub async fn connect_to_service_instance<T: ServiceMarker>(
        &self,
        instance: &str,
    ) -> Result<T::Proxy, anyhow::Error> {
        let (client, server) = create_endpoints::<fio::DirectoryMarker>();
        let res = self
            .inner
            .connect_to_service_instance(T::SERVICE_NAME, instance, server.into_channel())
            .await?;
        if let Some(op_err) = res.err() {
            bail!("{:?}", op_err);
        }

        Ok(T::Proxy::from_member_opener(Box::new(
            fuchsia_component::client::ServiceInstanceDirectory(
                client.into_proxy(),
                instance.to_owned(),
            ),
        )))
    }
}
