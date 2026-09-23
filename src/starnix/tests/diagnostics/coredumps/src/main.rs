// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use component_events::events::{EventStream, Stopped};
use component_events::matcher::EventMatcher;
use diagnostics_reader::ArchiveReader;
use fuchsia_component_test::ScopedInstance;
use log::info;
use parse_starnix_inspect::CoredumpReport;

#[fuchsia::main]
async fn main() {
    // Iterate for a bit more than the max number of core dumps to check the rollout behavior.
    let report_capacity = 64usize;
    let max_idx = report_capacity + 8;

    let mut events = EventStream::open().await.unwrap();
    let collection = "coredumps";
    let child_name = "coredumper";
    let url = "#meta/coredump.cm";
    let moniker = format!("{collection}:{child_name}");

    for current_idx in 0..max_idx {
        let mut instance =
            ScopedInstance::new_with_name(child_name.into(), collection.into(), url.into())
                .await
                .unwrap();

        info!("starting coredump instance...");
        instance.start_with_binder_sync().await.unwrap();

        info!("waiting for coredumper to stop...");
        EventMatcher::ok().moniker(&moniker).wait::<Stopped>(&mut events).await.unwrap();

        // The "earliest"/lowest index should be either 0 have advanced by how many were rolled out.
        let expected_min_idx = current_idx.saturating_sub(report_capacity - 1);
        let expected_len = (current_idx + 1).min(report_capacity);

        // Now that starnix has reported the component as stopped to the framework, we can expect
        // the kernel's inspect to have the last coredump.
        info!("retrieving coredump reports...");
        let observed_coredumps = loop {
            let observed_coredumps = get_coredumps_from_inspect().await.unwrap();

            // There should be as many reports as loop iterations unless we're at capacity.
            if observed_coredumps.len() == expected_len
                && observed_coredumps.last().unwrap().idx == current_idx
            {
                break observed_coredumps;
            } else {
                // Retry until we have the correct number of coredumps.
                continue;
            }
        };

        info!("observed coredump {current_idx} in inspect, validating...");
        let mut expected_idx = expected_min_idx;
        for coredump in observed_coredumps {
            assert_eq!(coredump.idx, expected_idx);
            assert!(
                coredump.pid >= expected_idx as i64 + 3,
                "coredumps starts at pid greater or equals to 3, nothing else runs here",
            );
            assert_eq!(
                coredump.argv,
                "data/tests/generate_linux_coredump AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA AAAAAAAAAAAAAAAAAAAAAAAA...",
                "coredump reports should truncate argv when its too long",
            );
            expected_idx += 1;
        }

        info!("destroying child before another iteration of the test loop...");
        let on_destroy = instance.take_destroy_waiter();
        drop(instance);
        on_destroy.await.unwrap();
    }
}

// Snapshotting inspect can be a bit racy -- over enough runs we'll end up reading the node
// hierarchy at points where the node exists but none of its children, where nodes exist but not
// one of their properties, etc. Return an Option here for cases where we couldn't actually read the
// coredump report so the caller can try again..
async fn get_coredumps_from_inspect() -> anyhow::Result<Vec<CoredumpReport>> {
    let kernel_inspect = ArchiveReader::inspect()
        .select_all_for_component("kernel")
        .with_minimum_schema_count(1)
        .snapshot()
        .await?;
    assert_eq!(kernel_inspect.len(), 1);
    CoredumpReport::extract_from_snapshot(&kernel_inspect[0])
        .context("extracting coredump from snapshot")
}
