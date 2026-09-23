// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use errors::IntoExitCode;
use ffx_config::EnvironmentContext;
use ffx_config::environment::ExecutableKind;
use fuchsia_async::TimeoutExt;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;

mod args_info;
mod describe;
mod ffx;
mod metrics;
mod subcommand;
mod tools;

// Re-export the top level elements
pub use args_info::{
    CliArgsInfo, ErrorCodeInfo, FlagInfo, FlagKind, Optionality, PositionalInfo, SubCommandInfo,
};

pub use ffx::{FFX_WRAPPER_INVOKE, Ffx, FfxCommandLine, check_strict_constraints};
pub use ffx_command_error::{
    Error, FfxContext, NonFatalError, Result, bug, exit_with_code, return_bug, return_user_error,
    user_error,
};
pub use metrics::{MetricsSession, analytics_command, send_enhanced_analytics};
pub use subcommand::ExternalSubToolSuite;
pub use tools::{FfxToolInfo, FfxToolSource, ToolRunner, ToolSuite};

pub use writer::Format;

fn stamp_file(stamp: &Option<String>) -> Result<Option<File>> {
    let Some(stamp) = stamp else { return Ok(None) };
    File::create(stamp)
        .with_bug_context(|| format!("Failure creating stamp file '{stamp}'"))
        .map(Some)
}

fn write_exit_code<W: Write>(res: &Result<ExitStatus>, out: &mut W) {
    let exit_code = match res {
        Ok(status) => status.code().unwrap_or(1),
        Err(err) => err.exit_code(),
    };
    write!(out, "{}\n", exit_code).ok();
}

fn get_upload_timeout(context: Option<&EnvironmentContext>) -> Duration {
    let timeout_float: f64 = context
        .and_then(|e| {
            // The reason for this not surfacing an error is that if metrics are enabled and this
            // value is malformed, then if a user successfully sets this to something like a
            // string, then we'll fail to set this back to normal using the `ffx config` command,
            // which would be a huge headache for any users. Generally we should return an error
            // when fetching a config value (in the event that it's set to something unexpected).
            //
            // So the tl;dr is if we return an error here, this would happen:
            //
            // ```shell
            // $ ffx config set metrics.upload_timeout "foobar"
            // $ ffx config set metrics.upload_timeout 10
            // ERROR!
            // ```
            //
            // And they'd be stuck!
            e.get(ffx_config::keys::METRICS_UPLOAD_TIMEOUT_KEY)
                .map_err(|e| {
                    // TODO(436934180): We shouldn't need to specify which key we couldn't fetch.
                    // This should be the config's job to format this error properly for us.
                    log::warn!(
                        "unable to load `{}`: {e}",
                        ffx_config::keys::METRICS_UPLOAD_TIMEOUT_KEY
                    )
                })
                .ok()
        })
        .unwrap_or(ffx_config::keys::METRICS_UPLOAD_TIMEOUT_DEFAULT);
    Duration::from_secs_f64(timeout_float)
}

/// Tries to report the given unexpected error to analytics if appropriate
pub async fn report_bug(env_context: Option<&EnvironmentContext>, err: &impl std::fmt::Display) {
    let upload_timeout = get_upload_timeout(env_context);
    if let Err(e) = analytics::add_crash_event(&format!("{}", err), None)
        .on_timeout(upload_timeout, || {
            log::error!("analytics timed out reporting crash event");
            Ok(())
        })
        .await
    {
        log::error!("analytics failed to submit crash event: {}", e);
    }
}

// LINT.IfChange
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MachineFormat {
    Json,
    JsonPretty,
    Raw,
}
// LINT.ThenChange(//src/testing/end_to_end/honeydew/honeydew/transports/ffx/types.py)

impl From<MachineFormat> for Option<writer::Format> {
    fn from(value: MachineFormat) -> Self {
        match value {
            MachineFormat::Json => Some(Format::Json),
            MachineFormat::JsonPretty => Some(Format::JsonPretty),
            MachineFormat::Raw => Some(Format::Raw),
        }
    }
}

impl From<writer::Format> for MachineFormat {
    fn from(value: writer::Format) -> Self {
        match value {
            Format::Json => MachineFormat::Json,
            Format::JsonPretty => MachineFormat::JsonPretty,
            Format::Raw => MachineFormat::Raw,
        }
    }
}

impl std::str::FromStr for MachineFormat {
    type Err = writer::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match Format::from_str(s) {
            Ok(f) => Ok(f.into()),
            _ => match s.to_lowercase().as_ref() {
                "raw" => Ok(MachineFormat::Raw),
                lower => Err(writer::Error::InvalidFormat(lower.into())),
            },
        }
    }
}

/// Indicates whether [`init_cmd`] discovered that we should return help info of
/// some kind. This could also be done with returning `Err(Error::Help .. )` but
/// [`run`] has to do some additional processing to return complete help output.
#[derive(Debug, Default)]
pub enum HelpState {
    /// We should return machine-readable argument info.
    ReturnArgsInfo,
    /// We should append our subcommands to this help info and return it.
    ReturnHelp { command: Vec<String>, output: String, code: i32 },
    #[default]
    None,
}

pub struct InitializedCmd {
    pub cmd: ffx::FfxCommandLine,
    pub context: ffx_config::EnvironmentContext,
    pub help_state: HelpState,
}

/// Creates an `FfxCommandLine` and `EnvironmentContext` from the environment,
/// and initializes config.
pub fn init_cmd(exe_kind: ExecutableKind) -> Result<InitializedCmd> {
    match ffx::FfxCommandLine::from_env() {
        Ok(c) => {
            let context = c.global.load_context(exe_kind)?;
            Ok(InitializedCmd { cmd: c, context, help_state: HelpState::None })
        }
        Err(Error::Help { command, output, code }) => {
            // Check for machine json output and  help
            // This is a little bit messy since the command line is not returned
            // when a help error is returned. So look for the `--machine json` flag
            // and either `help` or `--help` or `-h`.
            let argv = Vec::from_iter(std::env::args());
            let c = ffx::FfxCommandLine::from_args_for_help(&argv)?;
            let context = c.global.load_context(exe_kind)?;
            if find_machine_and_help(&c).is_some() {
                Ok(InitializedCmd { cmd: c, context, help_state: HelpState::ReturnArgsInfo })
            } else {
                Ok(InitializedCmd {
                    cmd: c,
                    context,
                    help_state: HelpState::ReturnHelp { command, output, code },
                })
            }
        }

        Err(e) => Err(e),
    }
}

/// Runs the specified tool suite.
///
/// If we're able to load the command and initialize the config without error, then `ctx_out` will
/// be set to `Some(_)`.
pub async fn run<T: ToolSuite>(icmd: InitializedCmd) -> Result<ExitStatus> {
    let InitializedCmd { cmd, context, help_state } = icmd;
    let app = &cmd.global;

    // Everything that needs to use the config must be after loading the config.
    if !context.has_no_environment() {
        context.env_file_path().map_err(|e| {
            let output = format!("ffx could not determine the environment configuration path: {}\nEnsure that $HOME is set, or pass the --env option to specify an environment configuration path", e);
            let code = 1;
            Error::Help { command: cmd.command.clone(), output, code }
        })?;
    }

    // initialize logging
    ffx_config::logging::init(&context, app.verbose, &app.log_destination)
        .map_err(anyhow::Error::from)?;

    let metrics = MetricsSession::start(&context).await?;
    log::debug!("metrics session started");

    let tools = T::from_env(&context)?;

    match help_state {
        HelpState::ReturnArgsInfo => {
            // This handles the top level ffx command information and prints the information
            // for all subcommands.
            let args = tools.get_args_info().await?;
            let output = match cmd.global.machine.unwrap() {
                MachineFormat::Json => serde_json::to_string(&args),
                MachineFormat::JsonPretty => serde_json::to_string_pretty(&args),
                MachineFormat::Raw => Ok(format!("{args:#?}")),
            };
            println!("{}", output.bug_context("Error serializing args")?);
            return Ok(ExitStatus::from_raw(0));
        }
        HelpState::ReturnHelp { command, output, code } => {
            let mut commands: String = Default::default();
            tools
                .print_command_list(&mut commands)
                .await
                .bug_context("Error getting command list")?;
            let full_output = format!("{output}\n{commands}");
            return Err(Error::Help { command, output: full_output, code });
        }
        HelpState::None => (),
    };

    // If the schema is requested, then find the tool runner for the requested tool
    // and return the schema. try_from_args() can't be used since it is common for
    // commands to have positional arguments and they are present if the schema is
    // requested.
    let tool = if app.schema && app.machine.is_some() {
        log::info!("Schema requested - calling try from name: {cmd:?}");
        tools.try_runner_from_name(&cmd).await?
    } else {
        log::info!("No schema requested - calling try from args: {cmd:?}");
        match tools.try_from_args(&cmd).await {
            Ok(t) => t,
            Err(Error::Help { command, output, code }) => {
                // TODO(b/303088345): Enhance argh to support custom help better.
                // Check for machine json output and  help.
                // This handles the sub command of ffx information.
                if let Some(machine_format) = find_machine_and_help(&cmd) {
                    let all_info = tools.get_args_info().await?;
                    // Tools will return the top level args info, so
                    //iterate over the subcommands to get to the right level
                    let mut info: CliArgsInfo = all_info;
                    for c in cmd.subcmd_iter() {
                        if c.starts_with("-") {
                            continue;
                        }
                        if info.name == c {
                            continue;
                        }
                        info = info
                            .commands
                            .iter()
                            .find(|s| s.name == c)
                            .map(|s| s.command.clone().into())
                            .unwrap_or(info);
                    }
                    let output = match machine_format {
                        MachineFormat::Json => serde_json::to_string(&info),
                        MachineFormat::JsonPretty => serde_json::to_string_pretty(&info),
                        MachineFormat::Raw => Ok(format!("{info:#?}")),
                    };
                    println!("{}", output.bug_context("Error serializing args")?);
                    return Ok(ExitStatus::from_raw(0));
                } else {
                    let command_clone = command.clone();
                    let res: Result<ExitStatus, Error> = Err(Error::Help { command, output, code });
                    let enhanced_args = match send_enhanced_analytics().await {
                        true => Some(cmd.unredacted_args_for_analytics()),
                        false => None,
                    };
                    metrics
                        .command_finished(&res, &command_clone, enhanced_args.as_deref())
                        .await
                        .ok();
                    return Err(res.unwrap_err());
                }
            }

            Err(e) => {
                let mut command_name = cmd.cmd_iter().map(|s| s.to_string()).collect::<Vec<_>>();
                if let Some(s) = cmd.subcmd_iter().next() {
                    command_name.push(s.to_string());
                }
                let res: Result<ExitStatus, Error> = Err(e);
                let enhanced_args = match send_enhanced_analytics().await {
                    true => Some(cmd.unredacted_args_for_analytics()),
                    false => None,
                };
                metrics.command_finished(&res, &command_name, enhanced_args.as_deref()).await.ok();
                return Err(res.unwrap_err());
            }
        }
    };

    log::info!("starting command: {:?}", Vec::from_iter(cmd.all_iter()));
    log::info!("with context: {kind:#?}", kind = context.env_kind());

    let stamp = stamp_file(&app.stamp)?;
    let res = match tool {
        Some(tool) => tool.run(metrics).await,
        // since we didn't run a subtool, do the metrics ourselves
        None => Err(cmd.no_handler_help(metrics, &tools).await?),
    };

    // Write to our stamp file if it was requested
    if let Some(mut stamp) = stamp {
        write_exit_code(&res, &mut stamp);
        if !context.is_isolated() {
            stamp.sync_all().bug_context("Error syncing exit code stamp write")?;
        }
    }

    res
}

/// Terminates the process, outputting errors as appropriately and with the indicated exit code.
pub async fn exit(
    env_context: Option<EnvironmentContext>,
    res: Result<ExitStatus>,
    should_format: bool,
) -> ! {
    const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
    let exit_code = res.exit_code();
    match res {
        Err(Error::Help { output, .. }) => {
            writeln!(&mut std::io::stdout(), "{output}").unwrap();
        }
        Err(err @ Error::Config(_)) | Err(err @ Error::User(_)) | Err(err @ Error::IoError(_)) => {
            // abort hard on a failure to print the user error somehow
            if should_format {
                let mut out = std::io::stdout();
                let err = SerializableError::from(&err);
                let message = serde_json::to_string(&err).unwrap();
                writeln!(&mut out, "{message}").unwrap();
            } else {
                let mut out = std::io::stderr();
                let message = format!("{err}");
                writeln!(&mut out, "{message}").unwrap();
            };
        }
        Err(err @ Error::Unexpected(_)) => {
            // abort hard on a failure to print the unexpected error somehow
            if should_format {
                let mut out = std::io::stdout();
                let err = SerializableError::from(&err);
                let message = serde_json::to_string(&err).unwrap();
                writeln!(&mut out, "{message}").unwrap();
            } else {
                let mut out = std::io::stderr();
                let message = format!("{err}");
                writeln!(&mut out, "{message}").unwrap();
                ffx_config::print_log_hint(&env_context, &mut out);
            };
            report_bug(env_context.as_ref(), &err).await;
        }
        Ok(_) | Err(Error::ExitWithCode(_)) => (),
    }

    if timeout::timeout(SHUTDOWN_TIMEOUT, fuchsia_async::emulated_handle::shut_down_handles())
        .await
        .is_err()
    {
        log::warn!("Timed out shutting down handles");
    };

    std::process::exit(exit_code);
}

/// look through the command line args for `--machine <format>`
/// and --help or help or -h. This is used to indicate the
/// JSON arg info should be returned.
fn find_machine_and_help(cmd: &FfxCommandLine) -> Option<MachineFormat> {
    if cmd.subcmd_iter().any(|c| c == "help" || c == "--help" || c == "-h") {
        cmd.global.machine
    } else {
        None
    }
}

/// We need this additional type to proxy the underlying ffx_command::Error
/// since that enum embeds anyhow::Error in it (which cannot be serialized).
/// Having a private enum we control the shape of as well as the
/// From::ffx_command::Error implementation allows us to define a schema for
/// the error messages and keep them stable across versions.
///
/// Note that we explicitly do NOT put the error chain in this enum as if that
/// information is conveyed to the caller, it becomes another API surface that
/// will become load bearing.
#[derive(Debug, thiserror::Error, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
enum SerializableError {
    #[error("{message}")]
    Unexpected { code: i32, message: String },
    #[error("{message}")]
    User { message: String, code: i32 },
    #[error("{output}")]
    Help { command: Vec<String>, output: String, code: i32 },
    #[error("{message}")]
    Config { code: i32, message: String },
    #[error("{message}")]
    IoError { message: String, code: i32 },
    #[error("Exiting with code {code}")]
    ExitWithCode { code: i32 },
}

impl From<&Error> for SerializableError {
    fn from(error: &Error) -> Self {
        match error {
            err @ Error::Unexpected(e) => {
                Self::Unexpected { code: err.exit_code(), message: format!("{}", e) }
            }
            err @ Error::IoError(e) => {
                Self::IoError { code: err.exit_code(), message: format!("{}", e) }
            }
            err @ Error::User(_) => {
                Self::User { code: err.exit_code(), message: format!("{}", err) }
            }
            err @ Error::Config(_) => {
                Self::Config { code: err.exit_code(), message: format!("{}", err) }
            }
            Error::ExitWithCode(code) => Self::ExitWithCode { code: *code },
            Error::Help { command, output, code } => {
                Self::Help { command: command.to_vec(), output: output.to_string(), code: *code }
            }
        }
    }
}

impl From<Error> for SerializableError {
    fn from(error: Error) -> Self {
        Self::from(&error)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use serde_json::json;
    use std::io::BufWriter;

    #[fuchsia::test]
    fn test_stamp_file_creation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stamp").into_os_string().into_string().ok();
        let stamp = stamp_file(&path);

        assert!(stamp.unwrap().is_some());
    }

    #[fuchsia::test]
    fn test_stamp_file_no_create() {
        let no_stamp = stamp_file(&None);
        assert!(no_stamp.unwrap().is_none());
    }

    #[fuchsia::test]
    fn test_write_exit_code() {
        let mut out = BufWriter::new(Vec::new());
        write_exit_code(&Ok(ExitStatus::from_raw(0)), &mut out);
        assert_eq!(String::from_utf8(out.into_inner().unwrap()).unwrap(), "0\n");
    }

    #[fuchsia::test]
    fn test_write_exit_code_on_failure() {
        let mut out = BufWriter::new(Vec::new());
        write_exit_code(&Result::<ExitStatus>::Err(Error::from(anyhow::anyhow!("fail"))), &mut out);
        assert_eq!(String::from_utf8(out.into_inner().unwrap()).unwrap(), "1\n")
    }

    #[fuchsia::test]
    fn test_serializable_error_from_error() -> () {
        assert_eq!(
            SerializableError::from(Error::Unexpected(anyhow::Error::msg("Cytherea".to_string()))),
            SerializableError::Unexpected { code: 1, message: "Cytherea".to_string() }
        );
        assert_eq!(
            SerializableError::from(Error::User(anyhow::Error::msg("Cytherea".to_string()))),
            SerializableError::User { code: 1, message: "Cytherea".to_string() }
        );
        assert_eq!(
            SerializableError::from(Error::User(anyhow::anyhow!(NonFatalError(anyhow::anyhow!(
                "inner error"
            ))))),
            SerializableError::User {
                code: 1,
                message: "non-fatal error encountered: inner error".to_string()
            }
        );
        assert_eq!(
            SerializableError::from(Error::Config(anyhow::Error::msg("Cytherea".to_string()))),
            SerializableError::Config { code: 1, message: "Cytherea".to_string() }
        );
        assert_eq!(
            SerializableError::from(Error::Help {
                command: vec!["Cytherea".to_string()],
                code: 134,
                output: "Hot-Sauce".to_string(),
            }),
            SerializableError::Help {
                code: 134,
                command: vec!["Cytherea".to_string()],
                output: "Hot-Sauce".to_string()
            }
        );
        assert_eq!(
            SerializableError::from(Error::ExitWithCode(123)),
            SerializableError::ExitWithCode { code: 123 }
        );
    }

    #[fuchsia::test]
    fn test_serializable_error() -> () {
        let cases = vec![
            SerializableError::Unexpected { code: 1, message: "Cytherea".to_string() },
            SerializableError::User { code: 1, message: "Cytherea".to_string() },
            SerializableError::Config { code: 1, message: "Cytherea".to_string() },
            SerializableError::Help {
                code: 134,
                command: vec!["Cytherea".to_string()],
                output: "Hot-Sauce".to_string(),
            },
            SerializableError::ExitWithCode { code: 123 },
        ];
        for case in cases {
            let output = serde_json::to_string(&case).unwrap();
            let err = format!("schema not valid {output}");
            let json: serde_json::Value = serde_json::from_str(&output).expect(&err);

            assert_eq!(json, json!(case));
        }
    }
}
