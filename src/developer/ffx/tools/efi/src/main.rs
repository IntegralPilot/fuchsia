// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod args;
mod efi_lib;

use efi_lib::Efi;
use fho::FfxTool;

#[fuchsia_async::run_singlethreaded]
async fn main() {
    Efi::execute_tool().await
}
