// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_gce_tool::GceSuiteTool;
use fho::FfxTool;

#[fuchsia_async::run_singlethreaded]
async fn main() {
    GceSuiteTool::execute_tool().await
}
