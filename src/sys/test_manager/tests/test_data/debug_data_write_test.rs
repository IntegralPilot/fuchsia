// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_debugdata::PublisherMarker;
use fuchsia_component::client::connect_to_protocol;

const VMO_CONTENTS: &[u8] = b"Debug data from test\n";
const VMO_NAME: zx::Name = zx::Name::new_lossy("vmo_name");
const VMO_DATA_SINK: &str = "data_sink";

#[fuchsia::test]
async fn publish_debug_data() {
    let publish_data = connect_to_protocol::<PublisherMarker>().unwrap();
    let vmo = zx::Vmo::create(1024).unwrap();
    vmo.write(VMO_CONTENTS, 0).expect("write to VMO");
    vmo.set_content_size(&(VMO_CONTENTS.len() as u64)).expect("set VMO content size");
    vmo.set_name(&VMO_NAME).expect("set VMO name");
    let (vmo_token, vmo_server) = zx::EventPair::create();
    publish_data.publish(VMO_DATA_SINK, vmo, vmo_server).expect("Publish debugdata");
    drop(vmo_token);
}
