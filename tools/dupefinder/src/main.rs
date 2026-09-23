// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context as _;
use argh::FromArgs;
use camino::Utf8PathBuf;
use chrono::{DateTime, Local};
use dupefinder::{read_heap_contents_dir, DupesReport, Sample};
use log::debug;
use std::fs::File;
use std::io::BufWriter;
use std::time::SystemTime;

/// report on duplicate allocations in a heap dump
#[derive(FromArgs)]
struct Options {
    /// path to a protobuf output file from `ffx profile heapdump ... --symbolize`
    #[argh(option)]
    profile: Utf8PathBuf,

    /// path to the contents directory produced by `ffx profile heapdump`
    #[argh(option)]
    contents_dir: Utf8PathBuf,

    /// path to which to write an HTML report with findings
    #[argh(option)]
    html_output: Option<Utf8PathBuf>,

    /// path to which to write a JSON dump of findings
    #[argh(option)]
    json_output: Option<Utf8PathBuf>,

    /// maximum number of allocations whose contents and blame sources should be included in report
    #[argh(option, default = "100")]
    max_allocs_reported: usize,

    /// minimum number of duplicated bytes for a blamed source location to be included in report
    #[argh(option, default = "512")]
    dupe_threshold: usize,

    /// whether to enable debug-level output
    #[argh(switch, short = 'v')]
    verbose: bool,
}

#[fuchsia::main]
fn main() -> Result<(), anyhow::Error> {
    let Options {
        profile,
        contents_dir,
        html_output,
        json_output,
        max_allocs_reported,
        dupe_threshold,
        verbose,
    } = argh::from_env();
    if verbose {
        log::set_max_level(log::LevelFilter::Debug);
    }

    if html_output.is_none() && json_output.is_none() {
        anyhow::bail!("Must specify JSON and/or HTML output path(s).");
    }

    debug!("Parsing allocation profile...");
    let profile_bytes = std::fs::read(&profile).context("reading profile")?;
    let samples = Sample::parse(&profile_bytes).context("parsing profile")?;

    debug!("Reading heap contents...");
    let heap_contents = read_heap_contents_dir(&contents_dir).context("reading heap contents")?;

    debug!("Identifying duplicate allocations...");
    let dupes = DupesReport::new(
        &heap_contents,
        samples,
        max_allocs_reported,
        dupe_threshold,
        DateTime::<Local>::from(SystemTime::now()).to_rfc3339(),
    )
    .context("creating duplicates report")?;
    dupes.summarize();

    if let Some(html_output) = html_output {
        debug!("Writing HTML report to {html_output}...");
        let mut out_fd = BufWriter::new(
            File::create(&html_output).with_context(|| format!("creating {html_output}"))?,
        );
        dupes.write_report(&mut out_fd).context("rendering and writing report")?;
        debug!("Wrote HTML report");
    }

    if let Some(json_output) = json_output {
        debug!("Writing JSON dump to {json_output}");
        let json =
            serde_json::to_string_pretty(&dupes).context("serializing dupes report to JSON")?;
        std::fs::write(json_output, json).context("writing JSON report to disk")?;
        debug!("Wrote JSON dump");
    }

    Ok(())
}
