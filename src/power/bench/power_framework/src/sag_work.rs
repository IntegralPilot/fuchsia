// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! common functions to be used by Criterion or integration test for SAG.

use anyhow::Result;
use fidl_fuchsia_power_system as fsystem;
use fuchsia_component::client::connect_to_protocol_sync;

use std::sync::Arc;

#[inline(always)]
fn black_box<T>(placeholder: T) -> T {
    std::hint::black_box(placeholder)
}

fn work_func(sag: &fsystem::ActivityGovernorSynchronousProxy) -> Result<()> {
    let _event_pair = sag.acquire_wake_lease("benchmark", zx::MonotonicInstant::INFINITE).unwrap();

    Ok(())
}

pub(crate) fn obtain_sag_proxy() -> Arc<fsystem::ActivityGovernorSynchronousProxy> {
    // Current Criterion library doesn't support async call yet.
    let sag = connect_to_protocol_sync::<fsystem::ActivityGovernorMarker>().unwrap();
    Arc::new(sag)
}

pub(crate) fn execute(sag_arc: &fsystem::ActivityGovernorSynchronousProxy) {
    let _ = black_box(work_func(sag_arc));
}
