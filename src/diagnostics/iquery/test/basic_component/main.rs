// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_inspect::component;
use fuchsia_inspect::health::Reporter;
use inspect_runtime::PublishOptions;

#[fuchsia::main(logging_tags = [ "iquery_basic_component" ])]
async fn main() {
    let inspector = component::inspector();
    inspector.root().record_string("iquery", "rocks");
    component::health().set_ok();
    let task = inspect_runtime::publish(inspector, PublishOptions::default());
    task.unwrap().await;
}
