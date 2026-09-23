// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async as fasync;
use fuchsia_component::escrow::EscrowOperation;

use log::error;
use std::process;

/// This component immediately escrows its outgoing directory and then exits.
#[fuchsia::main]
fn main() {
    let _executor = fasync::LocalExecutor::default();

    let Some(outgoing_directory) =
        fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleType::DirectoryRequest.into())
    else {
        error!("No outgoing directory server endpoint received, exiting.");
        process::abort();
    };
    EscrowOperation::new().run(outgoing_directory.into()).expect("failed to escrow outgoing dir");
}
