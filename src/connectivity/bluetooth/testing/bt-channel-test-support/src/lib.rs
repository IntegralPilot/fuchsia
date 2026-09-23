// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::create_proxy_and_stream;
use fidl_fuchsia_bluetooth as fidl_bt;
use fuchsia_bluetooth::types::Channel;

#[derive(Debug, Clone, Copy)]
pub enum Transport {
    Socket,
    Fidl,
}

/// Creates a pair of channels for testing.
///
/// Returns `(local, remote)` where:
/// - When `transport` is `Transport::Fidl`, the first element (`local`) is a FIDL client channel
///   (wraps a `ChannelProxy`) and the second element (`remote`) is a FIDL server channel
///   (wraps a `ChannelRequestStream`).
/// - When `transport` is `Transport::Socket`, the elements represent a symmetric socket pair
///   and are not distinguished as client or server.
pub fn create_test_channels(transport: Transport) -> (Channel, Channel) {
    create_test_channels_with_max_tx(transport, Channel::DEFAULT_MAX_TX)
}

/// Creates a pair of channels for testing, specifying the maximum TX size.
///
/// Returns `(local, remote)` where:
/// - When `transport` is `Transport::Fidl`, the first element (`local`) is a FIDL client channel
///   (wraps a `ChannelProxy`) and is typically used as the "local" channel by the component under test.
/// - The second element (`remote`) is a FIDL server channel (wraps a `ChannelRequestStream`)
///   and is typically used as the "remote" channel in unit tests to simulate the remote peer.
/// - When `transport` is `Transport::Socket`, the elements represent a symmetric socket pair
///   and are not distinguished as client or server.
pub fn create_test_channels_with_max_tx(
    transport: Transport,
    max_tx_size: usize,
) -> (Channel, Channel) {
    match transport {
        Transport::Socket => Channel::create_socket_pair_with_max_tx(max_tx_size),
        Transport::Fidl => {
            let (proxy, stream) = create_proxy_and_stream::<fidl_bt::ChannelMarker>();
            let client = Channel::from_fidl_client(proxy, max_tx_size);
            let server = Channel::from_fidl_server(stream, max_tx_size);
            (client, server)
        }
    }
}
