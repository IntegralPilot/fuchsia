// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use diagnostics_data::{Data, Inspect};
use fdomain_client::fidl::{DiscoverableProtocolMarker, Proxy};
use fdomain_fuchsia_diagnostics::ClientSelectorConfiguration::{SelectAll, Selectors};
use fdomain_fuchsia_diagnostics::{
    ArchiveAccessorMarker, Format, Selector, SelectorArgument, StreamParameters,
};
use fdomain_fuchsia_diagnostics_host::{
    ArchiveAccessorMarker as HostArchiveAccessorMarker, ArchiveAccessorProxy,
};
use fdomain_fuchsia_sys2 as fsys2;
use futures::AsyncReadExt;
use iquery_fdomain::commands::{
    DiagnosticsProvider, connect_accessor, fuzzy_search_accessors, get_accessor_selectors,
};
use iquery_fdomain::types::Error;
use serde::Deserialize;
use std::borrow::Cow;

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

pub struct HostArchiveReader {
    diagnostics_proxy: ArchiveAccessorProxy,
    query_proxy: fsys2::RealmQueryProxy,
}

fn target_protocol_to_host(protocol: &str) -> String {
    if let Some(suffix) = protocol.strip_prefix(ArchiveAccessorMarker::PROTOCOL_NAME) {
        format!("{}{suffix}", HostArchiveAccessorMarker::PROTOCOL_NAME)
    } else {
        protocol.to_string()
    }
}

impl HostArchiveReader {
    pub fn new(
        diagnostics_proxy: ArchiveAccessorProxy,
        query_proxy: fsys2::RealmQueryProxy,
    ) -> Self {
        Self { diagnostics_proxy, query_proxy }
    }

    pub async fn snapshot_diagnostics_data<D>(
        &self,
        accessor: Option<&str>,
        selectors: impl IntoIterator<Item = Selector>,
    ) -> Result<Vec<Data<D>>, Error>
    where
        D: diagnostics_data::DiagnosticsData,
    {
        let mut selectors = selectors.into_iter().peekable();
        let selectors = if selectors.peek().is_none() {
            SelectAll(true)
        } else {
            Selectors(selectors.map(|s| SelectorArgument::StructuredSelector(s)).collect())
        };

        let accessor = match accessor.as_deref() {
            Some(s) => {
                let (moniker, protocol) = fuzzy_search_accessors(&s, &self.query_proxy).await?;
                let host_protocol = target_protocol_to_host(&protocol);
                let proxy = connect_accessor::<HostArchiveAccessorMarker>(
                    &moniker,
                    &host_protocol,
                    &self.query_proxy,
                )
                .await?;
                Cow::Owned(proxy)
            }
            None => Cow::Borrowed(&self.diagnostics_proxy),
        };

        let params = StreamParameters {
            stream_mode: Some(fdomain_fuchsia_diagnostics::StreamMode::Snapshot),
            data_type: Some(D::DATA_TYPE),
            format: Some(Format::Json),
            client_selector_configuration: Some(selectors),
            ..Default::default()
        };

        let (mut client, server) = self.query_proxy.domain().create_stream_socket();

        let _ = accessor.stream_diagnostics(&params, server).await.map_err(|s| {
            Error::IOError(
                "call ArchiveAccessor".into(),
                anyhow!("failure setting up diagnostics stream: {:?}", s),
            )
        })?;

        let mut output = vec![];
        match client.read_to_end(&mut output).await {
            Err(e) => Err(Error::IOError("get next".into(), e.into())),
            Ok(_) => Ok(serde_json::Deserializer::from_slice(&output)
                .into_iter::<OneOrMany<Data<D>>>()
                .filter_map(|value| value.ok())
                .map(|value| match value {
                    OneOrMany::One(value) => vec![value],
                    OneOrMany::Many(values) => values,
                })
                .flatten()
                .collect()),
        }
    }
}

impl DiagnosticsProvider for HostArchiveReader {
    async fn snapshot(
        &self,
        accessor_path: Option<&str>,
        selectors: impl IntoIterator<Item = Selector>,
    ) -> Result<Vec<Data<Inspect>>, Error> {
        self.snapshot_diagnostics_data::<Inspect>(accessor_path, selectors).await
    }

    async fn get_accessor_paths(&self) -> Result<Vec<String>, Error> {
        get_accessor_selectors(&self.query_proxy).await
    }

    fn realm_query(&self) -> &fsys2::RealmQueryProxy {
        &self.query_proxy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case("fuchsia.diagnostics.ArchiveAccessor", "fuchsia.diagnostics.host.ArchiveAccessor")]
    #[test_case(
        "fuchsia.diagnostics.ArchiveAccessor.feedback",
        "fuchsia.diagnostics.host.ArchiveAccessor.feedback"
    )]
    #[test_case(
        "fuchsia.diagnostics.ArchiveAccessor.previous_boot",
        "fuchsia.diagnostics.host.ArchiveAccessor.previous_boot"
    )]
    #[test_case(
        "fuchsia.diagnostics.ArchiveAccessor.lowpan",
        "fuchsia.diagnostics.host.ArchiveAccessor.lowpan"
    )]
    #[test_case(
        "fuchsia.diagnostics.ArchiveAccessor.custom_pipeline",
        "fuchsia.diagnostics.host.ArchiveAccessor.custom_pipeline"
    )]
    #[test_case(
        "fuchsia.diagnostics.host.ArchiveAccessor",
        "fuchsia.diagnostics.host.ArchiveAccessor"
    )]
    #[test_case(
        "fuchsia.diagnostics.host.ArchiveAccessor.feedback",
        "fuchsia.diagnostics.host.ArchiveAccessor.feedback"
    )]
    fn test_target_protocol_to_host(input_protocol: &str, expected_protocol: &str) {
        assert_eq!(target_protocol_to_host(input_protocol), expected_protocol);
    }
}
