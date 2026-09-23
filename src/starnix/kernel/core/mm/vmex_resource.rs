// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_kernel as fkernel;
use fuchsia_component::client::connect_to_protocol_sync;
use std::sync::LazyLock;

pub static VMEX_RESOURCE: LazyLock<zx::Resource> = LazyLock::new(|| {
    connect_to_protocol_sync::<fkernel::VmexResourceMarker>()
        .expect("couldn't connect to fuchsia.kernel.VmexResource")
        .get(zx::MonotonicInstant::INFINITE)
        .expect("couldn't talk to fuchsia.kernel.VmexResource")
});
