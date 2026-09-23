// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::FfxExecutor;
use std::path::PathBuf;
use std::string::ToString;

pub struct StrictContext {
    ffx_path: PathBuf,
    log_output_location: LogOutputLocation,
    config: Config,
}

pub enum LogOutputLocation {
    Stdout,
    Stderr,
    File(PathBuf),
}

impl ToString for LogOutputLocation {
    fn to_string(&self) -> String {
        match self {
            Self::Stdout => "stdout".to_owned(),
            Self::Stderr => "stderr".to_owned(),
            Self::File(p) => p.to_string_lossy().to_string(),
        }
    }
}

// A config from key values (dot-delimited. No need for multiple levels) to strings.
pub type Config = std::collections::HashMap<String, String>;

/// Creates a strict context for running ffx --strict commands.
///
/// An example execution of the command looks like the following:
/// ffx --strict
///     --log-output stdout
///     --machine json
///     --config emu.instance_dir=./Fuchsia/ffx/emu/instances
///     --config ssh.priv="$HOME/.ssh/fuchsia_ed25519"
///     emu stop
///
impl StrictContext {
    pub fn new(ffx_path: PathBuf, log_output_location: LogOutputLocation, config: Config) -> Self {
        Self { ffx_path, log_output_location, config }
    }
}

impl FfxExecutor for StrictContext {
    type Error = crate::CommandConstructionError;

    fn make_ffx_cmd(
        &self,
        args: &[&str],
    ) -> std::result::Result<std::process::Command, Self::Error> {
        if args.is_empty() {
            return Err(crate::CommandConstructionError::EmptyArguments);
        }
        if args[0].starts_with("--") {
            return Err(crate::CommandConstructionError::FirstArgumentIsFlag);
        }
        let ffx_canonical_path = std::fs::canonicalize(&self.ffx_path)?;
        let mut ffx_root_path = ffx_canonical_path.clone();
        // Canonicalized path should already have appended at least one directory, so this
        // cannot be an empty directory.
        ffx_root_path.pop();
        let mut strict_args = vec![
            "--strict".to_owned(),
            "--log-output".to_owned(),
            self.log_output_location.to_string(),
            "--machine".to_owned(),
            "json".to_owned(),
        ];
        for (config_key, config_value) in self.config.iter() {
            strict_args
                .extend(vec!["--config".to_owned(), format!("{}={}", config_key, config_value)]);
        }
        let strict_args = strict_args.iter().map(|s| s.as_str()).collect::<Vec<_>>();
        let full_args = [&strict_args[..], &args[..]].concat();
        // It might be necessary to run this using the env_context and `rerun_prefix`.
        let mut cmd = std::process::Command::new(ffx_canonical_path);
        cmd.args(&full_args[..]);
        Ok(cmd)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn strict_ctx_fails_with_bad_params() {
        let ctx = StrictContext::new(
            PathBuf::from_str("wherever/ffx").unwrap(),
            LogOutputLocation::Stderr,
            Config::new(),
        );
        let res = ctx.make_ffx_cmd(&[]);
        assert!(res.is_err(), "expected error, instead got {res:?}");
        let res = ctx.make_ffx_cmd(&["--foo", "echo"]);
        assert!(res.is_err(), "expected error, instead got {res:?}");
    }
}
