// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use agents::{EnvironmentSource, SystemEnvironment, is_invoked_by_agent};
use async_trait::async_trait;
use diagnostics_data::{BuilderArgs, LogsDataBuilder, LogsProperty, Severity};
use error::LogError;
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_diagnostics::{LogSettingsMarker, LogSettingsProxy, StreamParameters};
use fdomain_fuchsia_diagnostics_host::ArchiveAccessorMarker;
use fdomain_fuchsia_sys2::RealmQueryProxy;
use ffx_config::EnvironmentContext;
use ffx_log_args::LogCommand;
use ffx_log_command_output::CommandOutputMachineWriter;
use ffx_writer::ToolIO;
use fho::{FfxMain, FfxTool};
use futures::future::select;
use futures::{FutureExt, select};
use log_command_fdomain::{
    BootTimeAccessor, DefaultLogFormatter, LogData, LogEntry, LogProcessingResult, LogSubCommand,
    Symbolize, Timestamp, WriterContainer, dump_logs_from_socket,
};
use std::io::Write;
use std::pin::pin;
use target_connector::Connector;
use target_holders::RemoteControlProxyHolder;
use tokio::signal::ctrl_c;
use transactional_symbolizer::{RealSymbolizerProcess, TransactionalSymbolizer};

mod condition_variable;
mod error;
mod mutex;
mod transactional_symbolizer;

trait Clock: Send + Sync {
    async fn sleep(&self, duration: std::time::Duration);
}

struct RealClock;
impl Clock for RealClock {
    async fn sleep(&self, duration: std::time::Duration) {
        fuchsia_async::Timer::new(duration).await
    }
}

#[cfg(test)]
mod testing_utils;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(FfxTool)]
pub struct LogTool {
    #[command]
    cmd: LogCommand,
    rcs_connector: Connector<RemoteControlProxyHolder>,
    context: EnvironmentContext,
}

struct NoOpSymoblizer;

#[async_trait(?Send)]
impl Symbolize for NoOpSymoblizer {
    async fn symbolize(&self, entry: LogEntry) -> Option<LogEntry> {
        Some(entry)
    }
}

fho::embedded_plugin!(LogTool);

#[async_trait::async_trait(?Send)]
impl FfxMain for LogTool {
    type Writer = CommandOutputMachineWriter;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        Box::pin(log_impl(writer, &self.context, self.cmd, self.rcs_connector, true)).await?;
        Ok(())
    }
}

// Main entrypoint called from other plugins
pub async fn log_impl(
    writer: impl ToolIO<OutputItem = LogEntry> + Write + 'static,
    ctx: &EnvironmentContext,
    mut cmd: LogCommand,
    rcs_connector: Connector<RemoteControlProxyHolder>,
    include_timestamp: bool,
) -> Result<(), LogError> {
    // TODO(b/333908164): We have 3 different flags that all do the same thing.
    // Remove them when possible.
    let color_config: bool = ctx.get(ffx_config::keys::LOG_CMD_COLOR).unwrap_or(true);
    if !color_config || ctx.is_strict() {
        cmd.set_no_color(true);
    }

    let symbolize_disabled = cmd.symbolize().is_symbolize_disabled();
    let prettification_disabled = cmd.symbolize().is_prettification_disabled();
    log_main(
        writer,
        cmd,
        if symbolize_disabled {
            None
        } else {
            Some(TransactionalSymbolizer::new(RealSymbolizerProcess::new(
                ctx,
                !prettification_disabled,
            )?)?)
        },
        rcs_connector,
        include_timestamp,
        RealClock,
        SystemEnvironment,
    )
    .await
}

// Main logging event loop.
async fn log_main<W, C, E>(
    writer: W,
    cmd: LogCommand,
    symbolizer: Option<impl Symbolize>,
    rcs_connector: Connector<RemoteControlProxyHolder>,
    include_timestamp: bool,
    clock: C,
    env: E,
) -> Result<(), LogError>
where
    W: ToolIO<OutputItem = LogEntry> + Write + 'static,
    C: Clock + 'static,
    E: EnvironmentSource,
{
    let formatter = DefaultLogFormatter::<W>::new_from_args(&cmd, writer)?;
    let future = log_loop(cmd, formatter, symbolizer, rcs_connector, include_timestamp, clock, env);
    select! {
        res = future.fuse() => res,
        _ = ctrl_c().fuse() => Ok(()),
    }
}

struct DeviceConnection {
    boot_timestamp: u64,
    log_socket: fdomain_client::Socket,
    log_settings_client: LogSettingsProxy,
    boot_id: Option<u64>,
    realm_query: RealmQueryProxy,
    was_reboot: bool,
}

async fn connect_to_rcs(
    rcs_connector: &Connector<RemoteControlProxyHolder>,
) -> fho::Result<RemoteControlProxyHolder> {
    rcs_connector.try_connect_indefinitely(|_target, _err| Ok(())).await
}

// TODO(https://fxbug.dev/42080003): Remove this once Overnet
// has support for reconnect handling.
async fn connect_to_target(
    stream_mode: &mut fdomain_fuchsia_diagnostics::StreamMode,
    prev_boot_id: Option<u64>,
    rcs_connector: &Connector<RemoteControlProxyHolder>,
) -> Result<DeviceConnection, LogError> {
    // Connect to device
    let rcs_client = connect_to_rcs(rcs_connector).await?;
    let host_id = rcs_client.identify_host().await??;

    let boot_timestamp = host_id.boot_timestamp_nanos.ok_or(LogError::NoBootTimestamp)?;
    let boot_id = host_id.boot_id;
    let realm_query =
        rcs::root_realm_query(&rcs_client, TIMEOUT).await.map_err(anyhow::Error::from)?;
    // If we detect a reboot we want to SnapshotThenSubscribe so
    // we get all of the logs from the reboot. If not, we use Snapshot
    // to avoid getting duplicate logs.
    let was_reboot = match prev_boot_id {
        Some(id) if Some(id) == boot_id => {
            // Reconnect detected, subscribe.
            *stream_mode = fdomain_fuchsia_diagnostics::StreamMode::Subscribe;
            false
        }
        Some(_) => {
            // Device rebooted
            *stream_mode = fdomain_fuchsia_diagnostics::StreamMode::SnapshotThenSubscribe;
            true
        }
        _ => false,
    };
    // Connect to ArchiveAccessor
    let diagnostics_client =
        rcs::toolbox::connect_with_timeout::<ArchiveAccessorMarker>(&rcs_client, TIMEOUT)
            .await
            .map_err(anyhow::Error::from)?;
    // Connect to LogSettings
    let log_settings_client =
        rcs::toolbox::connect_with_timeout::<LogSettingsMarker>(&rcs_client, TIMEOUT)
            .await
            .map_err(anyhow::Error::from)?;
    // Setup stream
    let (local, remote) = rcs_client.domain().create_stream_socket();
    diagnostics_client
        .stream_diagnostics(
            &StreamParameters {
                data_type: Some(fdomain_fuchsia_diagnostics::DataType::Logs),
                stream_mode: Some(*stream_mode),
                format: Some(fdomain_fuchsia_diagnostics::Format::Json),
                client_selector_configuration: Some(
                    fdomain_fuchsia_diagnostics::ClientSelectorConfiguration::SelectAll(true),
                ),
                ..Default::default()
            },
            remote,
        )
        .await?;
    Ok(DeviceConnection {
        boot_timestamp,
        log_socket: local,
        log_settings_client,
        boot_id,
        realm_query,
        was_reboot,
    })
}

async fn log_loop<W, C, E>(
    mut cmd: LogCommand,
    mut formatter: DefaultLogFormatter<W>,
    symbolizer: Option<impl Symbolize>,
    rcs_connector: Connector<RemoteControlProxyHolder>,
    include_timestamp: bool,
    clock: C,
    env: E,
) -> Result<(), LogError>
where
    W: ToolIO<OutputItem = LogEntry> + Write,
    C: Clock,
    E: EnvironmentSource,
{
    let symbolizer_channel: Box<dyn Symbolize> = match symbolizer {
        Some(inner) => Box::new(inner),
        None => Box::new(NoOpSymoblizer {}),
    };
    let disable_reconnect = cmd.disable_reconnect();
    let mut stream_mode = get_stream_mode(cmd.clone())?;
    // TODO(https://fxbug.dev/42080003): Add support for reconnect handling to Overnet.
    // This plugin needs special logic to handle reconnects as logging should tolerate
    // a device rebooting and remaining in a consistent state (automatically) after the reboot.
    // Eventually we should have direct support for this in Overnet, but for now we have to
    // handle reconnects manually.
    let mut prev_boot_id = None;
    let is_ai = is_invoked_by_agent(&env);
    loop {
        let connection;
        let mut backoff = 0;
        let mut last_error = None;
        if disable_reconnect && prev_boot_id.is_some() {
            return Ok(());
        }
        loop {
            if matches!(stream_mode, fdomain_fuchsia_diagnostics::StreamMode::Snapshot) && is_ai {
                let connection_fut =
                    pin!(connect_to_target(&mut stream_mode, prev_boot_id, &rcs_connector));
                let timeout_fut = pin!(clock.sleep(std::time::Duration::from_secs(10)));
                match select(connection_fut, timeout_fut).await {
                    futures::future::Either::Left((Ok(a), _)) => {
                        connection = a;
                        break;
                    }
                    _ => return Err(LogError::AIAgentTimedOut),
                }
            } else {
                let maybe_connection =
                    connect_to_target(&mut stream_mode, prev_boot_id, &rcs_connector).await;
                if let Ok(connected) = maybe_connection {
                    connection = connected;
                    break;
                }
                backoff += 1;
                if backoff > 10 {
                    backoff = 10;
                }
                let err = maybe_connection.err().unwrap();
                if matches!(err, LogError::FidlError(fidl::Error::ClientChannelClosed { .. })) {
                    continue;
                }
                let err = format!("{:?}", err);
                if matches!(&last_error, Some(value) if *value == err) {
                    eprintln!("Error connecting to device, retrying in {backoff} seconds.");
                } else {
                    if err.contains("FFX Daemon was told not to autostart and no existing Daemon instance was found") {
                        return Err(LogError::DaemonRetriesDisabled);
                    }
                    eprintln!(
                        "Error connecting to device, retrying in {backoff} seconds. Error: {err}",
                    );
                    last_error = Some(err);
                }
                clock.sleep(std::time::Duration::from_secs(backoff)).await;
            }
        }
        let prev_boot_id_for_logging = prev_boot_id;
        prev_boot_id = connection.boot_id;

        formatter.expand_monikers(&connection.realm_query).await?;

        for warning in cmd.validate_cmd_flags_with_warnings()? {
            writeln!(formatter.writer().stderr(), "{warning}")?;
        }

        cmd.maybe_set_interest(&connection.log_settings_client, &connection.realm_query).await?;
        if let Some(LogSubCommand::SetSeverity(ref options)) = cmd.sub_command {
            if options.no_persist {
                // Block forever.
                futures::future::pending::<()>().await;
            } else {
                // Interest persisted, exit.
                return Ok(());
            }
        }
        formatter.set_boot_timestamp(Timestamp::from_nanos(
            connection.boot_timestamp.try_into().unwrap(),
        ));
        if connection.was_reboot {
            // Generate synthetic reboot message
            let mut builder = LogsDataBuilder::new(BuilderArgs {
                component_url: Some("ffx".into()),
                moniker: "ffx".try_into().unwrap(),
                severity: Severity::Info,
                timestamp: Timestamp::from_nanos(formatter.get_boot_timestamp().into_nanos()),
            })
            .set_message("Target device rebooted");
            if let Some(prev_boot_id) = prev_boot_id_for_logging {
                builder =
                    builder.add_key(LogsProperty::Uint("previous_boot_id".into(), prev_boot_id));
            }
            if let Some(current_boot_id) = connection.boot_id {
                builder =
                    builder.add_key(LogsProperty::Uint("current_boot_id".into(), current_boot_id));
            }
            formatter
                .push_unfiltered_log(LogEntry { data: LogData::TargetLog(builder.build()) })
                .await?;
        }
        let result = dump_logs_from_socket(
            connection.log_socket,
            &mut formatter,
            symbolizer_channel.as_ref(),
            include_timestamp,
        )
        .await;
        if stream_mode == fdomain_fuchsia_diagnostics::StreamMode::Snapshot {
            break;
        }
        match result {
            Ok(LogProcessingResult::Exit) => {
                break;
            }
            Ok(LogProcessingResult::Continue) => {}
            Err(err) => {
                if err.is_broken_pipe() {
                    break;
                }
                writeln!(formatter.writer().stderr(), "{err}")?;
            }
        }
    }
    Ok(())
}

fn get_stream_mode(cmd: LogCommand) -> Result<fdomain_fuchsia_diagnostics::StreamMode, LogError> {
    let is_dump = matches!(cmd.sub_command, Some(LogSubCommand::Dump(..)));
    let stream_mode = if is_dump {
        if cmd.since().map(|value| value.is_now).unwrap_or(false) {
            return Err(LogError::DumpWithSinceNow);
        }
        fdomain_fuchsia_diagnostics::StreamMode::Snapshot
    } else {
        cmd.since()
            .map(|value| {
                if value.is_now {
                    fdomain_fuchsia_diagnostics::StreamMode::Subscribe
                } else {
                    fdomain_fuchsia_diagnostics::StreamMode::SnapshotThenSubscribe
                }
            })
            .unwrap_or(fdomain_fuchsia_diagnostics::StreamMode::SnapshotThenSubscribe)
    };
    Ok(stream_mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing_utils::{TestEnvironment, TestEnvironmentConfig, TestEvent};
    use assert_matches::assert_matches;
    use chrono::{Local, TimeZone};
    use diagnostics_data::{BuilderArgs, LogsDataBuilder, Severity, Timestamp};
    use fdomain_fuchsia_diagnostics::StreamMode;
    use ffx_log_command_output::CommandOutput;
    use ffx_writer::{Format, TestBuffers, VerifiedMachineWriter};
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use log_command_fdomain::{
        LogData, LogFilterArgs, OneOrMany, RawDumpCommand, RawWatchCommand, SymbolizeMode,
        TIMESTAMP_FORMAT, TimeFormat, parse_seconds_string_as_duration, parse_time, parse_utc_time,
    };
    use moniker::Moniker;
    use selectors::parse_log_interest_selector;

    const TEST_STR: &str = "[1980-01-01 00:00:03.000][ffx] INFO: Hello world!\u{1b}[m\n";
    const RECONNECT_STR: &str = "[1970-01-01 00:00:00.000][ffx] INFO: Target device rebooted current_boot_id=42 previous_boot_id=1\u{1b}[m\n[1980-01-01 00:00:03.000][ffx] INFO: Hello world!\u{1b}[m\n";
    const RECONNECT_STR_WITH_42_SECONDS: &str = "[1970-01-01 00:00:42.000][ffx] INFO: Target device rebooted current_boot_id=42 previous_boot_id=1\u{1b}[m\n[1980-01-01 00:00:45.000][ffx] INFO: Hello world!\u{1b}[m\n";
    const BOOT_TIMESTAMP: u64 = 57575757;

    async fn check_for_message(buffers: &TestBuffers, msg: &str) {
        let mut actual = String::new();
        loop {
            actual.push_str(&buffers.stdout.clone().into_string());
            let expected = msg;
            if actual != expected {
                buffers.stdout.wait_ready().await;
            } else {
                break;
            }
        }
    }

    struct FakeEnv {
        vars: std::collections::HashMap<String, String>,
    }

    impl Default for FakeEnv {
        fn default() -> Self {
            Self { vars: Default::default() }
        }
    }

    impl EnvironmentSource for FakeEnv {
        fn has_var(&self, key: &str) -> bool {
            self.vars.contains_key(key)
        }
    }

    struct FakeClock;

    impl Clock for FakeClock {
        async fn sleep(&self, _duration: std::time::Duration) {
            fasync::Timer::new(std::time::Duration::from_millis(1)).await
        }
    }

    async fn logger_dump_string_with_env_clock<C: Clock + 'static, E: EnvironmentSource>(
        config: TestEnvironmentConfig,
        cmd: LogCommand,
        clock: C,
        env: E,
    ) -> Result<String, fho::Error> {
        let environment = TestEnvironment::new(config).await;
        let rcs_connector = environment.rcs_connector().await;
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);

        let result =
            log_main(writer, cmd, None::<NoOpSymoblizer>, rcs_connector, false, clock, env).await;

        match result {
            Ok(()) => Ok(buffers.into_stdout_str()),
            Err(e) => Err(e.into()),
        }
    }

    impl LogTool {
        async fn main_no_timestamp(
            self,
            writer: <LogTool as fho::FfxMain>::Writer,
        ) -> fho::Result<()> {
            Box::pin(log_impl(writer, &self.context, self.cmd, self.rcs_connector, false)).await?;
            Ok(())
        }
    }

    #[fuchsia::test]
    async fn json_logger_test() {
        let environment = TestEnvironment::new(TestEnvironmentConfig::default()).await;
        let rcs_connector = environment.rcs_connector().await;
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            filters: LogFilterArgs { symbolize: Some(SymbolizeMode::Off), ..Default::default() },
            ..LogCommand::default()
        };
        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(Some(Format::Json), &buffers);

        assert_matches!(tool.main_no_timestamp(writer).await, Ok(()));
        let output = buffers.into_stdout_str();

        assert_eq!(
            serde_json::from_str::<LogEntry>(&output).unwrap(),
            LogEntry {
                data: LogData::TargetLog(
                    LogsDataBuilder::new(BuilderArgs {
                        component_url: Some("ffx".into()),
                        moniker: "host/ffx".try_into().unwrap(),
                        severity: Severity::Info,
                        timestamp: Timestamp::from_nanos(0),
                    })
                    .set_pid(1)
                    .set_tid(2)
                    .set_message("Hello world!")
                    .build(),
                ),
            }
        );
        let value: serde_json::Value = serde_json::from_str(&output).expect("parsing json");
        if let Err(e) = VerifiedMachineWriter::<CommandOutput>::verify_schema(&value) {
            panic!("Failed to verify JSON schema output of `{value:#?}`: {e}")
        }
    }

    #[fuchsia::test]
    async fn logger_prints_error_if_ambiguous_selector() {
        let environment = TestEnvironment::new(TestEnvironmentConfig {
            instances: vec![
                Moniker::try_from("core/some/ambiguous_selector:thing/test").unwrap(),
                Moniker::try_from("core/other/ambiguous_selector:thing/test").unwrap(),
            ],
            ..Default::default()
        })
        .await;
        let rcs_connector = environment.rcs_connector().await;
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            set_severity: vec![OneOrMany::One(
                parse_log_interest_selector("ambiguous_selector#INFO").unwrap(),
            )],
            filters: LogFilterArgs { symbolize: Some(SymbolizeMode::Off), ..Default::default() },
            ..LogCommand::default()
        };

        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);

        let error = format!("{}", tool.main_no_timestamp(writer).await.unwrap_err());

        const EXPECTED_INTEREST_ERROR: &str = r#"WARN: One or more of your selectors appears to be ambiguous
and may not match any components on your system.

If this is unintentional you can explicitly match using the
following command:

ffx log \
	--set-severity core/other/ambiguous_selector\\:thing/test#INFO \
	--set-severity core/some/ambiguous_selector\\:thing/test#INFO

If this is intentional, you can disable this with
ffx log --force-set-severity.
"#;
        assert_eq!(error, EXPECTED_INTEREST_ERROR);
    }

    async fn logger_dump_string(config: TestEnvironmentConfig, cmd: LogCommand) -> String {
        let show_initial_timestamp = config.show_initial_timestamp;
        let mut environment = TestEnvironment::new(config).await;
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            filters: LogFilterArgs { symbolize: Some(SymbolizeMode::Off), ..cmd.filters },
            ..cmd
        };

        let rcs_connector = environment.rcs_connector().await;
        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);
        let mut event_stream = environment.take_event_stream().unwrap();
        if show_initial_timestamp {
            assert_matches!(tool.main(writer).await, Ok(()));
        } else {
            assert_matches!(tool.main_no_timestamp(writer).await, Ok(()));
        }
        assert_matches!(event_stream.next().await, Some(TestEvent::LogSettingsClosed));
        buffers.into_stdout_str()
    }

    async fn logger_dump_test(
        config: TestEnvironmentConfig,
        cmd: LogCommand,
        expected_output: &str,
    ) {
        assert_eq!(Box::pin(logger_dump_string(config, cmd)).await, expected_output);
    }

    #[fuchsia::test]
    async fn logger_times_out_under_ai_agent_in_dump_mode() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("ANTIGRAVITY_AGENT".into(), "1".into());
        let env = FakeEnv { vars };
        let clock = FakeClock;

        let config = TestEnvironmentConfig { hang_device_connection: true, ..Default::default() };
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            filters: LogFilterArgs { symbolize: Some(SymbolizeMode::Off), ..Default::default() },
            ..LogCommand::default()
        };

        let result = Box::pin(logger_dump_string_with_env_clock(config, cmd, clock, env)).await;

        assert_matches!(result, Err(fho::Error::User(err)) => {
            assert_matches!(err.downcast_ref::<LogError>(), Some(LogError::AIAgentTimedOut));
        });
    }

    #[fuchsia::test]
    async fn logger_retries_under_ai_agent_in_non_dump_mode() {
        let mut vars = std::collections::HashMap::new();
        vars.insert("ANTIGRAVITY_AGENT".into(), "1".into());
        let env = FakeEnv { vars };
        let clock = FakeClock;

        let config = TestEnvironmentConfig { fail_device_connection: true, ..Default::default() };
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Watch(RawWatchCommand::default())),
            filters: LogFilterArgs { symbolize: Some(SymbolizeMode::Off), ..Default::default() },
            ..LogCommand::default()
        };

        let mut environment = TestEnvironment::new(config).await;
        let rcs_connector = environment.rcs_connector().await;
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);

        let handle = fasync::Task::local(log_main(
            writer,
            cmd,
            None::<NoOpSymoblizer>,
            rcs_connector,
            false,
            clock,
            env,
        ));

        // Yield to allow the first connection attempt to execute and fail.
        fasync::Timer::new(std::time::Duration::from_nanos(1)).await;

        // Configure subsequent connection attempts to succeed.
        environment.set_fail_device_connection(false);

        // Use select with a short timer to verify the subscription hangs (stalls).
        let timeout_fut = fasync::Timer::new(std::time::Duration::from_millis(1));
        match futures::future::select(handle, timeout_fut).await {
            futures::future::Either::Left((Ok(()), _)) => {
                panic!("Successfully connected, but subscription should not end!");
            }
            futures::future::Either::Left((Err(e), _)) => {
                panic!("Task failed with error: {:?}", e);
            }
            futures::future::Either::Right(((), _)) => {
                // Task correctly stalled on log subscription.
            }
        }
    }

    #[fuchsia::test]
    async fn logger_sets_interest_if_one_match() {
        let selectors = vec![OneOrMany::One(parse_log_interest_selector("core/foo#INFO").unwrap())];
        let mut environment = TestEnvironment::new(TestEnvironmentConfig {
            instances: vec![Moniker::try_from("core/foo").unwrap()],
            ..Default::default()
        })
        .await;
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            set_severity: selectors.clone(),
            filters: LogFilterArgs { symbolize: Some(SymbolizeMode::Off), ..Default::default() },
            ..LogCommand::default()
        };
        let rcs_connector = environment.rcs_connector().await;
        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);
        let mut event_stream = environment.take_event_stream().unwrap();

        assert_matches!(tool.main_no_timestamp(writer).await, Ok(()));
        assert_eq!(
            event_stream.next().await,
            Some(TestEvent::SetInterest(selectors.into_iter().flatten().collect()))
        );
    }

    #[fuchsia::test]
    async fn logger_prints_error_if_both_dump_and_since_now_are_combined() {
        let environment = TestEnvironment::new(TestEnvironmentConfig::default()).await;
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            filters: LogFilterArgs {
                symbolize: Some(SymbolizeMode::Off),
                since: Some(parse_time("now").unwrap()),
                ..Default::default()
            },
            ..LogCommand::default()
        };
        let rcs_connector = environment.rcs_connector().await;
        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);

        let result = tool.main_no_timestamp(writer).await;
        assert_matches!(result, Err(fho::Error::User(err)) => {
            assert_matches!(err.downcast_ref::<LogError>(), Some(LogError::DumpWithSinceNow));
        });
    }

    #[fuchsia::test]
    async fn test_no_symbolizer_config_message() {
        let err = LogError::NoSymbolizerConfig;
        let msg = format!("{}", err);
        assert_eq!(
            msg,
            "No symbolizer configuration provided. You can provide one via config, or run 'ffx log --symbolize off' to disable symbolization."
        );
    }

    #[fuchsia::test]
    async fn logger_prints_current_logs_and_exits_on_dump() {
        let mut environment = TestEnvironment::new(TestEnvironmentConfig::default()).await;
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            filters: LogFilterArgs { symbolize: Some(SymbolizeMode::Off), ..Default::default() },
            ..LogCommand::default()
        };
        let rcs_connector = environment.rcs_connector().await;
        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);
        let mut event_stream = environment.take_event_stream().unwrap();

        assert_matches!(tool.main_no_timestamp(writer).await, Ok(()));
        assert_eq!(buffers.into_stdout_str(), "[00000.000000][ffx] INFO: Hello world!\u{1b}[m\n",);
        // ffx log keeps this connection always open. If it exits, it means that ffx log exits.
        assert_matches!(event_stream.next().await, Some(TestEvent::LogSettingsClosed));
    }

    #[fuchsia::test]
    async fn logger_does_not_color_logs_if_disabled() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig::default(),
            LogCommand {
                filters: LogFilterArgs { no_color: true, ..Default::default() },
                ..LogCommand::default()
            },
            "[00000.000000][ffx] INFO: Hello world!\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_prints_initial_timestamp() {
        let environment = TestEnvironment::new(TestEnvironmentConfig {
            show_initial_timestamp: true,
            boot_timestamp: BOOT_TIMESTAMP,
            messages: vec![],
            ..Default::default()
        })
        .await;
        let rcs_connector = environment.rcs_connector().await;
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            filters: LogFilterArgs { symbolize: Some(SymbolizeMode::Off), ..Default::default() },
            ..LogCommand::default()
        };
        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(Some(Format::Json), &buffers);

        assert_matches!(tool.main(writer).await, Ok(()));
        let output = buffers.into_stdout_str();

        let json: LogEntry = serde_json::from_str::<LogEntry>(&output).unwrap();

        let target_log = json.data.as_target_log().unwrap();
        let properties = target_log.payload_keys().unwrap();
        assert_eq!(target_log.msg().unwrap(), "Logging started");

        // Ensure the end has a valid timestamp
        chrono::DateTime::parse_from_rfc3339(
            properties.get_property("utc_time_now").unwrap().string().unwrap(),
        )
        .unwrap();
        assert_eq!(
            properties.get_property("current_boot_timestamp").unwrap().uint().unwrap(),
            BOOT_TIMESTAMP
        );
    }

    #[fuchsia::test]
    async fn logger_shows_metadata_if_enabled() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig::default(),
            LogCommand {
                filters: LogFilterArgs {
                    no_color: true,
                    show_metadata: true,
                    ..Default::default()
                },
                ..LogCommand::default()
            },
            "[00000.000000][1][2][ffx] INFO: Hello world!\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_shows_utc_time_if_enabled() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig::default(),
            LogCommand {
                filters: LogFilterArgs { clock: Some(TimeFormat::Utc), ..Default::default() },
                ..LogCommand::default()
            },
            "[1970-01-01 00:00:00.000][ffx] INFO: Hello world!\u{1b}[m\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_does_not_reconnect_if_disable_reconnect_flag_passed() {
        let selectors = vec![OneOrMany::One(parse_log_interest_selector("core/foo#INFO").unwrap())];
        let mut environment = TestEnvironment::new(TestEnvironmentConfig {
            send_connected_event: true,
            ..Default::default()
        })
        .await;

        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Watch(RawWatchCommand::default())),
            set_severity: selectors.clone(),
            filters: LogFilterArgs {
                symbolize: Some(SymbolizeMode::Off),
                no_color: true,
                disable_reconnect: true,
                until: None,
                ..Default::default()
            },
            ..LogCommand::default()
        };

        let rcs_connector = environment.rcs_connector().await;
        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);
        let mut event_stream = environment.take_event_stream().unwrap();

        let result = fasync::Task::local(tool.main_no_timestamp(writer));
        // Run the stream until we get the expected message.
        check_for_message(&buffers, "[00000.000000][ffx] INFO: Hello world!\n").await;

        // First connection should have used Subscribe mode.
        assert_matches!(
            event_stream.next().await,
            Some(TestEvent::Connected(StreamMode::SnapshotThenSubscribe))
        );

        // Interest should be set
        assert_eq!(
            event_stream.next().await,
            Some(TestEvent::SetInterest(selectors.clone().into_iter().flatten().collect()))
        );

        environment.reboot_target(Some(42));
        // If reconnect is disabled, we should return Ok(())
        assert!(result.await.is_ok());
    }

    #[fuchsia::test]
    async fn logger_shows_logs_filtered_by_severity() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig {
                messages: vec![
                    testing_utils::test_log_with_severity(0, Severity::Info),
                    testing_utils::test_log_with_severity(3000000000i64, Severity::Error),
                    testing_utils::test_log_with_severity(6000000000i64, Severity::Info),
                ],
                ..Default::default()
            },
            LogCommand {
                filters: LogFilterArgs {
                    clock: Some(TimeFormat::Utc),
                    severity: Some(Severity::Error),
                    ..Default::default()
                },
                ..LogCommand::default()
            },
            "\u{1b}[38;5;1m[1970-01-01 00:00:03.000][ffx] ERROR: Hello world!\u{1b}[m\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_shows_logs_since_specific_timestamp_across_reboots() {
        let selectors = vec![OneOrMany::One(parse_log_interest_selector("core/foo#INFO").unwrap())];
        let mut environment = TestEnvironment::new(TestEnvironmentConfig {
            messages: vec![testing_utils::test_log(testing_utils::naive_utc_nanos(
                "1980-01-01T00:00:03",
            ))],
            send_connected_event: true,
            ..Default::default()
        })
        .await;
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Watch(RawWatchCommand::default())),
            set_severity: selectors.clone(),
            filters: LogFilterArgs {
                symbolize: Some(SymbolizeMode::Off),
                clock: Some(TimeFormat::Utc),
                since: Some(log_command_fdomain::DetailedDateTime {
                    is_now: true,
                    ..parse_utc_time("1980-01-01T00:00:01").unwrap()
                }),
                until: None,
                ..Default::default()
            },
            ..LogCommand::default()
        };

        let rcs_connector = environment.rcs_connector().await;
        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);
        let mut event_stream = environment.take_event_stream().unwrap();

        // Intentionally unused. When in streaming mode, this should never return a value.
        let _result = fasync::Task::local(tool.main_no_timestamp(writer));

        // Run the stream until we get the expected message.
        check_for_message(&buffers, TEST_STR).await;

        // First connection should have used Subscribe mode.
        assert_matches!(
            event_stream.next().await,
            Some(TestEvent::Connected(StreamMode::Subscribe))
        );

        // Interest should be set
        assert_eq!(
            event_stream.next().await,
            Some(TestEvent::SetInterest(selectors.clone().into_iter().flatten().collect()))
        );

        environment.reboot_target(Some(42));

        // Device is paused when we exit the loop because there's nothing
        // polling the future.
        assert_matches!(event_stream.next().await, Some(TestEvent::LogSettingsClosed));

        check_for_message(&buffers, RECONNECT_STR).await;

        // Second connection has a different timestamp so should be treated
        // as a reboot.
        assert_matches!(
            event_stream.next().await,
            Some(TestEvent::Connected(StreamMode::SnapshotThenSubscribe))
        );

        environment.disconnect_target();

        // Interest should be set again
        assert_eq!(
            event_stream.next().await,
            Some(TestEvent::SetInterest(selectors.clone().into_iter().flatten().collect()))
        );

        assert_matches!(event_stream.next().await, Some(TestEvent::LogSettingsClosed));
    }

    #[fuchsia::test]
    async fn logger_works_with_legacy_fuchsia_version() {
        let mut environment = TestEnvironment::new(TestEnvironmentConfig {
            messages: vec![testing_utils::test_log(testing_utils::naive_utc_nanos(
                "1980-01-01T00:00:03",
            ))],
            // No boot ID is present, because this version of Fuchsia
            // didn't have that field.
            boot_id: None,
            send_connected_event: true,
            ..Default::default()
        })
        .await;
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Watch(RawWatchCommand::default())),
            filters: LogFilterArgs {
                symbolize: Some(SymbolizeMode::Off),
                clock: Some(TimeFormat::Utc),
                since: Some(log_command_fdomain::DetailedDateTime {
                    is_now: true,
                    ..parse_utc_time("1980-01-01T00:00:01").unwrap()
                }),
                until: None,
                ..Default::default()
            },
            ..LogCommand::default()
        };

        let rcs_connector = environment.rcs_connector().await;
        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);
        let mut event_stream = environment.take_event_stream().unwrap();

        // Intentionally unused. When in streaming mode, this should never return a value.
        let _result = fasync::Task::local(tool.main_no_timestamp(writer));

        // Run the stream until we get the expected message.
        check_for_message(&buffers, TEST_STR).await;

        // First connection should have used Subscribe mode.
        assert_matches!(
            event_stream.next().await,
            Some(TestEvent::Connected(StreamMode::Subscribe))
        );
    }

    #[fuchsia::test]
    async fn logger_shows_logs_since_specific_timestamp_across_reboots_heuristic() {
        let mut environment = TestEnvironment::new(TestEnvironmentConfig {
            messages: vec![testing_utils::test_log(testing_utils::naive_utc_nanos(
                "1980-01-01T00:00:03",
            ))],
            send_connected_event: true,
            ..Default::default()
        })
        .await;
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Watch(RawWatchCommand::default())),
            filters: LogFilterArgs {
                symbolize: Some(SymbolizeMode::Off),
                clock: Some(TimeFormat::Utc),
                since: Some(log_command_fdomain::DetailedDateTime {
                    is_now: true,
                    ..parse_utc_time("1980-01-01T00:00:01").unwrap()
                }),
                until: None,
                ..Default::default()
            },
            ..LogCommand::default()
        };

        let rcs_connector = environment.rcs_connector().await;
        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);
        let mut event_stream = environment.take_event_stream().unwrap();

        // Intentionally unused. When in streaming mode, this should never return a value.
        let _result = fasync::Task::local(tool.main_no_timestamp(writer));

        // Run the stream until we get the expected message.
        check_for_message(&buffers, TEST_STR).await;

        // First connection should have used Subscribe mode.
        assert_matches!(
            event_stream.next().await,
            Some(TestEvent::Connected(StreamMode::Subscribe))
        );

        environment.disconnect_target();

        // Device is paused when we exit the loop because there's nothing
        // polling the future.
        assert_matches!(event_stream.next().await, Some(TestEvent::LogSettingsClosed));

        // We should reconnect and get another message.
        check_for_message(&buffers, TEST_STR).await;

        // Second connection has a matching timestamp to the first one, so we should
        // Subscribe to not repeat messages.
        assert_matches!(
            event_stream.next().await,
            Some(TestEvent::Connected(StreamMode::Subscribe))
        );

        // For the third connection, we should get a
        // SnapshotThenSubscribe request because the timestamp
        // changed and it's clear it's actually a separate boot not a disconnect/reconnect
        environment.set_boot_timestamp(std::time::Duration::from_secs(42).as_nanos() as u64);
        environment.reboot_target(Some(42));

        assert_matches!(event_stream.next().await, Some(TestEvent::LogSettingsClosed));

        check_for_message(&buffers, RECONNECT_STR_WITH_42_SECONDS).await;

        assert_matches!(
            event_stream.next().await,
            Some(TestEvent::Connected(StreamMode::SnapshotThenSubscribe))
        );
    }

    #[fuchsia::test]
    async fn logger_shows_logs_since_specific_timestamp() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig {
                messages: vec![
                    testing_utils::test_log(testing_utils::naive_utc_nanos("1980-01-01T00:00:00")),
                    testing_utils::test_log(testing_utils::naive_utc_nanos("1980-01-01T00:00:03")),
                    testing_utils::test_log(testing_utils::naive_utc_nanos("1980-01-01T00:00:06")),
                ],
                ..Default::default()
            },
            LogCommand {
                filters: LogFilterArgs {
                    since: Some(parse_utc_time("1980-01-01T00:00:01").unwrap()),
                    until: Some(parse_utc_time("1980-01-01T00:00:05").unwrap()),
                    clock: Some(TimeFormat::Utc),
                    ..Default::default()
                },
                ..LogCommand::default()
            },
            "[1980-01-01 00:00:03.000][ffx] INFO: Hello world!\u{1b}[m\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_shows_logs_since_specific_timestamp_boot() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig {
                messages: vec![
                    testing_utils::test_log(0),
                    testing_utils::test_log(3000000000i64),
                    testing_utils::test_log(6000000000i64),
                ],
                ..Default::default()
            },
            LogCommand {
                filters: LogFilterArgs {
                    clock: Some(TimeFormat::Utc),
                    since_boot: Some(parse_seconds_string_as_duration("1").unwrap()),
                    until_boot: Some(parse_seconds_string_as_duration("5").unwrap()),
                    ..Default::default()
                },
                ..LogCommand::default()
            },
            "[1970-01-01 00:00:03.000][ffx] INFO: Hello world!\u{1b}[m\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_shows_local_time_if_enabled() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig::default(),
            LogCommand {
                filters: LogFilterArgs { clock: Some(TimeFormat::Local), ..Default::default() },
                ..LogCommand::default()
            },
            &format!(
                "[{}][ffx] INFO: Hello world!\u{1b}[m\n",
                Local.timestamp_opt(0, 1).unwrap().format(TIMESTAMP_FORMAT)
            ),
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_shows_tags_by_default() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig {
                messages: vec![testing_utils::test_log_with_tag(0)],
                ..Default::default()
            },
            LogCommand::default(),
            "[00000.000000][ffx][test tag] INFO: Hello world!\u{1b}[m\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_hides_full_moniker_by_default() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig {
                messages: vec![testing_utils::test_log_with_tag(0)],
                ..Default::default()
            },
            LogCommand::default(),
            "[00000.000000][ffx][test tag] INFO: Hello world!\u{1b}[m\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_shows_full_moniker_when_enabled() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig {
                messages: vec![testing_utils::test_log_with_tag(0)],
                ..Default::default()
            },
            LogCommand {
                filters: LogFilterArgs { show_full_moniker: true, ..Default::default() },
                ..LogCommand::default()
            },
            "[00000.000000][host/ffx][test tag] INFO: Hello world!\u{1b}[m\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_hides_tag_when_instructed() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig {
                messages: vec![testing_utils::test_log_with_tag(0)],
                ..Default::default()
            },
            LogCommand {
                filters: LogFilterArgs { hide_tags: true, ..Default::default() },
                ..LogCommand::default()
            },
            "[00000.000000][ffx] INFO: Hello world!\u{1b}[m\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_sets_severity_appropriately_then_exits() {
        let mut environment = TestEnvironment::new(TestEnvironmentConfig {
            messages: vec![testing_utils::test_log(0)],
            ..Default::default()
        })
        .await;
        let selector =
            vec![OneOrMany::One(parse_log_interest_selector("archivist.cm#TRACE").unwrap())];
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            set_severity: selector.clone(),
            filters: LogFilterArgs { symbolize: Some(SymbolizeMode::Off), ..Default::default() },
            ..LogCommand::default()
        };
        let mut event_stream = environment.take_event_stream().unwrap();

        let rcs_connector = environment.rcs_connector().await;
        let tool = LogTool { cmd, rcs_connector, context: environment.environment_context() };
        let buffers = TestBuffers::default();
        let writer = CommandOutputMachineWriter::new_test(None, &buffers);
        let selector = selector.into_iter().flatten().collect::<Vec<_>>();
        assert_matches!(tool.main_no_timestamp(writer).await, Ok(()));
        assert_eq!(buffers.into_stdout_str(), "[00000.000000][ffx] INFO: Hello world!\u{1b}[m\n");
        assert_matches!(
            event_stream.next().await,
            Some(TestEvent::SetInterest(s)) if s == selector
        );
        assert_matches!(event_stream.next().await, Some(TestEvent::LogSettingsClosed));
    }

    #[fuchsia::test]
    async fn logger_shows_file_names_by_default() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig {
                messages: vec![testing_utils::test_log_with_file(0)],
                ..Default::default()
            },
            LogCommand::default(),
            "[00000.000000][ffx][test tag] INFO: [test_filename.cc(42)] Hello world!\u{1b}[m\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn logger_hides_filename_if_disabled() {
        Box::pin(logger_dump_test(
            TestEnvironmentConfig {
                messages: vec![testing_utils::test_log_with_file(0)],
                ..Default::default()
            },
            LogCommand {
                filters: LogFilterArgs { hide_file: true, ..Default::default() },
                ..LogCommand::default()
            },
            "[00000.000000][ffx][test tag] INFO: Hello world!\u{1b}[m\n",
        ))
        .await;
    }

    #[fuchsia::test]
    async fn get_stream_mode_tests() {
        assert_matches!(
            get_stream_mode(LogCommand { ..LogCommand::default() }),
            Ok(fdomain_fuchsia_diagnostics::StreamMode::SnapshotThenSubscribe)
        );
        assert_matches!(
            get_stream_mode(LogCommand {
                filters: LogFilterArgs {
                    since: Some(parse_time("now").unwrap()),
                    ..Default::default()
                },
                ..LogCommand::default()
            },),
            Ok(fdomain_fuchsia_diagnostics::StreamMode::Subscribe)
        );
        assert_matches!(
            get_stream_mode(LogCommand {
                filters: LogFilterArgs {
                    since: Some(parse_time("09/04/1998").unwrap()),
                    ..Default::default()
                },
                ..LogCommand::default()
            },),
            Ok(fdomain_fuchsia_diagnostics::StreamMode::SnapshotThenSubscribe)
        );
    }

    struct BrokenPipeWriter;
    impl std::io::Write for BrokenPipeWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe"))
        }
    }

    impl ToolIO for BrokenPipeWriter {
        type OutputItem = LogEntry;
        fn is_machine(&self) -> bool {
            false
        }
        fn stderr(&mut self) -> &mut dyn std::io::Write {
            self
        }
        fn item(&mut self, _value: &Self::OutputItem) -> ffx_writer::Result<()> {
            Err(ffx_writer::Error::Io(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "broken pipe",
            )))
        }
    }

    #[fuchsia::test]
    async fn test_log_impl_exits_on_broken_pipe() {
        let environment = TestEnvironment::new(TestEnvironmentConfig::default()).await;
        let rcs_connector = environment.rcs_connector().await;
        let cmd = LogCommand {
            sub_command: Some(LogSubCommand::Dump(RawDumpCommand::default())),
            filters: LogFilterArgs { symbolize: Some(SymbolizeMode::Off), ..Default::default() },
            ..LogCommand::default()
        };
        let writer = BrokenPipeWriter;
        assert_matches!(
            Box::pin(log_impl(
                writer,
                &environment.environment_context(),
                cmd,
                rcs_connector,
                false
            ))
            .await,
            Ok(())
        );
    }
}
