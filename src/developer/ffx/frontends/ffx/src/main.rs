// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::{ArgsInfo, FromArgs, SubCommands};
use errors::ffx_error;
use ffx_command::{
    CliArgsInfo, Error, ExternalSubToolSuite, FfxCommandLine, FfxContext, FfxToolInfo,
    MetricsSession, Optionality, Result, ToolRunner, ToolSuite, analytics_command, return_bug,
    return_user_error, send_enhanced_analytics,
};
use ffx_config::EnvironmentContext;
use ffx_config::environment::ExecutableKind;
use ffx_lib_args::FfxBuiltIn;
use ffx_lib_sub_command::SubCommand;
use fho::FhoEnvironment;
use std::collections::HashSet;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use traceable_error::TraceableError;

/// The command to be invoked and everything it needs to invoke
struct FfxSubCommand {
    app: FfxCommandLine,
    context: EnvironmentContext,
    cmd: FfxBuiltIn,
}

/// The suite of commands FFX supports.
struct FfxSuite {
    context: EnvironmentContext,
    external_commands: std::cell::OnceCell<Result<ExternalSubToolSuite>>,
}

impl FfxSuite {
    fn get_external_commands(&self) -> &ExternalSubToolSuite {
        // unwrap() is okay because ExternalSubToolSuite::from_env() has no failure path
        self.external_commands
            .get_or_init(|| ExternalSubToolSuite::from_env(&self.context))
            .as_ref()
            .unwrap()
    }
}

#[async_trait::async_trait(?Send)]
impl ToolSuite for FfxSuite {
    fn from_env(env: &EnvironmentContext) -> Result<Self> {
        let context = env.clone();

        Ok(Self { context, external_commands: std::cell::OnceCell::new() })
    }

    fn global_command_list() -> &'static [&'static argh::CommandInfo] {
        SubCommand::COMMANDS
    }

    async fn get_args_info(&self) -> Result<ffx_command::CliArgsInfo> {
        // Determine if we're handling a subcommand, or need to collect all the info
        //from all the subcommands.
        let argv = Vec::from_iter(std::env::args());
        let cmdline0 =
            FfxCommandLine::from_args_for_help(&argv).bug_context("cmd line for help")?;
        if cmdline0.subcmd_iter().count() > 1 {
            let args = Vec::from_iter(cmdline0.global.subcommand.iter().map(String::as_str));
            let all_info = SubCommand::get_args_info();
            let mut info: Option<ffx_command::CliArgsInfo> = None;
            for c in args {
                if c.starts_with("-") {
                    continue;
                }
                if info.is_none() {
                    info = all_info
                        .commands
                        .iter()
                        .find(|s| s.name == c)
                        .map(|s| s.command.clone().into());
                } else {
                    info = info
                        .unwrap()
                        .commands
                        .iter()
                        .find(|s| s.name == c)
                        .map(|s| s.command.clone().into());
                }
            }
            let args_info = info.ok_or(ffx_command::bug!("No args info found"))?;
            return Ok(args_info);
        } else {
            // Gather information about all the subcommands, both internal and external.
            let mut seen: HashSet<&str> = HashSet::new();
            let mut info: ffx_command::CliArgsInfo = ffx_command::Ffx::get_args_info().into();
            let internal_info: ffx_command::CliArgsInfo = SubCommand::get_args_info().into();
            let external_info = self.get_external_commands().get_args_info().await?;

            // filter out duplicate commands
            for sub in &internal_info.commands {
                if !seen.contains(sub.name.as_str()) {
                    seen.insert(&sub.name);
                    info.commands.push(sub.clone());
                }
            }
            for sub in &external_info.commands {
                if !seen.contains(sub.name.as_str()) {
                    seen.insert(&sub.name);
                    info.commands.push(sub.clone());
                }
            }
            return Ok(info);
        }
    }

    async fn command_list(&self) -> Vec<FfxToolInfo> {
        let builtin_commands = SubCommand::COMMANDS.iter().copied().map(FfxToolInfo::from);

        let tools = self.get_external_commands();
        builtin_commands.chain(tools.command_list().await.into_iter()).collect()
    }

    async fn try_runner_from_name(
        &self,
        ffx_cmd: &FfxCommandLine,
    ) -> Result<Option<Box<dyn ToolRunner + '_>>> {
        let argv: Vec<_> = ffx_cmd.all_iter().map(|s| s.to_string()).collect();
        if ffx_cmd.subcmd_iter().count() > 1 {
            let args = Vec::from_iter(ffx_cmd.global.subcommand.iter().map(String::as_str));
            let all_info = SubCommand::get_args_info();

            let mut info = find_info_from_cmd(&args, &all_info.into());

            let args_info = match info {
                Some(cli_info) => cli_info,
                None => {
                    // If info is none, then it is an external command (or an unknown command)
                    let external_info = self.get_external_commands().get_args_info().await?;
                    info = find_info_from_cmd(&args, &external_info);
                    match info {
                        Some(cli_info) => cli_info,
                        None => {
                            return_bug!("No internal or external args metadata found for {args:?}")
                        }
                    }
                }
            };

            // add fake args to the command line for required parameters
            let cmd_args = match build_required_args(&args_info) {
                Ok(fake_args) => fake_args,
                Err(e) => {
                    eprintln!("{e}");
                    return self.try_from_args(&ffx_cmd).await;
                }
            };

            let mut new_argv: Vec<String> = argv.clone();
            new_argv.extend(cmd_args);

            let schema_cmdline =
                FfxCommandLine::from_args_for_help(&new_argv).bug_context("cmd line for schema")?;
            return self.try_from_args(&schema_cmdline).await;
        }
        return self.try_from_args(&ffx_cmd).await;
    }

    async fn try_from_args(
        &self,
        ffx_cmd: &FfxCommandLine,
    ) -> Result<Option<Box<dyn ToolRunner + '_>>> {
        let context = self.context.clone();
        let app = ffx_cmd.clone();
        let args = Vec::from_iter(app.global.subcommand.iter().map(String::as_str));
        match args.first().copied() {
            Some("commands") => {
                let mut output = String::new();
                self.print_command_list(&mut output).await.ok();
                let code = 0;
                Err(Error::Help { command: ffx_cmd.command.clone(), output, code })
            }
            Some(name) if SubCommand::COMMANDS.iter().any(|c| c.name == name) => {
                let cmd = FfxBuiltIn::from_args(&Vec::from_iter(ffx_cmd.cmd_iter()), &args)
                    .map_err(|err| Error::from_early_exit(&ffx_cmd.command, err))?;
                Ok(Some(Box::new(FfxSubCommand { cmd, context, app })))
            }
            Some(name)
                if SubCommand::COMMANDS
                    .iter()
                    .any(|c| name.len() == 1 && *c.short == name.chars().next().unwrap()) =>
            {
                let cmd = FfxBuiltIn::from_args(&Vec::from_iter(ffx_cmd.cmd_iter()), &args)
                    .map_err(|err| Error::from_early_exit(&ffx_cmd.command, err))?;
                Ok(Some(Box::new(FfxSubCommand { cmd, context, app })))
            }
            _ => self.get_external_commands().try_from_args(ffx_cmd).await,
        }
    }
}

#[async_trait::async_trait(?Send)]
impl ToolRunner for FfxSubCommand {
    async fn run(self: Box<Self>, metrics: MetricsSession) -> Result<ExitStatus> {
        if self.app.global.schema && !ffx_lib_suite::ffx_plugin_has_schema(&self.cmd) {
            Err(ffx_error!("Schema is not defined for this subcommand").into())
        } else if self.app.global.schema && !self.app.global.machine.is_some() {
            Err(ffx_error!("The schema flag requires the machine flag").into())
        } else {
            if !analytics_command(&self.app.unredacted_args_for_analytics().join(" ")) {
                metrics.print_notice(&mut std::io::stderr()).await?;
            }
            let redacted_args = ffx_lib_suite::ffx_plugin_redact_args(&self.app, &self.cmd);
            let enhanced_args = match send_enhanced_analytics().await {
                false => None,
                true => Some(self.app.unredacted_args_for_analytics()),
            };
            let res = run_legacy_subcommand(self.app, self.context, self.cmd)
                .await
                .map(|_| ExitStatus::from_raw(0));
            if let Err(ref e) = res {
                log::error!("[{}]: {:?}", e.diagnostic_code(), e);
            }
            metrics.command_finished(&res, &redacted_args, enhanced_args.as_deref()).await.and(res)
        }
    }
}

/// Builds a vec of arguments that are required for the given info.
/// This is used to create a command line that can be successfully
/// parsed for the info by appending the returned list to the command
/// line that represents the info.
fn build_required_args(info: &CliArgsInfo) -> Result<Vec<String>> {
    let mut args = vec![];
    if !info.commands.is_empty() {
        return_user_error!("Schema for commands with subcommands is not supported");
    }
    for f in &info.flags {
        if f.optionality == Optionality::Required {
            match &f.kind {
                ffx_command::FlagKind::Option { arg_name } => {
                    args.extend_from_slice(&[f.long.clone(), arg_name.clone()])
                }
                ffx_command::FlagKind::Switch => {
                    args.extend_from_slice(std::slice::from_ref(&f.long))
                }
            };
        }
    }
    for p in &info.positionals {
        if p.optionality == Optionality::Required {
            args.extend_from_slice(std::slice::from_ref(&p.name))
        }
    }
    Ok(args)
}

/// Finds command line info given the command line args starting with the all_info
/// collection of top level commands.
fn find_info_from_cmd(args: &[&str], all_info: &CliArgsInfo) -> Option<CliArgsInfo> {
    let mut info: Option<CliArgsInfo> = None;

    for c in args {
        // skip options
        if c.starts_with("-") {
            continue;
        }
        if info.is_none() {
            info =
                all_info.commands.iter().find(|s| s.name == *c).map(|s| s.command.clone().into());
        } else {
            info = info
                .unwrap()
                .commands
                .iter()
                .find(|s| s.name == *c)
                .map(|s| s.command.clone().into());
        }
    }
    info
}

async fn run_legacy_subcommand(
    ffx: FfxCommandLine,
    context: EnvironmentContext,
    subcommand: FfxBuiltIn,
) -> Result<()> {
    let env = FhoEnvironment::new(&context, &ffx);
    ffx_lib_suite::ffx_plugin_impl(&env, subcommand).await
}

#[fuchsia_async::run_singlethreaded]
async fn main() {
    let mut env_context = None;
    let result = match ffx_command::init_cmd(ExecutableKind::MainFfx) {
        Ok(c) => {
            env_context = Some(c.context.clone());
            ffx_command::run::<FfxSuite>(c).await
        }
        Err(e) => Err(e),
    };
    let should_format = match FfxCommandLine::from_env() {
        Ok(cli) => cli.global.should_format(),
        Err(_e) => true,
    };
    ffx_command::exit(env_context, result, should_format).await
}

#[cfg(test)]
mod test {
    use super::*;
    use ffx_command::{FlagInfo, FlagKind, PositionalInfo, SubCommandInfo};

    #[fuchsia::test]
    async fn test_try_runner_from_name() {
        let env = ffx_config::test_init().expect("test env");
        let suite = FfxSuite::from_env(&env.context).expect("ffx suite");

        let cmd = FfxCommandLine::new(None, &["ffx", "target", "list"]).expect("ffx cmdline");
        let runner = suite.try_runner_from_name(&cmd).await.expect("runner from name");
        assert!(runner.is_some());

        let cmd = FfxCommandLine::new(None, &["ffx", "no-known-cmd"]).expect("ffx cmdline");
        let runner = suite.try_runner_from_name(&cmd).await.expect("runner from name");
        assert!(runner.is_none());
    }

    #[fuchsia::test]
    async fn test_build_required_args() {
        let info = CliArgsInfo::default();
        let args = build_required_args(&info).expect("build args");
        let expected: Vec<String> = vec![];
        assert_eq!(args, expected);

        let mut required_info = CliArgsInfo::default();
        required_info.flags.push(FlagInfo {
            kind: FlagKind::Switch,
            optionality: Optionality::Required,
            long: "sw1".to_string(),
            ..Default::default()
        });
        required_info.flags.push(FlagInfo {
            kind: FlagKind::Switch,
            optionality: Optionality::Optional,
            long: "skip".to_string(),
            ..Default::default()
        });
        required_info.flags.push(FlagInfo {
            kind: FlagKind::Option { arg_name: "value".into() },
            optionality: Optionality::Required,
            long: "option".to_string(),
            ..Default::default()
        });
        required_info.positionals.push(PositionalInfo {
            name: "pos1".into(),
            optionality: Optionality::Required,
            ..Default::default()
        });
        let some_args = build_required_args(&required_info).expect("build args");
        let expected: Vec<String> =
            vec!["sw1".into(), "option".into(), "value".into(), "pos1".into()];
        assert_eq!(some_args, expected);
    }

    #[fuchsia::test]
    async fn test_find_info_from_cmd() {
        let args = ["--skip", "target", "list"];
        let all_info = CliArgsInfo::default();
        let actual = find_info_from_cmd(&args, &all_info);
        assert!(actual.is_none());

        let some_info = CliArgsInfo {
            commands: vec![SubCommandInfo {
                name: "target".into(),
                command: CliArgsInfo {
                    name: "target".into(),
                    commands: vec![SubCommandInfo {
                        name: "list".into(),
                        command: CliArgsInfo { name: "list".into(), ..Default::default() },
                    }],
                    ..Default::default()
                },
            }],
            ..Default::default()
        };
        let actual = find_info_from_cmd(&args, &some_info);
        assert!(actual.is_some());
    }
}
