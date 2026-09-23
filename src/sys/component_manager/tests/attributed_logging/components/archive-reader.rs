// Copyright 2020 the Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use diagnostics_data::ExtendedMoniker;
use diagnostics_reader::ArchiveReader;
use fuchsia_async::TimeoutExt;
use futures::stream::StreamExt;
use log::info;
use std::collections::HashMap;

#[fuchsia::main(logging_tags = ["archive-reader"])]
async fn main() {
    let reader = ArchiveReader::logs();
    let mut non_matching_logs = vec![];

    type Fingerprint = Vec<&'static str>;
    let mut treasure = HashMap::<ExtendedMoniker, Vec<Fingerprint>>::new();
    treasure.insert(
        "routing-tests/offers-to-children-unavailable/child-for-offer-from-parent"
            .try_into()
            .unwrap(),
        vec![vec![
            "protocol `fidl.test.components.Trigger`",
            "not available for target `child-for-offer-from-parent`",
            "cannot offer protocol fidl.test.components.Trigger from parent at root/routing-tests/offers-to-children-unavailable because parent does not offer the capability",
        ]],
    );
    treasure.insert(
        "routing-tests/offers-to-children-unavailable-but-optional/child-for-offer-from-parent"
            .try_into()
            .unwrap(),
        vec![vec![
            "Optional",
            "protocol `fidl.test.components.Trigger`",
            "not available for target `child-for-offer-from-parent`",
            "cannot offer protocol fidl.test.components.Trigger from parent at root/routing-tests/offers-to-children-unavailable-but-optional because parent does not offer the capability",
        ]],
    );
    treasure.insert(
        "routing-tests/child".try_into().unwrap(),
        vec![vec![
            "protocol `fidl.test.components.Trigger`",
            "not available for target `child`",
            "cannot use protocol fidl.test.components.Trigger from parent at root/routing-tests/child because parent does not offer the capability",
        ]],
    );
    treasure.insert(
        "routing-tests/child-with-optional-use".try_into().unwrap(),
        vec![vec![
            "Optional",
            "protocol `fidl.test.components.Trigger`",
            "not available for target `child-with-optional-use`",
            "cannot use protocol fidl.test.components.Trigger from parent at root/routing-tests/child-with-optional-use because parent does not offer the capability",
        ]],
    );
    treasure.insert(
        "routing-tests/offers-to-children-unavailable/child-for-offer-from-sibling".try_into().unwrap(),
        vec![vec![
            "protocol `fidl.test.components.Trigger`",
            "not available for target `child-for-offer-from-sibling`",
            "cannot offer protocol fidl.test.components.Trigger from child child-that-doesnt-expose at root/routing-tests/offers-to-children-unavailable because child child-that-doesnt-expose does not expose the capability",
        ]],
    );
    treasure.insert(
        "routing-tests/offers-to-children-unavailable-but-optional/child-for-offer-from-sibling".try_into().unwrap(),
        vec![vec![
            "Optional",
            "protocol `fidl.test.components.Trigger`",
            "not available for target `child-for-offer-from-sibling`",
            "cannot offer protocol fidl.test.components.Trigger from child child-that-doesnt-expose at root/routing-tests/offers-to-children-unavailable-but-optional because child child-that-doesnt-expose does not expose the capability",
        ]],
    );
    treasure.insert(
        "routing-tests/offers-to-children-unavailable/child-open-unrequested".try_into().unwrap(),
        vec![vec![
            "No capability available",
            "fidl.test.components.Trigger",
            "root/routing-tests/offers-to-children-unavailable/child-open-unrequested",
            "`use` declaration",
        ]],
    );
    treasure.insert(
        "routing-tests/offers-to-children-unavailable-but-optional/child-open-unrequested"
            .try_into()
            .unwrap(),
        vec![vec![
            "No capability available",
            "fidl.test.components.Trigger",
            "root/routing-tests/offers-to-children-unavailable-but-optional/child-open-unrequested",
            "`use` declaration",
        ]],
    );

    if let Ok(mut results) = reader.snapshot_then_subscribe() {
        while let Some(Ok(log_record)) =
            results.next().on_timeout(std::time::Duration::from_millis(5000), || None).await
        {
            if let Some(log_str) = log_record.msg() {
                match treasure.get_mut(&log_record.moniker) {
                    None => non_matching_logs.push(log_record),
                    Some(log_fingerprints) => {
                        let removed = {
                            let print_count = log_fingerprints.len();
                            log_fingerprints.retain(|fingerprint| {
                                // If all the part of the fingerprint match, remove
                                // the fingerprint, otherwise keep it.
                                let has_all_features =
                                    fingerprint.iter().all(|feature| log_str.contains(feature));
                                !has_all_features
                            });

                            print_count != log_fingerprints.len()
                        };

                        // If there are no more fingerprint sets for this
                        // component, remove it
                        if log_fingerprints.is_empty() {
                            treasure.remove(&log_record.moniker);
                        }
                        // If we didn't remove any fingerprints, this log didn't
                        // match anything, so push it into the non-matching logs.
                        if !removed {
                            non_matching_logs.push(log_record);
                        }
                        if treasure.is_empty() {
                            return;
                        }
                    }
                }
            }
        }
    }

    // Assert that no logs were received for optional-use-from-void.
    for log in &non_matching_logs {
        if log.moniker == "routing-tests/optional-use-from-void".try_into().unwrap() {
            panic!("Received unexpected log for optional-use-from-void: {:?}", log);
        }
    }

    if !treasure.is_empty() {
        info!("One or more logs that we expected were not found. These were missing:");
        for (moniker, fingerprint) in treasure {
            info!("- from {moniker}: {fingerprint:?}");
        }
        info!("\n");
    }

    if !non_matching_logs.is_empty() {
        info!("One or more logs were read that were unexpected. These were found:");
        for log in non_matching_logs {
            info!("- from {}: {:?}", log.moniker, log.msg());
        }
    }

    panic!("observed logs did not match expectations");
}
