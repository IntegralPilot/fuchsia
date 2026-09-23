// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ffx_product_tool_lib::ProductSuiteTool;
use fho::FfxTool;

#[fuchsia_async::run_singlethreaded]
async fn main() {
    ProductSuiteTool::execute_tool().await
}
