// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use fidl::endpoints::{DiscoverableProtocolMarker, ProtocolMarker, create_endpoints};
use fuchsia_component::client::connect_to_protocol;
use realm_proxy_client::RealmProxyClient;
use {fidl_fuchsia_archivist_test as ftest, fidl_fuchsia_testing_harness as fharness};

/// Creates a new test realm with an archivist inside.
/// `options_fn` is called with a default RealmOptions struct and can modify any options
/// before the realm is created.
pub async fn create_realm(options: ftest::RealmOptions) -> Result<RealmProxyClient> {
    let realm_factory = connect_to_protocol::<ftest::RealmFactoryMarker>()?;
    let (client, server) = create_endpoints::<fharness::RealmProxy_Marker>();
    realm_factory
        .create_realm(&options, server)
        .await?
        .map_err(realm_proxy_client::Error::OperationError)?;
    Ok(RealmProxyClient::from(client))
}

// Helper type for constructing `PuppetDecl`.
pub(crate) struct PuppetDeclBuilder {
    name: String,
}

impl PuppetDeclBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl From<PuppetDeclBuilder> for ftest::PuppetDecl {
    fn from(val: PuppetDeclBuilder) -> Self {
        ftest::PuppetDecl { name: Some(val.name), ..Default::default() }
    }
}

/// Connects to the puppet in the test realm with the given name.
pub async fn connect_to_puppet(
    realm_proxy: &RealmProxyClient,
    puppet_name: &str,
) -> Result<<ftest::PuppetMarker as ProtocolMarker>::Proxy> {
    let puppet_protocol_alias = format!("{}.{puppet_name}", ftest::PuppetMarker::PROTOCOL_NAME);
    realm_proxy
        .connect_to_named_protocol::<ftest::PuppetMarker>(&puppet_protocol_alias)
        .await
        .with_context(|| format!("failed to connect to {puppet_name}"))
}
