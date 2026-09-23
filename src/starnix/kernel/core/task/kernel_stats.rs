// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component::client::connect_to_protocol_sync;
use std::sync::OnceLock;

#[derive(Default)]
pub struct KernelStats(OnceLock<fidl_fuchsia_kernel::StatsSynchronousProxy>);

impl KernelStats {
    pub fn get(&self) -> &fidl_fuchsia_kernel::StatsSynchronousProxy {
        self.0.get_or_init(|| {
            connect_to_protocol_sync::<fidl_fuchsia_kernel::StatsMarker>()
                .expect("Failed to connect to fuchsia.kernel.Stats.")
        })
    }
}
