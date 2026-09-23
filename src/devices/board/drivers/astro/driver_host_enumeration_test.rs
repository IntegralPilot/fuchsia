// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This test verifies that the drivers on the astro board are grouped into
// driver hosts as expected. The expected groupings are defined in `astro_host_golden.json`.

use anyhow::{Context, Result};
use driver_host_enumeration_lib::{ExpectedDriverHost, verify_driver_hosts};

#[fuchsia::main]
async fn main() -> Result<()> {
    let expected: Vec<ExpectedDriverHost> =
        serde_json::from_str(include_str!("astro_host_golden.json"))
            .context("Failed to parse astro_host_golden.json")?;
    verify_driver_hosts(expected).await
}
