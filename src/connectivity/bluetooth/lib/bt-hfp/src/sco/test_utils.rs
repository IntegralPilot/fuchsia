// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bredr::ScoConnectionMarker;
use fidl_fuchsia_bluetooth_bredr as bredr;
use fuchsia_bluetooth::types::PeerId;

use crate::codec_id::CodecId;
use crate::sco::Connection;
use crate::sco::connector::parameter_sets_for_codec;

#[track_caller]
pub fn connection_for_codec(
    peer_id: PeerId,
    codec_id: CodecId,
    in_band: bool,
) -> (Connection, bredr::ScoConnectionRequestStream) {
    let sco_params = parameter_sets_for_codec(codec_id, in_band).pop().unwrap();
    let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<ScoConnectionMarker>();
    let connection = Connection::build(peer_id, sco_params, proxy);
    (connection, stream)
}
