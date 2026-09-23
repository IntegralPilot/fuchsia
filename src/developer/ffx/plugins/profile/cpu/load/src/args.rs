// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use std::time::Duration;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "load",
    description = "\
Collect and print CPU usage data for the specified time frame, or instruct the metrics-logger \
component to record the CPU usage data to Inspect, Trace, and/or syslog.
",
    example = "\
1) To measure the CPU load over a two second duration:

    $ ffx profile cpu load --duration 2s

    The measured CPU load from each core is printed in the following format:

        CPU 0: 0.66%
        CPU 1: 1.56%
        CPU 2: 0.83%
        CPU 3: 0.71%
        Total: 3.76%

    The valid range for each CPU load is [0-100]%. The \"Total\" value represents the summation of \
    the load percentages of all CPU cores and is valid in the range [0-100*[NUM_CPU]]%.

2) To log CPU load every 500 ms indefinitely:

    $ ffx profile cpu load start --interval 500ms

    Logged samples will be available via iquery under core/metrics-logger and via tracing in the \
    `metrics_logger` category.

3) To log CPU load every 1 second for 30 seconds with output-to-syslog enabled:

    $ ffx profile cpu load start --interval 1s -d 30s --output-to-syslog

    Logged samples will be available in syslog, via iquery under core/metrics-logger and via \
    tracing in the `metrics_logger` category.
",
    note = "\
Please specify a duration for immediate load display, or alternatively, utilize the start/stop \
subcommand to instruct the metrics-logger component to record the CPU usage data to Inspect, \
Trace, and/or syslog.
If the metrics-logger component is not available to the target, add \
`--with //src/power/metrics-logger` to fx set."
)]

pub struct CpuLoadCommand {
    #[argh(option, long = "duration", short = 'd', from_str_fn(parse_duration))]
    /// duration over which to measure and print the CPU load
    pub duration: Option<Duration>,

    #[argh(subcommand)]
    /// top-level command to instruct the metrics-logger component to record the CPU usage data
    pub subcommand: Option<SubCommand>,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
pub enum SubCommand {
    Start(StartCommand),
    Stop(StopCommand),
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
/// Start logging on the target
#[argh(subcommand, name = "start")]
pub struct StartCommand {
    #[argh(option, long = "interval", short = 's', from_str_fn(parse_duration))]
    /// interval for logging the CPU load
    pub interval: Duration,

    #[argh(switch)]
    /// toggle for logging CPU loads to syslog
    pub output_to_syslog: bool,

    #[argh(option, long = "duration", short = 'd', from_str_fn(parse_duration))]
    /// duration for which to log; if omitted, logging will continue indefinitely
    pub duration: Option<Duration>,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
/// Stop logging on the target
#[argh(subcommand, name = "stop")]
pub struct StopCommand {}

const DURATION_REGEX: &'static str = r"^(\d+)(h|m|s|ms)$";

/// Parses a Duration from string.
fn parse_duration(value: &str) -> Result<Duration, String> {
    let re = regex::Regex::new(DURATION_REGEX).unwrap();
    let captures = re
        .captures(&value)
        .ok_or_else(|| format!("Durations must be specified in the form {}", DURATION_REGEX))?;
    let number: u64 = captures[1].parse().unwrap();
    let unit = &captures[2];

    match unit {
        "ms" => Ok(Duration::from_millis(number)),
        "s" => Ok(Duration::from_secs(number)),
        "m" => Ok(Duration::from_secs(number * 60)),
        "h" => Ok(Duration::from_secs(number * 3600)),
        _ => Err(format!(
            "Invalid duration string \"{}\"; must be of the form {}",
            value, DURATION_REGEX
        )),
    }
}
