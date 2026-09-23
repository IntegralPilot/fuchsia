// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use ffx_build_version::build_info;
use ffx_usb_host_driver::run;

#[fuchsia_async::run_singlethreaded]
async fn main() {
    // There are some symbols that the build system uses to examine FFX plugin
    // binaries and get metadata from them. Calling this here prevents them from
    // being garbage collected by the linker.
    let _build_info = build_info();
    run().await;
}
