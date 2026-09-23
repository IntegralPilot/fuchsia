// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use netstack3_base::testutil::TEST_ADDRS_V4;
use netstack3_core::device::DeviceId;
use netstack3_core::device_socket::TargetDevice;
use netstack3_core::sync::Mutex;
use netstack3_core::testutil::{CtxPairExt as _, FakeCtxBuilder};

#[test]
fn drop_real_ids() {
    // Test with a real `CoreCtx` to assert that IDs aren't dropped in the
    // wrong order.
    let (mut ctx, device_ids) = FakeCtxBuilder::with_addrs(TEST_ADDRS_V4).build();

    let mut api = ctx.core_api().device_socket();

    let never_bound = api.create(Mutex::default());
    let bound_any_device = {
        let id = api.create(Mutex::default());
        api.set_device(&id, TargetDevice::AnyDevice);
        id
    };
    let bound_specific_device = {
        let id = api.create(Mutex::default());
        api.set_device(
            &id,
            TargetDevice::SpecificDevice(&DeviceId::Ethernet(device_ids[0].clone())),
        );
        id
    };

    // Make sure the socket IDs go out of scope before `ctx`.
    drop((never_bound, bound_any_device, bound_specific_device));
}
