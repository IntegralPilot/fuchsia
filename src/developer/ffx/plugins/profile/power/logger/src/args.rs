// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use arg_parsing::parse_duration;
use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use std::time::Duration;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "logger",
    description = "Controls the metrics-logger component to log power. Logged power samples will \
be available in syslog, via iquery under core/metrics-logger and via tracing in the \
`metrics_logger` category.",
    example = "\
To poll power sensor every 500 ms indefinitely:

    $ ffx profile power logger start --sampling-interval 500ms

To poll power sensor every 500 ms and summarize statistics every 1 second for 30 seconds with \
output-samples-to-syslog and output-stats-to-syslog enabled:

    $ ffx profile power logger start --sampling-interval 500ms --statistics-interval 1s \
    --output-stats-to-syslog --output-samples-to-syslog -d 30s",
    note = "\
If the metrics-logger component is not available to the target, then this command will not work
properly. Add --with //src/power/metrics-logger to fx set."
)]
/// Top-level command for "ffx profile power logger".
pub struct Command {
    #[argh(subcommand)]
    pub subcommand: SubCommand,
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
    #[argh(option, long = "statistics-interval", short = 'l', from_str_fn(parse_duration))]
    /// interval for summarizing statistics; if omitted, statistics is disabled
    pub statistics_interval: Option<Duration>,

    #[argh(option, long = "sampling-interval", short = 's', from_str_fn(parse_duration))]
    /// interval for polling the sensor
    pub sampling_interval: Duration,

    #[argh(switch)]
    /// toggle for logging samples to syslog
    pub output_samples_to_syslog: bool,

    #[argh(switch)]
    /// toggle for logging statistics to syslog
    pub output_stats_to_syslog: bool,

    #[argh(option, long = "duration", short = 'd', from_str_fn(parse_duration))]
    /// duration for which to log; if omitted, logging will continue indefinitely
    pub duration: Option<Duration>,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Debug)]
/// Stop logging on the target
#[argh(subcommand, name = "stop")]
pub struct StopCommand {}
