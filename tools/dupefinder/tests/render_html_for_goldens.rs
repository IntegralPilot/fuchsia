// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use camino::Utf8PathBuf;
use dupefinder::DupesReport;
use std::fs::File;
use std::io::BufWriter;

/// generate an HTML report from a JSON dump of a previous dupefinder run for testing
#[derive(Debug, FromArgs)]
struct Options {
    /// path to a JSON dump from dupefinder's --json-output flag
    #[argh(option)]
    json_dump: Utf8PathBuf,
    /// path to which to write an HTML report with findings
    #[argh(option)]
    html_output: Utf8PathBuf,
}

#[fuchsia::main]
fn main() {
    let Options { json_dump, html_output } = argh::from_env();
    let report_json = std::fs::read_to_string(&json_dump).unwrap();
    let dupes: DupesReport = serde_json::from_str(&report_json).unwrap();
    let mut out_fd = BufWriter::new(File::create(&html_output).unwrap());
    dupes.write_report(&mut out_fd).unwrap();
}
