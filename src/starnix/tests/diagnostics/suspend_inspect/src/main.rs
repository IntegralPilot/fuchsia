// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result};
use component_events::events::{EventStream, Started, Stopped};
use component_events::matcher::EventMatcher;
use diagnostics_reader::ArchiveReader;
use fidl_fuchsia_test_suspend as fftsu;
use fuchsia_component as fxc;
use fuchsia_component_test as fxct;
use fuchsia_driver_test as _;
use log::info;

#[fuchsia::test]
async fn test_suspend() -> Result<()> {
    let mut events = EventStream::open().await.unwrap();
    let collection = "suspend_inspect";
    let child_name = "suspend_linux";
    let url = "#meta/suspend_client.cm";

    // This moniker is relative to this test component. However, monikers in
    // test framework are relative to the **test root**. If you want to have
    // monikers relative to this component (which you probably do), then
    // the CML for routing `event_stream` **must** include a directive:
    // `scope: "#test_suite"`, or similar to inform the event subsystem.
    // If you don't do this, your code will not receive the events you expect.
    let moniker = format!("{collection}:{child_name}");

    {
        // Keep this alive to keep the test realm alive.
        let realm_control_proxy = fxc::client::connect_to_protocol::<fftsu::RealmMarker>()
            .context("connecting to driver test realm")?;

        // Required for the test realm to be created.
        realm_control_proxy.create().await.context("while creating test realm")?;

        {
            let instance = fxct::ScopedInstance::new_with_name(
                child_name.into(),
                collection.into(),
                url.into(),
            )
            .await?;

            info!("starting suspend_linux binary ... ");
            let _binder = instance.connect_to_binder().unwrap();

            EventMatcher::ok().moniker(&moniker).wait::<Started>(&mut events).await.unwrap();
            EventMatcher::ok().moniker(&moniker).wait::<Stopped>(&mut events).await.unwrap();
            info!("... and it stopped.");
        }

        // Linux program started, suspended, resumed, then stopped. The resulting
        // inspect should have been captured and ought to be available.
        let kernel_inspect = ArchiveReader::inspect()
            // See notes above for moniker.
            .select_all_for_component("test_suite/kernel")
            .with_minimum_schema_count(1)
            .snapshot()
            .await?;
        print!("{:?}", kernel_inspect);
        assert_eq!(kernel_inspect.len(), 1);

        info!("tearing down the test realm now");
    }

    Ok(())
}
