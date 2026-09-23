// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, Result};
use diagnostics_reader::ArchiveReader;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_diagnostics::{self as fdiagnostics, LogSettingsSetComponentInterestRequest};
use fidl_fuchsia_diagnostics_types::{Interest, Severity};
use realm_proxy_client::RealmProxyClient;
use selectors::{VerboseError, parse_component_selector};
use std::borrow::Cow;

pub const ALL_PIPELINE: &str = "all";

/// Returns a snapshot of the realm's logs as a stream.
///
/// The realm must expose fuchsia.diagnostics.ArchiveAccessor.
pub(crate) async fn snapshot_and_stream_logs(
    realm_proxy: &RealmProxyClient,
) -> impl crate::assert::LogStream + use<> {
    let accessor = connect_accessor(realm_proxy, ALL_PIPELINE).await;
    let subscription = ArchiveReader::logs()
        .with_archive(accessor)
        .snapshot_then_subscribe()
        .expect("subscribe to logs");
    subscription.wait_for_ready().await;
    subscription
}

pub(crate) async fn connect_accessor(
    realm_proxy: &RealmProxyClient,
    pipeline_name: &str,
) -> fdiagnostics::ArchiveAccessorProxy {
    connect_accessor_protocol::<fdiagnostics::ArchiveAccessorMarker>(realm_proxy, pipeline_name)
        .await
}

async fn connect_accessor_protocol<P: DiscoverableProtocolMarker>(
    realm_proxy: &RealmProxyClient,
    pipeline_name: &str,
) -> P::Proxy {
    let accessor_name = if pipeline_name == ALL_PIPELINE {
        Cow::Borrowed(P::PROTOCOL_NAME)
    } else {
        Cow::Owned(format!("{}.{}", P::PROTOCOL_NAME, pipeline_name))
    };
    realm_proxy
        .connect_to_named_protocol::<P>(&format!("diagnostics-accessors/{accessor_name}"))
        .await
        .expect("connect to archive accessor")
}

/// Extension methods on LogSettingsProxy.
#[async_trait::async_trait]
pub(crate) trait LogSettingsExt {
    /// Changes the logs interest configuration for a set of components that match `selector`
    ///
    /// # Errors
    ///
    /// Returns an error if `selector` is not a valid component selector.
    /// Returns an error if the call to LogSettings/SetInterest fails.
    async fn set_interest_for_component(
        &self,
        selector: &str,
        severity: Severity,
        persist: bool,
    ) -> Result<(), Error>;
}

#[async_trait::async_trait]
impl LogSettingsExt for fdiagnostics::LogSettingsProxy {
    async fn set_interest_for_component(
        &self,
        selector: &str,
        severity: Severity,
        persist: bool,
    ) -> Result<(), Error> {
        let component_selector = parse_component_selector::<VerboseError>(selector)?;
        let interests = vec![fdiagnostics::LogInterestSelector {
            selector: component_selector,
            interest: Interest { min_severity: Some(severity), ..Default::default() },
        }];
        self.set_component_interest(&LogSettingsSetComponentInterestRequest {
            selectors: Some(interests),
            persist: Some(persist),
            ..Default::default()
        })
        .await
        .context("set interest")?;
        Ok(())
    }
}
