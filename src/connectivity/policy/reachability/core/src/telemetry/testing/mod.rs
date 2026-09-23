// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async as fasync;
use fuchsia_inspect::reader::{
    DiagnosticsHierarchy, {self as reader},
};
use fuchsia_inspect::{Inspector, Node as InspectNode};
use futures::task::Poll;
use std::pin::pin;
use windowed_stats::experimental::testing::MockTimeMatrixClient;

// Based on `TestHelper` in src/connectivity/wlan/lib/telemetry/src/lib.rs.
pub struct TestHelper {
    pub inspector: Inspector,
    pub inspect_node: InspectNode,
    pub inspect_metadata_node: InspectNode,
    pub inspect_metadata_path: String,
    pub mock_time_matrix_client: MockTimeMatrixClient,

    // Note: keep the executor field last in the struct so it gets dropped last.
    pub exec: fasync::TestExecutor,
}

impl TestHelper {
    pub fn get_inspect_data_tree(&mut self) -> DiagnosticsHierarchy {
        let read_fut = reader::read(&self.inspector);
        let mut read_fut = pin!(read_fut);
        match self.exec.run_until_stalled(&mut read_fut) {
            Poll::Pending => {
                panic!("Unexpected pending state");
            }
            Poll::Ready(result) => result.expect("failed to get hierarchy"),
        }
    }
}

pub fn setup_test() -> TestHelper {
    let exec = fasync::TestExecutor::new_with_fake_time();
    exec.set_fake_time(fasync::MonotonicInstant::from_nanos(0));

    let inspector = Inspector::default();
    let inspect_node = inspector.root().create_child("test_stats");
    let inspect_metadata_node = inspect_node.create_child("metadata");
    let inspect_metadata_path = "root/test_stats/metadata".to_string();

    let mock_time_matrix_client = MockTimeMatrixClient::new();

    TestHelper {
        inspector,
        inspect_node,
        inspect_metadata_node,
        inspect_metadata_path,
        mock_time_matrix_client,
        exec,
    }
}
