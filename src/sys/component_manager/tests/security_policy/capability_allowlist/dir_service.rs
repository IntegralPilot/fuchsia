// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component::server::ServiceFs;
use futures::stream::StreamExt;

#[fuchsia::main]
async fn main() {
    let mut svc = ServiceFs::new();
    let dir = vfs::directory::immutable::simple();
    svc.add_entry_at("unrestricted", dir.clone());
    svc.add_entry_at("restricted", dir);
    svc.take_and_serve_directory_handle().expect("failed to serve outgoing dir");
    svc.collect::<()>().await;
}
