// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fidl_fuchsia_driver_test as fdt;
use fuchsia_component_test::RealmBuilder;
use fuchsia_driver_test::{DriverTestRealmBuilder, DriverTestRealmInstance};
use log::info;

#[fuchsia::main(logging = true)]
async fn main() -> Result<(), anyhow::Error> {
    // This is a test component that exists solely to spin up an instance of Driver Test Realm so
    // that driver_dump_select_test.sh can test `ffx driver dump --select`.
    let builder = RealmBuilder::new().await?;
    builder.driver_test_realm_setup().await?;
    let instance = builder.build().await?;
    instance.driver_test_realm_start(fdt::RealmArgs::default()).await?;

    info!("Started Driver Test Realm. Now looping forever.");
    loop {}
}
