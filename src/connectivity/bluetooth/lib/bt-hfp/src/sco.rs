// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::Proxy;
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_bluetooth::profile::ValidScoConnectionParameters;
use fuchsia_bluetooth::types::PeerId;
use fuchsia_inspect_derive::Unit;
use futures::Future;
use profile_client::Error as ProfileError;
use thiserror::Error;

pub mod connector;
pub mod state;
pub mod test_utils;

pub use connector::Connector;
pub use state::{Active, InspectableState, State};

/// A failure occurred connecting SCO to a peer.
#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("SCO connection failed")]
    Failed,
    #[error("SCO connection canceled by new connection request or server")]
    Canceled,
    #[error("SCO connection provided invalid arguments")]
    InvalidArguments,
    #[error("Profile client error: {:?}", source)]
    ProfileClient {
        #[from]
        source: ProfileError,
    },
    #[error("FIDL error: {:?}", source)]
    Fidl {
        #[from]
        source: fidl::Error,
    },
}

impl From<bredr::ScoErrorCode> for ConnectError {
    fn from(err: bredr::ScoErrorCode) -> Self {
        match err {
            bredr::ScoErrorCode::Cancelled => Self::Canceled,
            bredr::ScoErrorCode::Failure => Self::Failed,
            bredr::ScoErrorCode::InvalidArguments => Self::InvalidArguments,
            bredr::ScoErrorCode::ParametersRejected => Self::Failed,
            _ => Self::Failed, // Flexible enum future proofing.
        }
    }
}

/// The components of an active SCO connection.
/// Dropping this struct will close the SCO connection.
#[derive(Debug)]
pub struct Connection {
    /// The peer this is connected to.
    pub peer_id: PeerId,
    /// The parameters that this connection was set up with.
    pub params: ValidScoConnectionParameters,
    /// Proxy for this connection.  Used to read/write to the connection.
    pub proxy: bredr::ScoConnectionProxy,
}

impl Unit for Connection {
    type Data = <ValidScoConnectionParameters as Unit>::Data;
    fn inspect_create(&self, parent: &fuchsia_inspect::Node, name: impl AsRef<str>) -> Self::Data {
        self.params.inspect_create(parent, name)
    }

    fn inspect_update(&self, data: &mut Self::Data) {
        self.params.inspect_update(data)
    }
}

impl Connection {
    pub fn on_closed(&self) -> impl Future<Output = ()> + 'static {
        let proxy = self.proxy.clone();
        async move {
            let _ = proxy.on_closed().await;
        }
    }

    pub fn is_closed(&self) -> bool {
        self.proxy.is_closed()
    }

    // This function is intendred to be used in test code. Since it's used outside of this crate, it
    // can't be marked as #[cfg(test)].
    // The proxy should have been built using the params specified.
    pub fn build(
        peer_id: PeerId,
        params: bredr::ScoConnectionParameters,
        proxy: bredr::ScoConnectionProxy,
    ) -> Self {
        Connection { peer_id, params: params.try_into().unwrap(), proxy }
    }
}
