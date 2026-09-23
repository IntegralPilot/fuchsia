// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs};
use ffx_core::ffx_command;

#[ffx_command()]
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "doctor",
    description = "Run common checks for the ffx tool and host environment",
    example = "To run diagnostics:

    $ ffx doctor

To capture the output and additional logs:

    $ ffx doctor --record

By default, this outputs the zip in the current directory. Enabling `--record`
also sets the `--verbose` flag to true.

To override output dir:

    $ ffx doctor --record --output-dir /tmp/ffx",
    note = "The `doctor` subcommand automatically attempts to repair common target
interaction issues and provides useful diagnostic information to the user.

The default `retry_delay` is '2000' milliseconds."
)]
pub struct DoctorCommand {
    #[argh(switch, description = "generates an output zip file with logs")]
    pub record: bool,

    #[argh(switch, description = "do not include the ffx configuration file")]
    pub no_config: bool,

    #[argh(
        option,
        default = "2000",
        description = "timeout delay in ms during connection attempt"
    )]
    pub retry_delay: u64,

    #[argh(
        option,
        description = "deprecated: number of times to retry failed connection attempts",
        hidden_help
    )]
    pub retry_count: Option<usize>,

    #[argh(
        switch,
        description = "deprecated: force restart the daemon, even if the connection is working",
        hidden_help
    )]
    pub restart_daemon: bool,

    #[argh(switch, short = 'v', description = "verbose, display all steps")]
    pub verbose: bool,

    #[argh(option, description = "override the default output directory for doctor records")]
    pub output_dir: Option<String>,

    #[argh(
        switch,
        description = "checks SSH key consistency and repairs them if needed. This may cause any devices to be reflashed."
    )]
    pub repair_keys: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deprecated_args_parse() {
        let cmd =
            DoctorCommand::from_args(&["doctor"], &["--restart-daemon", "--retry-count", "5"])
                .unwrap();
        assert!(cmd.restart_daemon);
        assert_eq!(cmd.retry_count, Some(5));
    }
}
