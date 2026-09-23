// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ProtocolMarker as _;
use fidl_fuchsia_net_name::{self as fnet_name, DnsServerWatcherRequestStream};

use log::warn;

use crate::bindings::Netstack;

pub(super) async fn serve(
    _ns: Netstack,
    _stream: DnsServerWatcherRequestStream,
) -> Result<(), fidl::Error> {
    // netstack3 deliberately does not serve fuchsia.net.name.DnsServerWatcher,
    // with the expectation that netcfg serves a unified view of DNS servers
    // instead (including consuming the
    // fuchsia.net.ndp.RouterAdvertisementOptionWatcherProvider instance that
    // netstack3 does serve).
    warn!(
        "Blocking forever serving {}. To obtain DNS servers, use netcfg instead.",
        fnet_name::DnsServerWatcherMarker::DEBUG_NAME
    );
    futures::future::pending().await
}
