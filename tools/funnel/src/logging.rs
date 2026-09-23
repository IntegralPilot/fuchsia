// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use log::LevelFilter;
use logging::{FfxLog, FfxLogSink, FormatOpts, LogSinkTrait, TargetsFilter};
use std::sync::{Arc, LazyLock, Mutex};

static LOGGING_ID: LazyLock<u64> = LazyLock::new(generate_id);

fn generate_id() -> u64 {
    rand::random::<u64>()
}

pub fn init(level: LevelFilter) -> Result<()> {
    configure_subscribers(level);

    Ok(())
}

struct OnFilter;

impl logging::Filter for OnFilter {
    fn should_emit(&self, _record: &log::Metadata<'_>) -> bool {
        true
    }
}

fn configure_subscribers(level: LevelFilter) {
    let sinks: Vec<Box<dyn LogSinkTrait>> =
        vec![FfxLogSink::new(Arc::new(Mutex::new(std::io::stdout()))).boxed()];
    let targets_filter = TargetsFilter::new(vec![]);
    let format = FormatOpts::new(*LOGGING_ID);
    let logger = FfxLog::new(sinks, format, OnFilter {}, level, targets_filter);
    let _ = log::set_boxed_logger(Box::new(logger))
        .map(|()| log::set_max_level(log::LevelFilter::Trace));
}
