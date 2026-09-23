// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl::endpoints::DiscoverableProtocolMarker;
use fuchsia_component::client::connect_to_protocol;

pub trait ServiceConnect: Send + Sync {
    fn connect_to_service<P: DiscoverableProtocolMarker>(&self) -> Result<P::Proxy, Error>;
}

#[derive(Debug, Clone)]
pub struct ServiceConnector;

impl ServiceConnect for ServiceConnector {
    fn connect_to_service<P: DiscoverableProtocolMarker>(&self) -> Result<P::Proxy, Error> {
        connect_to_protocol::<P>()
    }
}
