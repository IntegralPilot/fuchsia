// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use argh::ArgsInfo;
use ffx_command::FfxCommandLine;
use std::sync::OnceLock;

///! A minimal setup for setting the command line context for diagnostics analytics.
///! This is all thread-safe. This is intended to only be used via `get_command_line`
///! in the analytics library so code can fail in arbitrary locations and we can locate
///! the command line info.

static FFX_COMMAND_LINE: OnceLock<Vec<String>> = OnceLock::new();

/// Sets the GLOBAL ffx command line info. This is idempotent.
pub fn set_command_line_context<C: ArgsInfo>(ffx: &FfxCommandLine, subcmd: &C) {
    let ctx = ffx.redact_subcmd_for_enhanced_analytics(subcmd);
    log::info!("Setting command line ctx for analytics: {ctx:?}");
    let _ = FFX_COMMAND_LINE.set(ctx);
}

/// Gets the ffx command line info.
pub fn get_command_line() -> Option<Vec<String>> {
    let res = FFX_COMMAND_LINE.get().cloned();
    log::debug!("Analytics command line ctx grabbed: {res:?}");
    res
}
