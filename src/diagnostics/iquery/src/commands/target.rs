// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::commands::types::DiagnosticsProvider;
use crate::commands::utils::*;
use crate::types::Error;
use diagnostics_data::{Data, Inspect};
use diagnostics_reader::{ArchiveReader, RetryConfig};
use fidl::endpoints::DiscoverableProtocolMarker;
use flex_fuchsia_diagnostics::{ArchiveAccessorMarker, ArchiveAccessorProxy, Selector};
use flex_fuchsia_sys2 as fsys2;
use moniker::Moniker;

const ROOT_ARCHIVIST: &str = "bootstrap/archivist";

pub struct ArchiveAccessorProvider {
    query_proxy: fsys2::RealmQueryProxy,
}

impl ArchiveAccessorProvider {
    pub fn new(proxy: fsys2::RealmQueryProxy) -> Self {
        Self { query_proxy: proxy }
    }
}

impl DiagnosticsProvider for ArchiveAccessorProvider {
    async fn snapshot(
        &self,
        accessor: Option<&str>,
        selectors: impl IntoIterator<Item = Selector>,
    ) -> Result<Vec<Data<Inspect>>, Error> {
        let archive = connect_to_accessor_selector(accessor, &self.query_proxy).await?;
        ArchiveReader::inspect()
            .with_archive(archive)
            .retry(RetryConfig::never())
            .add_selectors(selectors.into_iter())
            .snapshot()
            .await
            .map_err(Error::Fetch)
    }

    async fn get_accessor_paths(&self) -> Result<Vec<String>, Error> {
        get_accessor_selectors(&self.query_proxy).await
    }

    fn realm_query(&self) -> &fsys2::RealmQueryProxy {
        &self.query_proxy
    }
}

/// Connect to `fuchsia.diagnostics.*ArchivistAccessor` with the provided selector string.
/// The selector string can be in the form of "<moniker>:<service_name>" or "<moniker>", and it
/// performs fuzzy matching on both. See `commands::utils::fuzzy_search_accessors` for more
/// information.
/// If no selector string is provided, it will try to connect to
/// `bootstrap/archivist:fuchsia.diagnostics.ArchiveAccessor`.
pub async fn connect_to_accessor_selector(
    selector: Option<&str>,
    query_proxy: &fsys2::RealmQueryProxy,
) -> Result<ArchiveAccessorProxy, Error> {
    match selector {
        Some(s) => {
            // try to fuzzy search for the moniker and protocol
            let (moniker, protocol) = fuzzy_search_accessors(s, query_proxy).await?;
            connect_accessor::<ArchiveAccessorMarker>(&moniker, &protocol, query_proxy).await
        }
        None => {
            let moniker = Moniker::try_from(ROOT_ARCHIVIST).unwrap();
            connect_accessor::<ArchiveAccessorMarker>(
                &moniker,
                ArchiveAccessorMarker::PROTOCOL_NAME,
                query_proxy,
            )
            .await
        }
    }
}
