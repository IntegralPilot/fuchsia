// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{test_topology, utils};
use diagnostics_reader::{ArchiveReader, Data, Inspect};
use fidl_fuchsia_archivist_test as ftest;

#[fuchsia::test]
async fn accessor_truncation_test() {
    let letters = ['a', 'b'];
    let puppets = itertools::iproduct!(0..3, letters.iter())
        .map(|(i, x)| test_topology::PuppetDeclBuilder::new(format!("child_{x}{i}")).into())
        .collect();
    let realm_proxy = test_topology::create_realm(ftest::RealmOptions {
        puppets: Some(puppets),
        ..Default::default()
    })
    .await
    .expect("create base topology");

    let mut writers = vec![];
    for (i, x) in itertools::iproduct!(0..3, letters.iter()) {
        let puppet =
            test_topology::connect_to_puppet(&realm_proxy, &format!("child_{x}{i}")).await.unwrap();

        let writer = puppet
            .create_inspector(&ftest::InspectPuppetCreateInspectorRequest::default())
            .await
            .unwrap()
            .into_proxy();

        writer.emit_example_inspect_data().await.unwrap();
        writers.push(writer);
    }

    let accessor = utils::connect_accessor(&realm_proxy, utils::ALL_PIPELINE).await;
    let mut reader = ArchiveReader::inspect();
    reader.with_archive(accessor);
    let data = reader
        .with_aggregated_result_bytes_limit(1)
        .add_selector("child_a*:root")
        .with_minimum_schema_count(3)
        .snapshot()
        .await
        .expect("got inspect data");

    assert_eq!(data.len(), 3);

    assert_eq!(count_dropped_schemas_per_moniker(&data, "child_a"), 3);

    const MAX_EXPECTED_BYTES_FOR_3_COMPONENTS: u64 = 8000;

    // Use the default format with this read. At HEAD this will be CBOR, but when frozen for CTF
    // we will fall back to JSON.
    //
    // Leave enough room for the result of 3 components in this read and the following one.
    let data = reader
        .with_aggregated_result_bytes_limit(MAX_EXPECTED_BYTES_FOR_3_COMPONENTS)
        .add_selector("child_a*:root")
        .with_minimum_schema_count(3)
        .snapshot()
        .await
        .expect("got inspect data");

    assert_eq!(data.len(), 3);

    assert_eq!(count_dropped_schemas_per_moniker(&data, "child_a"), 0);

    // Force use of JSON for this read, ensuring that we still fit within the limit.
    let data = reader
        .with_format(fidl_fuchsia_diagnostics::Format::Json)
        .with_aggregated_result_bytes_limit(MAX_EXPECTED_BYTES_FOR_3_COMPONENTS)
        .add_selector("child_a*:root")
        .with_minimum_schema_count(3)
        .snapshot()
        .await
        .expect("got inspect data");

    assert_eq!(data.len(), 3);

    assert_eq!(count_dropped_schemas_per_moniker(&data, "child_a"), 0);

    let data = reader
        .with_aggregated_result_bytes_limit(1)
        .add_selector("child_b*:root")
        .add_selector("child_a*:root")
        .with_minimum_schema_count(6)
        .snapshot()
        .await
        .expect("got inspect data");

    assert_eq!(data.len(), 6);
    assert_eq!(count_dropped_schemas_per_moniker(&data, "child_a"), 3);
    assert_eq!(count_dropped_schemas_per_moniker(&data, "child_b"), 3);

    // Similar to the above reads, ensure that we leave enough room for 6 components' output.
    let data = reader
        .with_aggregated_result_bytes_limit(2 * MAX_EXPECTED_BYTES_FOR_3_COMPONENTS)
        .add_selector("child_b*:root")
        .add_selector("child_a*:root")
        .with_minimum_schema_count(6)
        .snapshot()
        .await
        .expect("got inspect data");

    assert_eq!(data.len(), 6);
    assert_eq!(count_dropped_schemas_per_moniker(&data, "child_a"), 0);
    assert_eq!(count_dropped_schemas_per_moniker(&data, "child_b"), 0);

    let data = reader
        .with_format(fidl_fuchsia_diagnostics::Format::Json)
        .with_aggregated_result_bytes_limit(2 * MAX_EXPECTED_BYTES_FOR_3_COMPONENTS)
        .add_selector("child_b*:root")
        .add_selector("child_a*:root")
        .with_minimum_schema_count(6)
        .snapshot()
        .await
        .expect("got inspect data");

    assert_eq!(data.len(), 6);
    assert_eq!(count_dropped_schemas_per_moniker(&data, "child_a"), 0);
    assert_eq!(count_dropped_schemas_per_moniker(&data, "child_b"), 0);
}

fn count_dropped_schemas_per_moniker(data: &[Data<Inspect>], moniker: &str) -> i64 {
    let mut dropped_schema_count = 0;
    for data_entry in data {
        if !data_entry.moniker.to_string().contains(moniker) {
            continue;
        }
        if let Some(errors) = &data_entry.metadata.errors {
            assert!(
                data_entry.payload.is_none(),
                "shouldn't have payloads when errors are present."
            );
            assert_eq!(
                errors[0].message, "Schema failed to fit component budget.",
                "Accessor truncation test should only produce one error."
            );
            dropped_schema_count += 1;
        }
    }
    dropped_schema_count
}
