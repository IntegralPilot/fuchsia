// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Debug;

use futures::channel::oneshot;

use crate::client::InternalClient;
use crate::logging::log_debug;
use crate::messaging::Sender;
use crate::protocol_family::ProtocolFamily;
use crate::protocol_family::route::NetlinkRoute;

pub(crate) fn respond_to_completer<
    S: Sender<<NetlinkRoute as ProtocolFamily>::Response>,
    D: Debug,
    E: Debug + PartialEq + Copy,
>(
    client: InternalClient<NetlinkRoute, S>,
    completer: oneshot::Sender<Result<(), E>>,
    result: Result<(), E>,
    request_for_log: D,
) {
    match completer.send(result) {
        Ok(()) => (),
        Err(err) => {
            assert_eq!(err, result, "should get back what we tried to send");

            // Not treated as a hard error because the socket may have been
            // closed.
            log_debug!(
                "failed to send result ({:?}) to {} after handling {:?}",
                result,
                client,
                request_for_log,
            )
        }
    }
}
