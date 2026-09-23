// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use ffx_lib_args::FfxBuiltIn;
use fho::FhoEnvironment;
pub fn ffx_plugin_supports_structured_output() -> bool {
    unreachable!("This is a side effect needed for the jinja template")
}

pub fn ffx_plugin_has_schema() -> bool {
    unreachable!("This is a side effect needed for the jinja template")
}

#[allow(clippy::unused_async)]
pub async fn ffx_plugin_impl(_: &FhoEnvironment, _: FfxBuiltIn) -> Result<()> {
    unreachable!("This is a side effect needed for the jinja template")
}
