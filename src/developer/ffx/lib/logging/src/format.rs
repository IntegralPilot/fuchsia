// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use std::io::{Result, Write};

const TIME_FORMAT: &str = "%b %d %H:%M:%S%.3f";

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq)]
pub struct LogTimer;

impl LogTimer {
    fn time_formatted(&self) -> String {
        chrono::Local::now().format(TIME_FORMAT).to_string()
    }
}

/// Implements a compact formatter which also knows about tags.
#[allow(dead_code)]
#[derive(Default)]
pub struct FormatOpts {
    pub(crate) id: u64,
    pub(crate) display_thread_id: bool,
    pub(crate) display_filename: bool,
    pub(crate) display_line_number: bool,
    display_target: bool,
    tags: Vec<String>,
    pub(crate) timer: LogTimer,
}

impl FormatOpts {
    pub fn new(id: u64) -> Self {
        FormatOpts {
            id,
            display_target: true,
            display_filename: true,
            display_line_number: true,
            ..Default::default()
        }
    }
}

pub fn format_record<W: Write>(
    opts: &FormatOpts,
    writer: &mut W,
    record: &log::Record<'_>,
) -> Result<()> {
    write!(writer, "{}", opts.timer.time_formatted())?;

    let meta = record.metadata();

    write!(writer, "[{:0>20?}] ", opts.id)?;
    let level = match meta.level() {
        log::Level::Trace => "TRACE ",
        log::Level::Debug => "DEBUG ",
        log::Level::Info => "INFO ",
        log::Level::Warn => "WARN ",
        log::Level::Error => "ERROR ",
    };
    write!(writer, "{}", level)?;

    if opts.display_thread_id {
        write!(writer, "{:0>2?} ", std::thread::current().id())?;
    }

    if opts.display_target {
        write!(writer, "{}: ", meta.target())?;
    }

    let line_number = if opts.display_line_number { record.line() } else { None };

    if opts.display_filename {
        if let Some(filename) = record.file() {
            let _ =
                write!(writer, "{}:{}", filename, if line_number.is_some() { "" } else { " " })?;
        }
    }

    if let Some(line_number) = line_number {
        write!(writer, "{}: ", line_number)?;
    }

    write!(writer, "{}", record.args())
}
