// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use analytics::add_custom_event;

/// Emit an event indicating a playground command was run.
pub async fn emit_playground_cmd_event(succeeded: bool, ty: &str) {
    let _ = add_custom_event(
        Some("ffx_playground_cmd"),
        Some(if succeeded { "success" } else { "failed" }),
        None,
        [("type", ty.to_owned().into())].into_iter().collect(),
    )
    .await;
}
