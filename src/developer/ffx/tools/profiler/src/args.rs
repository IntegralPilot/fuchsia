// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
/// Interact with the profiling subsystem.
#[argh(subcommand, name = "profiler")]
pub struct ProfilerCommand {
    #[argh(subcommand)]
    pub sub_cmd: ProfilerSubCommand,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
#[argh(subcommand)]
pub enum ProfilerSubCommand {
    Attach(Attach),
    Launch(Launch),
    Symbolize(Symbolize),
    DownloadAndroidSymbols(DownloadAndroidSymbols),
    Stop(Stop),
    Status(Status),
}

#[derive(Clone, Debug, PartialEq)]
pub enum UnwindStrategy {
    FramePointer,
    Dwarf,
}

impl Default for UnwindStrategy {
    fn default() -> Self {
        UnwindStrategy::FramePointer
    }
}

impl FromStr for UnwindStrategy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "fp" => Ok(UnwindStrategy::FramePointer),
            "dwarf" => Ok(UnwindStrategy::Dwarf),
            _ => Err(format!("invalid unwind strategy '{}', valid options are 'fp', 'dwarf'", s)),
        }
    }
}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// Profile a running task or component
#[argh(subcommand, name = "attach")]
#[derive(Default)]
pub struct Attach {
    /// url of a component to profile. If there is no matching component, wait for one to appear.
    #[argh(option)]
    pub url: Option<String>,

    /// buffer size in MiB to profile. Specifies the amount of memory allocated for storing profiling information.
    #[argh(option)]
    pub buffer_size_mb: Option<u64>,

    /// moniker of a component to profile. If there is no matching component, the profiler will
    #[argh(option)]
    pub moniker: Option<String>,

    /// pids to profile
    #[argh(option)]
    pub pids: Vec<u64>,

    /// tids to profile
    #[argh(option)]
    pub tids: Vec<u64>,

    /// jobs to profile
    #[argh(option)]
    pub job_ids: Vec<u64>,

    /// profile everything running on the system. Equivalent to profiling the root job and
    /// everything running under it.
    #[argh(switch)]
    pub system_wide: bool,

    /// how long to profiler for. If unspecified, will interactively wait until <ENTER> is pressed.
    #[argh(option)]
    pub duration: Option<u64>,

    /// name of output trace file. Defaults to "profile.pb".
    #[argh(option)]
    pub output: Option<String>,

    /// print stats about how the profiling session went
    #[argh(switch)]
    pub print_stats: bool,

    /// if false, output the raw sample file instead of attempting to symbolize it
    #[argh(option, default = "true")]
    pub symbolize: bool,

    /// if false, output the raw symbolized sample file instead of attempting to convert to the
    /// pprof format. Ignored if --symbolize is false.
    #[argh(option, default = "true")]
    pub pprof_conversion: bool,

    /// how frequently to take a sample
    #[argh(option, default = "10000")]
    pub sample_period_us: u64,

    /// if true, include color codes in output. Defaults to true if terminal output is
    /// detected, else false
    #[argh(option, default = "std::io::stdout().is_terminal()")]
    pub color_output: bool,

    /// run the profiler session in the background
    #[argh(switch)]
    pub background: bool,

    /// unwinding strategy to use. Options are "fp" and "dwarf".
    /// fp: uses on-device frame pointers for unwinding.
    /// dwarf: uses off-device DWARF unwinding. Enable this to profile binaries
    /// compiled without frame pointers, such as 32 bit starnix containers.
    #[argh(option, default = "UnwindStrategy::FramePointer")]
    pub unwind_strategy: UnwindStrategy,

    /// run the profiler session when the device reboots.
    #[argh(switch)]
    pub on_boot: bool,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// Record a profile.
#[argh(subcommand, name = "launch")]
#[derive(Default)]
pub struct Launch {
    /// url of a component to launch and profile
    #[argh(option)]
    pub url: String,

    /// buffer size in MiB to profile. Specifies the amount of memory allocated for storing profiling information.
    #[argh(option)]
    pub buffer_size_mb: Option<u64>,

    /// moniker of a component to attach to and profile. If specified in combination with `--url`,
    /// will attempt to launch the component at the given moniker.
    #[argh(option)]
    pub moniker: Option<String>,

    /// how long in seconds to profile for. If unspecified, will interactively wait until <ENTER> is pressed.
    #[argh(option)]
    pub duration: Option<u64>,

    /// name of output trace file. Defaults to "profile.pb".
    #[argh(option)]
    pub output: Option<String>,

    /// print stats about how the profiling session went
    #[argh(switch)]
    pub print_stats: bool,

    /// if false, output the raw sample file instead of attempting to symbolize it
    #[argh(option, default = "true")]
    pub symbolize: bool,

    /// if false, output the raw symbolized sample file instead of attempting to convert to the
    /// pprof format. Ignored if --symbolize is false.
    #[argh(option, default = "true")]
    pub pprof_conversion: bool,

    /// how frequently to take a sample. This is the time interval between samples, in
    /// microseconds. The default is 10,000 microseconds (10 ms).
    #[argh(option, default = "10000")]
    pub sample_period_us: u64,

    /// the package being launched is a test to be launched via test_manager
    #[argh(switch)]
    pub test: bool,

    /// test case filters to apply to profiled tests
    #[argh(option)]
    pub test_filters: Vec<String>,

    /// if true, include color codes in output. Defaults to true if terminal output is
    /// detected, else false
    #[argh(option, default = "std::io::stdout().is_terminal()")]
    pub color_output: bool,

    /// run the profiler session in the background
    #[argh(switch)]
    pub background: bool,

    /// unwinding strategy to use. Options are "fp" and "dwarf".
    /// fp: uses on-device frame pointers for unwinding.
    /// dwarf: uses off-device DWARF unwinding. Enable this to profile binaries
    /// compiled without frame pointers, such as 32 bit starnix containers.
    #[argh(option, default = "UnwindStrategy::FramePointer")]
    pub unwind_strategy: UnwindStrategy,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// Symbolize a previously-recorded profile that was not symbolized.
#[argh(subcommand, name = "symbolize")]
#[derive(Default)]
pub struct Symbolize {
    /// path to the unsymbolized text file
    #[argh(positional)]
    pub input: PathBuf,

    /// path to which to write the symbolized pprof file
    #[argh(positional)]
    pub output: PathBuf,

    /// if false, output the raw symbolized sample file instead of attempting to convert to the
    /// pprof format.
    #[argh(option, default = "true")]
    pub pprof_conversion: bool,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// Download Android debug symbols from Android Build using fetch_artifact.
#[argh(subcommand, name = "download-android-symbols")]
#[derive(Default)]
pub struct DownloadAndroidSymbols {
    /// build id of the Android target (e.g. 11000000 or P023423)
    // We cannot use build_id here because some args could have clashes if we don't watch out, but bid is standard for fetch_artifact
    #[argh(option)]
    pub bid: String,

    /// target name of the Android build (e.g. aosp_arm64-userdebug, etc.)
    #[argh(option)]
    pub target: String,
}
#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// Stop a background profiling session and download results.
#[argh(subcommand, name = "stop")]
#[derive(Default)]
pub struct Stop {
    /// path to save the profile
    #[argh(option)]
    pub output: Option<String>,

    /// abort the session without saving the profile data
    #[argh(switch)]
    pub abort: bool,

    /// whether to try to symbolize the profile using the debug symbol index
    #[argh(option, default = "true")]
    pub symbolize: bool,

    /// if false, output the raw symbolized sample file instead of attempting to convert to the
    /// pprof format.
    #[argh(option, default = "true")]
    pub pprof_conversion: bool,

    /// if true, include color codes in output. Defaults to true if terminal output is
    /// detected, else false
    #[argh(option, default = "std::io::stdout().is_terminal()")]
    pub color_output: bool,

    /// print stats to stdout
    #[argh(switch)]
    pub print_stats: bool,
}

#[derive(ArgsInfo, FromArgs, PartialEq, Clone, Debug)]
/// List active profiling sessions.
#[argh(subcommand, name = "status")]
#[derive(Default)]
pub struct Status {}
