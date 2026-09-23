// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_starnix::StarnixTool;
use fho::FfxTool;

#[fuchsia_async::run_singlethreaded]
async fn main() {
    StarnixTool::execute_tool().await
}
