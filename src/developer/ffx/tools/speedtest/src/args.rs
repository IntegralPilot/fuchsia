// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::num::NonZeroU32;
use std::time::Duration;

use argh::{ArgsInfo, FromArgs};
use fdomain_fuchsia_developer_ffx_speedtest as fspeedtest;
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "speedtest",
    description = "Test ffx latency and throughput between the host and target."
)]
pub struct SpeedtestCommand {
    /// test the legacy Overnet connectivity.
    #[argh(switch)]
    pub overnet: bool,
    /// how many times to repeat the test. Set zero to run until interrupted.
    #[argh(option, short = 'r', default = "1")]
    pub repeat: usize,
    /// time to delay between repetitions, in milliseconds.
    #[argh(
        option,
        short = 'I',
        from_str_fn(duration_from_millis),
        default = "Duration::from_secs(1)"
    )]
    pub delay: Duration,
    #[argh(subcommand)]
    pub cmd: Subcommand,
}

fn duration_from_millis(value: &str) -> Result<Duration, String> {
    u64::from_str_radix(value, 10)
        .map(Duration::from_millis)
        .map_err(|_| format!("failed to parse milliseconds value from '{value}'"))
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum Subcommand {
    Ping(Ping),
    Socket(Socket),
}

/// Calculates latency to the target with simple channel messages.
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "ping")]
pub struct Ping {
    /// the number of probes to send to calculate average latency.
    #[argh(option, short = 'c', default = "NonZeroU32::new(10).unwrap()")]
    pub count: NonZeroU32,
}

const DEFAULT_TRANSFER_MB: NonZeroU32 =
    NonZeroU32::new(fspeedtest::DEFAULT_TRANSFER_SIZE / 1_000_000).unwrap();
const DEFAULT_BUFFER_KB: NonZeroU32 =
    NonZeroU32::new(fspeedtest::DEFAULT_BUFFER_SIZE >> 10).unwrap();

/// Calculates throughput to the target using zircon socket abstractions.
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "socket")]
pub struct Socket {
    /// transfer size in Mega Bytes (MB).
    #[argh(option, short = 'L', default = "DEFAULT_TRANSFER_MB")]
    pub transfer_mb: NonZeroU32,
    /// buffer size in Kilo Bytes (KiB).
    #[argh(option, short = 'b', default = "DEFAULT_BUFFER_KB")]
    pub buffer_kb: NonZeroU32,
    /// perform target->host transfer. host->target transfer is performed by
    /// default.
    #[argh(switch, short = 'R')]
    pub rx: bool,
    /// suppress the use of FDomain's streaming read API and instead receive
    /// data by making individual read requests. This means more round trips and
    /// less efficient use of the stream.
    #[argh(switch)]
    pub fdomain_individual_reads: bool,
    /// how many writes to send via FDomain at a time before waiting for a
    /// response. More writes in flight means better chance of saturating the
    /// outgoing stream. Defaults to enough to send the whole transfer at once.
    #[argh(option)]
    pub fdomain_writes_in_flight: Option<NonZeroU32>,
}
