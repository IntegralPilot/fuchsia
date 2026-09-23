// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;

use fidl::endpoints::DiscoverableProtocolMarker;

#[derive(Error, Debug)]
#[error("can't connect to protocol {proto}: {err}")]
pub struct ConnectToProtocolError {
    proto: &'static str,
    err: anyhow::Error,
}

pub fn connect_to_protocol<P: DiscoverableProtocolMarker>()
-> Result<P::Proxy, ConnectToProtocolError> {
    fuchsia_component::client::connect_to_protocol::<P>()
        .map_err(|err| ConnectToProtocolError { proto: P::PROTOCOL_NAME, err })
}
