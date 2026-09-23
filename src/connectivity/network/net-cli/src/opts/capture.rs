// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Argument parsing options for the packet capture CLI.

use argh::{ArgsInfo, FromArgs};

/// Subcommands available for capture.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "capture")]
/// Manage rolling packet captures
pub struct Capture {
    #[argh(subcommand)]
    pub capture_cmd: CaptureEnum,
}

#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand)]
pub enum CaptureEnum {
    /// Start a rolling packet capture.
    StartRolling(StartRollingCommand),
    /// Stop and download a rolling packet capture.
    StopRolling(StopRollingCommand),
}

/// Arguments for the `start-rolling` subcommand.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "start-rolling")]
/// Start a rolling packet capture
pub struct StartRollingCommand {
    /// name to assign to this rolling packet capture upon detaching (maximum 64 characters)
    ///
    /// If omitted, a default name will be used.
    #[argh(option)]
    pub name: Option<String>,

    /// interface to capture on
    ///
    /// Must specify one interface using a prefix format: `id:<id>`, `name:<name>`,
    /// or `ip:<address>` (e.g. `name:lo`, `id:1`, `ip:fe80::1`).
    #[argh(positional)]
    pub interface: String,

    /// pcap-filter string describing which packets are captured
    #[argh(positional)]
    pub pcap_filter: Option<String>,

    /// number of bytes from the start of each packet to save
    ///
    /// Defaults to 262144 (256 KiB) if 0 or absent.
    #[argh(option)]
    pub snap_len: Option<u32>,

    /// buffer size in bytes for the rolling capture
    ///
    /// Must be between 262144 (256 KiB) and 16777216 (16 MiB).
    /// Defaults to 2097152 (2 MiB) if 0 or absent.
    #[argh(option)]
    pub capture_size: Option<u32>,
}

/// Arguments for the `stop-rolling` subcommand.
#[derive(ArgsInfo, FromArgs, Clone, Debug, PartialEq)]
#[argh(subcommand, name = "stop-rolling")]
/// Stop and download a packet capture
pub struct StopRollingCommand {
    /// name of the detached rolling packet capture to stop
    ///
    /// If omitted, the default name will be used.
    #[argh(option)]
    pub name: Option<String>,

    /// destination file path to save the pcapng file
    ///
    /// If omitted, defaults to `/tmp/pcap/{name}.pcapng`.
    /// Cannot be used with --skip-download.
    #[argh(option)]
    pub output: Option<String>,

    /// stop the capture and discard the data without downloading
    ///
    /// Cannot be used with --output.
    #[argh(switch)]
    pub skip_download: bool,
}
