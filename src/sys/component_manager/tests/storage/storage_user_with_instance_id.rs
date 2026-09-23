// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::*;
use std::path::PathBuf;

#[fuchsia::main]
fn main() {
    // Write a file to the storage directory of this component
    let expected_content = "hippos_are_neat";
    write("/data/hippo", expected_content).unwrap();

    // This component's instance id in `component_id_index_for_debug.json5`
    let component_instance_id = "30f79a42f42300a635c8e04f92002e992368a4947199244554cdb5ec0c023be0";
    let file_path: PathBuf = ["/memfs", component_instance_id, "hippo"].iter().collect();

    // Read the file back from the global storage path and compare it
    let actual_content = read_to_string(file_path).unwrap();
    assert_eq!(actual_content, expected_content);
}
