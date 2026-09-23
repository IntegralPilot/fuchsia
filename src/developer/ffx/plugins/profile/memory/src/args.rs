// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, bail};
use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;
use ffx_profile_memory_sub_command::SubCommand;
use std::str::FromStr;

#[derive(Debug, PartialEq)]
pub enum Backend {
    // Read from memory monitor 1 if available, and fallback to memory monitor 2.
    Default,
    // Read memory monitor or fail. This is legacy behavior.
    MemoryMonitor1,
    // Read memory monitor or fail.
    MemoryMonitor2,
}
impl FromStr for Backend {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "default" => Ok(Backend::Default),
            "memory_monitor_1" => Ok(Backend::MemoryMonitor1),
            "memory_monitor_2" => Ok(Backend::MemoryMonitor2),
            _ => bail!(
                "Unable to parse backend: {}. Value should be one of 'default', 'memory_monitor_1', or 'memory_monitor_2'.",
                s
            ),
        }
    }
}

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "memory", description = "Query memory related information")]
pub struct MemoryCommand {
    #[argh(subcommand)]
    pub subcommand: Option<SubCommand>,

    #[argh(
        switch,
        description = "outputs the json returned by memory_monitor. For debug purposes only, \
        no guarantee is made on the stability of the output of this command."
    )]
    pub debug_json: bool,

    #[argh(option, description = "filters by process koids. Repeatable flag.")]
    pub process_koids: Vec<u64>,

    #[argh(option, description = "filters by process names (exact match). Repeatable flag.")]
    pub process_names: Vec<String>,

    #[argh(
        option,
        description = "repeats the command at the given interval (in seconds) until terminated."
    )]
    pub interval: Option<f64>,

    #[argh(switch, description = "prints a bucketized digest of the memory usage.")]
    pub buckets: bool,

    #[argh(
        switch,
        description = "displays the detailed view of only the undigested memory \
        (memory not part of any bucket) instead of the full memory."
    )]
    pub undigested: bool,

    #[argh(
        switch,
        description = "outputs csv that for every process shows the device uptime in seconds, \
        the process koid, the process name, and the private, scale, and total memory usage. \
        This option is not supported with other output options like --machine."
    )]
    pub csv: bool,

    #[argh(
        switch,
        description = "outputs the exact byte sizes, as opposed to a human-friendly format. \
        Does not impact machine oriented outputs, such as CSV and JSON outputs."
    )]
    pub exact_sizes: bool,

    #[argh(switch, description = "loads the unprocessed memory information as json from stdin.")]
    pub stdin_input: bool,

    #[argh(
        option,
        default = "Backend::Default",
        long = "backend",
        description = "selects where to read the memory information from. \
        'default', 'memory_monitor_1', or 'memory_monitor_2' are supported."
    )]
    pub backend: Backend,
}
