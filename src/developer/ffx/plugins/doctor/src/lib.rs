// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::doctor_ledger::*;
use crate::ledger_view::*;
use anyhow::Result;
use async_lock::Mutex;
use async_trait::async_trait;
use doctor_utils::DoctorRecorder;
use errors::ffx_bail;
use ffx_build_version::VersionInfo;
use ffx_config::EnvironmentContext;
use ffx_doctor_args::DoctorCommand;
use ffx_ssh::SshKeyFiles;
use ffx_target_show::ShowTool;
use ffx_target_show_args::TargetShow;
use ffx_writer::{MachineWriter, ToolIO, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool, FhoEnvironment};
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use termio::Colors;

mod doctor_ledger;
mod environment;
mod gcheck;
mod ledger_view;
mod network;
mod record;
mod single_target_diagnostics;
mod target;
mod types;
mod usb;

use crate::environment::{
    check_emulators, check_env_context, check_ffx_info, check_inotify_watches,
    get_config_permission,
};
use crate::network::run_google_network_checks;
use crate::record::doctor_record;
use crate::target::check_targets_locally;
use crate::types::{
    DefaultDoctorStepHandler, DoctorRecorderParameters, DoctorResult, DoctorStepHandler, StepType,
};
use crate::usb::{CommandUsbDriverFinder, UsbDriverFinder, check_usb_driver};

pub struct ShowToolWrapper {
    env: FhoEnvironment,
    inner: Option<ShowTool>,
}

impl ShowToolWrapper {
    async fn allocate(&mut self, target_spec: Option<String>) -> fho::Result<()> {
        let mut context = self
            .env
            .ffx_command()
            .global
            .load_context(self.env.environment_context().exe_kind())?;
        context.override_target_specifier(&target_spec);
        let fho_env = FhoEnvironment::new(&context, self.env.ffx_command());
        self.inner.replace(ShowTool::from_env(fho_env, TargetShow::default()).await?);
        Ok(())
    }

    /// This requires that `allocate` is run first. This is really only to ensure that there are
    /// two steps in the process for running an invocation of `ffx target show`.
    async fn run(&mut self) -> fho::Result<(String, String)> {
        let tool = self.inner.take().unwrap();
        let buffers = ffx_writer::TestBuffers::default();
        match tool.main(VerifiedMachineWriter::new_test(None, &buffers)).await {
            Ok(_) => Ok(buffers.into_strings()),
            Err(e) => Err(fho::user_error!("{}\n\tstderr: {}", e, buffers.into_stderr_str())),
        }
    }
}

#[async_trait(?Send)]
impl fho::TryFromEnv for ShowToolWrapper {
    type Error = std::convert::Infallible;
    async fn try_from_env(env: &FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        Ok(Self { env: env.clone(), inner: None })
    }
}

#[derive(FfxTool)]
pub struct DoctorTool {
    #[command]
    cmd: DoctorCommand,
    show_tool: ShowToolWrapper,
    context: EnvironmentContext,
}

fho::embedded_plugin!(DoctorTool);

#[async_trait(?Send)]
impl FfxMain for DoctorTool {
    type Writer = MachineWriter<DoctorResult>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        // TODO(b/373720502): This is passing a `Some(self.show_tool)` to make it simpler not to
        // have to update existing tests that take in a dozen arguments. The proper approach for
        // this is to refactor `ffx doctor` to make testing things like this less cumbersome.
        // TODO(b/373723080): Add actual tests for the usage of `ffx target show` within `ffx
        // doctor`.
        // This duplication avoids dynamic dispatch overhead from generic Write arguments
        // (std::io::Sink vs std::io::Stdout), which would result in different types.
        if writer.is_machine() {
            let ledger = Box::pin(doctor_cmd_impl(
                self.context,
                self.cmd,
                Some(self.show_tool),
                std::io::sink(),
                std::io::sink(),
            ))
            .await?;
            writer.machine(&DoctorResult { steps: ledger.into_root_node() })?;
        } else {
            Box::pin(doctor_cmd_impl(
                self.context,
                self.cmd,
                Some(self.show_tool),
                stdout(),
                stdout(),
            ))
            .await?;
        }
        Ok(())
    }
}

pub async fn doctor_cmd_impl<
    StepWriter: Write + Send + Sync + 'static,
    LedgerWriter: Write + Send + Sync + 'static,
>(
    context: EnvironmentContext,

    mut cmd: DoctorCommand,
    show_tool: Option<ShowToolWrapper>,
    step_writer: StepWriter,
    ledger_writer: LedgerWriter,
) -> Result<DoctorLedger<LedgerWriter>> {
    let mut writer: Box<dyn Write + Send + Sync + 'static> = Box::new(step_writer);
    let gchecker = gcheck::DefaultGChecker;
    let delay = Duration::from_millis(cmd.retry_delay);
    let target_spec = ffx_target::get_target_specifier(&context)?;
    let target_str = target_spec.unwrap_or_else(String::default);
    let version_info: VersionInfo = context.build_info();
    let colors = Colors::current();

    if cmd.restart_daemon {
        log::warn!(
            "--restart-daemon is deprecated and no longer has any effect as the ffx daemon has been removed."
        );
        writeln!(
            &mut writer,
            "{}WARNING:{} --restart-daemon is deprecated and no longer has any effect as the ffx daemon has been removed.",
            colors.red, colors.reset
        )?;
    }
    if cmd.retry_count.is_some() {
        log::warn!(
            "--retry-count is deprecated and no longer has any effect as the ffx daemon has been removed."
        );
        writeln!(
            &mut writer,
            "{}WARNING:{} --retry-count is deprecated and no longer has any effect as the ffx daemon has been removed.",
            colors.red, colors.reset
        )?;
    }
    let mut log_root = None;
    let mut output_dir = None;
    let mut record = cmd.record;
    // Force-enable verbose mode if `record` is enabled.
    if record {
        cmd.verbose = true;
    }
    match context.get("log.enabled") {
        Ok(enabled) => {
            let enabled: bool = enabled;
            if !enabled && cmd.record {
                writeln!(
                    &mut writer,
                    "{}WARNING:{} --record was provided but ffx logs are not enabled. This means your record will only include doctor output.",
                    colors.red, colors.reset
                )?;
                writeln!(
                    &mut writer,
                    "ffx doctor will proceed, but if you want to enable logs, you can do so by running:"
                )?;
                writeln!(&mut writer, "  ffx config set log.enabled true\n\n")?;
                fuchsia_async::Timer::new(Duration::from_secs(3)).await;
            }

            log_root = Some(context.get("log.dir")?);
            let final_output_dir =
                cmd.output_dir.map(|s| PathBuf::from(s)).unwrap_or(std::env::current_dir()?);

            if !final_output_dir.is_dir() {
                ffx_bail!(
                    "cannot record: output directory does not exist or is unreadable: {:?}",
                    output_dir
                );
            }

            output_dir = Some(final_output_dir);
        }
        Err(e) => {
            writeln!(
                &mut writer,
                "{}WARNING:{} getting log status from ffx config failed. The error was: {:?}",
                colors.red, colors.reset, e
            )?;
            if cmd.record {
                writeln!(
                    &mut writer,
                    "Record mode requires configuration and will be turned off for this run."
                )?;
            }
            writeln!(
                &mut writer,
                "If this issue persists, please file a bug here: {}",
                errors::BUG_REPORT_URL
            )?;
            fuchsia_async::Timer::new(Duration::from_millis(10000)).await;

            record = false;
        }
    };

    let user_config_enabled = if !record || cmd.no_config {
        false
    } else {
        match get_config_permission(&context, &mut writer).await {
            Ok(b) => b,
            Err(e) => {
                writeln!(&mut writer, "Failed to get permission to record config data: {}", e)?;
                writeln!(&mut writer, "Config data will not be recorded")?;
                false
            }
        }
    };

    if cmd.repair_keys {
        let keys = SshKeyFiles::load(&context)?;
        let message = keys.check_keys(true)?;
        writeln!(&mut writer, "{message}")?;
    }

    let recorder = Arc::new(Mutex::new(DoctorRecorder::new()));
    let mut handler = DefaultDoctorStepHandler::new(recorder.clone(), writer, colors);

    // create ledger
    let ledger_mode = match cmd.verbose {
        true => LedgerViewMode::Verbose,
        false => LedgerViewMode::Normal,
    };
    let mut ledger =
        DoctorLedger::new(ledger_writer, Box::new(VisualLedgerView::new()), ledger_mode);
    let usb_driver_finder = CommandUsbDriverFinder {};

    Box::pin(doctor(
        &mut handler,
        &mut ledger.root_guard(),
        &target_str,
        delay,
        version_info,
        &context,
        DoctorRecorderParameters {
            record,
            user_config_enabled,
            log_root,
            output_dir,
            recorder: recorder.clone(),
        },
        usb_driver_finder,
        gchecker,
        show_tool,
    ))
    .await?;

    match ledger.calc_outcome(0) {
        LedgerOutcome::Warning => {
            handler.output_step(StepType::DoctorNoticeWarning).await?;
        }
        LedgerOutcome::Failure => {
            handler.output_step(StepType::DoctorNoticeFailure).await?;
        }
        _ => {}
    }

    Ok(ledger)
}

async fn doctor<W: Write>(
    step_handler: &mut impl DoctorStepHandler,
    ledger: &mut LedgerNodeGuard<'_, W>,
    target_str: &str,
    retry_delay: Duration,
    version_info: VersionInfo,
    env_context: &EnvironmentContext,
    record_params: DoctorRecorderParameters,
    usb_driver_finder: impl UsbDriverFinder,
    gchecker: impl gcheck::GChecker,
    show_tool: Option<ShowToolWrapper>,
) -> Result<()> {
    doctor_summary(
        step_handler,
        target_str,
        retry_delay,
        version_info,
        env_context,
        show_tool,
        usb_driver_finder,
        gchecker,
        ledger,
    )
    .await?;

    if record_params.record {
        let mut record_view = RecordLedgerView::new();
        let data = ledger.write_all(&mut record_view);
        step_handler.record(StepType::Output(data)).await?;
        doctor_record(env_context, step_handler, record_params).await?;
    }

    Ok(())
}

fn print_summary_outcome<W: Write>(ledger: &mut LedgerNodeGuard<'_, W>) {
    match ledger.calc_outcome_at_next_depth() {
        LedgerOutcome::Failure => {
            let msg = match ledger.get_ledger_mode() {
                LedgerViewMode::Normal => String::from(
                    "Doctor found issues in one or more categories; \
                    run 'ffx doctor -v' for more details.",
                ),
                _ => String::from("Doctor found issues in one or more categories."),
            };
            let node = ledger.add_node(&msg, LedgerMode::Automatic);
            node.set_outcome(LedgerOutcome::Failure);
        }
        _ => {
            let node = ledger.add_node("No issues found", LedgerMode::Automatic);
            node.set_outcome(LedgerOutcome::Success);
        }
    }
}

async fn doctor_summary<W: Write>(
    step_handler: &mut impl DoctorStepHandler,
    target_str: &str,
    retry_delay: Duration,
    version_info: VersionInfo,
    env_context: &EnvironmentContext,
    show_tool: Option<ShowToolWrapper>,
    usb_driver_finder: impl UsbDriverFinder,
    gchecker: impl gcheck::GChecker,
    ledger: &mut LedgerNodeGuard<'_, W>,
) -> Result<()> {
    match ledger.get_ledger_mode() {
        LedgerViewMode::Normal => {
            step_handler.output_step(StepType::DoctorSummaryInitNormal).await?
        }
        LedgerViewMode::Verbose => {
            step_handler.output_step(StepType::DoctorSummaryInitVerbose).await?
        }
    }

    check_ffx_info(ledger, &version_info).await;
    check_env_context(ledger, env_context).await?;
    check_emulators(ledger, env_context).await?;
    check_inotify_watches(ledger).await;

    check_usb_driver(&usb_driver_finder, ledger, env_context).await;

    run_google_network_checks(ledger, env_context, &gchecker).await?;
    Box::pin(check_targets_locally(ledger, target_str, env_context, show_tool, retry_delay))
        .await?;

    print_summary_outcome(ledger);

    Ok(())
}

///////////////////////////////////////////////////////////////////////////////////////////////////
// Tests
///////////////////////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod test {
    use super::*;
    use crate::record::collect_log_files;
    use crate::usb::{FindUsbDriverError, MockUsbDriverFinder, UsbDriverStatus};
    use doctor_utils::Recorder;
    use emulator_instance::{EmulatorInstanceData, EngineState};
    use ffx_doctor_test_utils::MockWriter;
    use serde_json::json;
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::{fmt, fs};
    use tempfile::tempdir;

    const INDENT_STR: &str = "    ";

    struct FakeGChecker;

    impl gcheck::GChecker for FakeGChecker {
        fn is_gcorp_machine(&self) -> bool {
            true
        }
    }

    struct FakeLedgerView {
        tree: LedgerViewNode,
        omit_error_reason: bool,
    }

    impl FakeLedgerView {
        pub fn new() -> Self {
            FakeLedgerView { tree: LedgerViewNode::default(), omit_error_reason: true }
        }
        fn gen_output(&self, parent_node: &LedgerViewNode, indent_level: usize) -> String {
            let mut data = parent_node.data.clone();
            // Remove error details to make the tests more stable
            if self.omit_error_reason && data.starts_with("Error") {
                let v: Vec<_> = data.split(":").collect();
                if v.len() > 1 {
                    data = format!("{}: <reason omitted>", v.first().unwrap().to_string());
                }
            }

            let mut output_str = format!(
                "{}[{}] {}\n",
                INDENT_STR.repeat(indent_level),
                parent_node.outcome.format(&Colors::disabled()),
                data
            );

            for child_node in &parent_node.children {
                let child_str = self.gen_output(child_node, indent_level + 1);
                output_str = format!("{}{}", output_str, child_str);
            }

            return output_str;
        }
    }

    impl fmt::Display for FakeLedgerView {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "{}", self.gen_output(&self.tree, 0))
        }
    }

    impl LedgerView for FakeLedgerView {
        fn set(&mut self, new_tree: LedgerViewNode) {
            self.tree = new_tree;
        }
    }

    struct DisabledRecorder {}

    impl DisabledRecorder {
        fn new() -> Self {
            return Self {};
        }
    }

    impl Recorder for DisabledRecorder {
        fn add_sources(&mut self, _sources: Vec<PathBuf>) {
            panic!("add_sources should not be called.")
        }

        fn add_content(&mut self, _filename: &str, _content: String) {
            // Do nothing, we don't verify output in tests.
        }

        fn generate(
            &self,
            _output_dir: PathBuf,
        ) -> Result<PathBuf, doctor_utils::DoctorUtilsError> {
            panic!("generate should not be called.")
        }
    }

    fn record_params_no_record() -> DoctorRecorderParameters {
        DoctorRecorderParameters {
            record: false,
            user_config_enabled: false,
            log_root: None,
            output_dir: None,
            recorder: Arc::new(Mutex::new(DisabledRecorder::new())),
        }
    }

    async fn setup_ssh_keys(isolate_root: &Path) -> Result<(PathBuf, PathBuf)> {
        let pub_key = isolate_root.join("test_authorized_keys");
        let priv_key = isolate_root.join("test_ed25519_key");
        let keys = SshKeyFiles { authorized_keys: pub_key.clone(), private_key: priv_key.clone() };
        keys.create_keys_if_needed(false)?;
        Ok((pub_key, priv_key))
    }

    fn setup_emu_dir(isolate_root: &Path) -> Result<PathBuf> {
        let emu_dir = isolate_root.join("emu_data");
        fs::create_dir_all(&emu_dir)?;
        Ok(emu_dir)
    }

    async fn setup_driver_socket_file(isolate_root: &Path) -> Result<PathBuf> {
        let socket_file_dir = isolate_root.join("test_usb_driver_socket");
        fs::create_dir_all(&socket_file_dir)?;
        let socket_file = socket_file_dir.join("socket");
        std::fs::File::create(&socket_file)?;
        Ok(socket_file)
    }

    fn default_mock_driver_finder() -> MockUsbDriverFinder {
        let mut mock = MockUsbDriverFinder::new();
        mock.expect_find().returning(|| {
            Ok(vec![UsbDriverStatus { pid: 1, socket_path: "/tmp/fake/socket/path".to_string() }])
        });
        mock
    }

    struct FakeRecorder {
        expected_sources: Vec<PathBuf>,
        expected_output_dir: PathBuf,
        generate_called: Cell<bool>,
    }

    impl FakeRecorder {
        fn new(expected_sources: Vec<PathBuf>, expected_output_dir: PathBuf) -> Self {
            Self { expected_sources, expected_output_dir, generate_called: Cell::new(false) }
        }

        fn result_path() -> PathBuf {
            PathBuf::from("/tmp").canonicalize().unwrap()
        }
    }

    impl Recorder for FakeRecorder {
        fn add_sources(&mut self, sources: Vec<PathBuf>) {
            let source_set: HashSet<_> = sources.iter().collect();
            let expected_set: HashSet<_> = self.expected_sources.iter().collect();
            assert_eq!(source_set, expected_set);
        }

        fn add_content(&mut self, _filename: &str, _content: String) {
            // Do nothing, we don't verify output in tests.
        }

        fn generate(&self, output_dir: PathBuf) -> Result<PathBuf, doctor_utils::DoctorUtilsError> {
            assert_eq!(output_dir, self.expected_output_dir);
            self.generate_called.set(true);
            Ok(Self::result_path())
        }
    }

    fn record_params_with_temp(
        root: PathBuf,
    ) -> (Arc<Mutex<FakeRecorder>>, DoctorRecorderParameters) {
        let mut fe_log = root.clone();
        fe_log.push("ffx.log");
        fs::write(&fe_log, "ffx.log contents").expect("writing test ffx.log");
        let recorder = Arc::new(Mutex::new(FakeRecorder::new(vec![fe_log], root.clone())));
        (
            recorder.clone(),
            DoctorRecorderParameters {
                record: true,
                user_config_enabled: false,
                log_root: Some(root.clone()),
                output_dir: Some(root.clone()),
                recorder: recorder.clone(),
            },
        )
    }

    async fn missing_field_test(
        _fake_recorder: Arc<Mutex<FakeRecorder>>,
        params: DoctorRecorderParameters,
    ) {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let socket_file = setup_driver_socket_file(&isolate_root)
            .await
            .expect("setting up fake driver socket file");
        let test_env = builder
            .user_config(usb_driver_api::CONFIG_USB_SOCKET_PATH, json!(socket_file))
            .build()
            .unwrap();

        let recorder = Arc::new(Mutex::new(DoctorRecorder::new()));
        let mut handler = DefaultDoctorStepHandler::new(
            recorder,
            Box::new(MockWriter::new()),
            Colors::disabled(),
        );
        let mut ledger = DoctorLedger::<MockWriter>::new(
            MockWriter::new(),
            Box::new(FakeLedgerView::new()),
            LedgerViewMode::Verbose,
        );

        let mock_driver_finder = default_mock_driver_finder();
        assert!(
            doctor(
                &mut handler,
                &mut ledger.root_guard(),
                "",
                std::time::Duration::from_millis(2000),
                VersionInfo::default(),
                &test_env.context,
                params,
                mock_driver_finder,
                FakeGChecker,
                None,
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn test_collect_log_files() {
        let temp = tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let mut expected = vec![root.join("f1.log"), root.join("f2.log")];
        for p in &expected {
            fs::write(p, "something").expect("written testdata");
        }
        // write out other files
        fs::write(root.join("no_extension"), "something").expect("written testdata");
        fs::write(root.join("notlog.txt"), "something").expect("written testdata");
        fs::write(root.join("save.log.save"), "something").expect("written testdata");

        let subdir = root.join("subdir");
        fs::create_dir_all(&subdir).expect("subdir created");
        fs::write(subdir.join("sublog.log"), "something").expect("written testdata");

        let mut actual = collect_log_files(root.clone()).expect("collecting");
        // Sort the lists to make comparison easy.
        expected.sort();
        actual.sort();
        assert_eq!(expected, actual);
    }

    #[fuchsia::test]
    async fn test_doctor_summary_with_gdoctor_subtool() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let subtool_search_dir_path = temp_dir.path();
        let mock_gdoctor_path = temp_dir.path().join("ffx-gdoctor");
        // Scope to ensure File handle is dropped (and file closed) before setting permissions
        {
            let mut mock_gdoctor_script =
                fs::File::create(&mock_gdoctor_path).expect("Failed to create mock script");
            // example data in DoctorCheck format
            write!(
            mock_gdoctor_script,
            "#!/bin/sh\n\
             echo '{{\"name\": \"Corp DHCP\", \"message\": \"Successfully connected\", \"result\": \"passed\"}}'\n\
             echo '{{\"name\": \"GPN\", \"message\": \"GPN not detected\", \"result\": \"failed\"}}'\n"
        )
        .expect("Failed to write to mock script");
        }

        let mut perms = fs::metadata(&mock_gdoctor_path)
            .expect("Failed to get metadata for mock script")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&mock_gdoctor_path, perms)
            .expect("Failed to set permissions on mock script");
        let metadata_path = temp_dir.path().join("ffx-gdoctor.json");
        let metadata_content = serde_json::json!({
            "name": "gdoctor",
            "description": "Mock gdoctor for testing",
            "requires_fho": 0,
            "fho_details": {
                "version": 0
            }
        });
        fs::write(&metadata_path, metadata_content.to_string()).expect("Failed to write metadata");

        // Configure ffx to use our temporary search path
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let socket_file = setup_driver_socket_file(&isolate_root)
            .await
            .expect("setting up fake driver socket file");

        let test_env = builder
            .env_var(EnvironmentContext::FFX_BIN_ENV, "host-tools/ffx")
            .runtime_config("ffx.subtool-search-paths", json!([subtool_search_dir_path]))
            .user_config(usb_driver_api::CONFIG_USB_SOCKET_PATH, json!(socket_file))
            .build()
            .expect("Setting up test environment");

        let recorder = Arc::new(Mutex::new(DoctorRecorder::new()));
        let mut handler = DefaultDoctorStepHandler::new(
            recorder,
            Box::new(MockWriter::new()),
            Colors::disabled(),
        );
        let mut ledger = DoctorLedger::<MockWriter>::new(
            MockWriter::new(),
            Box::new(FakeLedgerView::new()),
            LedgerViewMode::Verbose,
        );

        let mock_driver_finder = default_mock_driver_finder();
        doctor(
            &mut handler,
            &mut ledger.root_guard(),
            "",
            std::time::Duration::from_millis(2000),
            VersionInfo::default(),
            &test_env.context,
            record_params_no_record(),
            mock_driver_finder,
            FakeGChecker,
            None,
        )
        .await
        .unwrap();

        let output = ledger.writer.get_data();
        assert!(
            output.contains("[✗] Google Network Checks"),
            "Main 'Google Network Checks' node missing or has wrong outcome. Output:\n{}",
            output
        );
        assert!(
            output.contains("[✓] Corp DHCP: Successfully connected"),
            "'Corp DHCP' check missing or has wrong outcome. Output:\n{}",
            output
        );
        assert!(
            output.contains("[✗] GPN: GPN not detected"),
            "'GPN' check missing or has wrong outcome. Output:\n{}",
            output
        );
    }

    #[fuchsia::test]
    async fn test_check_emulators() -> Result<()> {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();

        let socket_file = setup_driver_socket_file(&isolate_root)
            .await
            .expect("setting up fake driver socket file");
        let emu_dir = setup_emu_dir(&isolate_root).expect("setting up emulator data");

        let test_env = builder
            .user_config(usb_driver_api::CONFIG_USB_SOCKET_PATH, json!(socket_file))
            .user_config(ffx_config::keys::EMU_INSTANCE_ROOT_DIR, json!(&emu_dir))
            .build()
            .unwrap();
        // No instances
        {
            let mut writer = MockWriter::new();
            let mut ledger = DoctorLedger::new(
                &mut writer,
                Box::new(VisualLedgerView::new()),
                LedgerViewMode::Verbose,
            );
            check_emulators(&mut ledger.root_guard(), &test_env.context).await?;
            let output = writer.get_data();
            assert!(output.contains("FFX Emulator Instances"));
            assert!(!output.contains("Name:"), "got instance on empty dir: {}", output);
        }

        // One running instance
        let instance_dir = emu_dir.as_path().join("fuchsia-emulator");
        fs::create_dir(&instance_dir)?;
        let mut instance_data =
            EmulatorInstanceData::new_with_state("fuchsia-emulator", EngineState::Running);
        instance_data.set_pid(std::process::id());
        let engine_json_path = instance_dir.join("engine.json");
        fs::write(&engine_json_path, serde_json::to_string(&instance_data)?)?;

        {
            let mut writer = MockWriter::new();
            let mut ledger = DoctorLedger::new(
                &mut writer,
                Box::new(VisualLedgerView::new()),
                LedgerViewMode::Verbose,
            );
            check_emulators(&mut ledger.root_guard(), &test_env.context).await?;
            let output = writer.get_data();
            assert!(output.contains("FFX Emulator Instances"));
            assert!(output.contains("Name: fuchsia-emulator"));
            assert!(output.contains("Is Running: true"));
            assert!(output.contains("Engine State: running"));
        }

        // One stopped instance
        instance_data.set_engine_state(EngineState::Staged);
        instance_data.set_pid(0);
        fs::write(&engine_json_path, serde_json::to_string(&instance_data)?)?;

        {
            let mut writer = MockWriter::new();
            let mut ledger = DoctorLedger::new(
                &mut writer,
                Box::new(VisualLedgerView::new()),
                LedgerViewMode::Verbose,
            );
            check_emulators(&mut ledger.root_guard(), &test_env.context).await?;
            let output = writer.get_data();
            assert!(output.contains("FFX Emulator Instances"));
            assert!(output.contains("Name: fuchsia-emulator"));
            assert!(output.contains("Is Running: false"));
            assert!(output.contains("Engine State: staged"));
        }

        // One orphaned instance (Running but pid not running/crashing, triggers cleanup)
        instance_data.set_engine_state(EngineState::Running);
        instance_data.set_pid(9999999);
        fs::write(&engine_json_path, serde_json::to_string(&instance_data)?)?;

        {
            let mut writer = MockWriter::new();
            let mut ledger = DoctorLedger::new(
                &mut writer,
                Box::new(VisualLedgerView::new()),
                LedgerViewMode::Verbose,
            );
            check_emulators(&mut ledger.root_guard(), &test_env.context).await?;
            let output = writer.get_data();
            assert!(output.contains("FFX Emulator Instances"));
            assert!(output.contains("Cleanup Stale Emulator: fuchsia-emulator"));
            assert!(output.contains("Cleaning up orphaned instance 'fuchsia-emulator'"));
            assert!(output.contains("Cleanup successful."));
            // At this point, the cleanup should have removed the directory.
            assert!(!instance_dir.exists());
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn test_usb_driver_not_running() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();

        let (pub_key, priv_key) =
            setup_ssh_keys(&isolate_root).await.expect("setting up ssh test keys");
        let emu_dir = setup_emu_dir(&isolate_root).expect("setting up emulator data");

        let test_env = builder
            .user_config("ssh.pub", json!([&pub_key]))
            .user_config("ssh.priv", json!([&priv_key]))
            .user_config(ffx_config::keys::EMU_INSTANCE_ROOT_DIR, json!(&emu_dir))
            .user_config(ffx_config::keys::USB_ENABLED, json!(true))
            .build()
            .unwrap();

        let recorder = Arc::new(Mutex::new(DoctorRecorder::new()));
        let mut handler = DefaultDoctorStepHandler::new(
            recorder,
            Box::new(MockWriter::new()),
            Colors::disabled(),
        );
        let mut ledger = DoctorLedger::<MockWriter>::new(
            MockWriter::new(),
            Box::new(FakeLedgerView::new()),
            LedgerViewMode::Verbose,
        );

        let mut mock_driver_finder = MockUsbDriverFinder::new();
        mock_driver_finder
            .expect_find()
            .times(1)
            .returning(|| Err(FindUsbDriverError::DriverIsNotRunning));

        doctor(
            &mut handler,
            &mut ledger.root_guard(),
            "",
            std::time::Duration::from_millis(2000),
            VersionInfo::default(),
            &test_env.context,
            record_params_no_record(),
            mock_driver_finder,
            FakeGChecker,
            None,
        )
        .await
        .unwrap();

        let output = ledger.writer.get_data();
        assert!(
            output.contains("The ffx-usb-driver is not running."),
            "Output missing USB driver not running message: {}",
            output
        );
    }

    #[fuchsia::test]
    async fn test_usb_driver_disabled() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();

        let (pub_key, priv_key) =
            setup_ssh_keys(&isolate_root).await.expect("setting up ssh test keys");
        let emu_dir = setup_emu_dir(&isolate_root).expect("setting up emulator data");

        let test_env = builder
            .user_config("ssh.pub", json!([&pub_key]))
            .user_config("ssh.priv", json!([&priv_key]))
            .user_config(ffx_config::keys::EMU_INSTANCE_ROOT_DIR, json!(&emu_dir))
            .user_config(ffx_config::keys::USB_ENABLED, json!(false))
            .build()
            .unwrap();

        let recorder = Arc::new(Mutex::new(DoctorRecorder::new()));
        let mut handler = DefaultDoctorStepHandler::new(
            recorder,
            Box::new(MockWriter::new()),
            Colors::disabled(),
        );
        let mut ledger = DoctorLedger::<MockWriter>::new(
            MockWriter::new(),
            Box::new(FakeLedgerView::new()),
            LedgerViewMode::Verbose,
        );

        let mut mock_driver_finder = MockUsbDriverFinder::new();
        mock_driver_finder.expect_find().times(0);

        doctor(
            &mut handler,
            &mut ledger.root_guard(),
            "",
            std::time::Duration::from_millis(2000),
            VersionInfo::default(),
            &test_env.context,
            record_params_no_record(),
            mock_driver_finder,
            FakeGChecker,
            None,
        )
        .await
        .unwrap();

        let output = ledger.writer.get_data();
        assert!(!output.contains("FFX USB Driver"), "Output contains FFX USB Driver: {}", output);
    }

    #[fuchsia::test]
    async fn test_missing_ssh_keys() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let pub_key = isolate_root.join("test_authorized_keys");
        let priv_key = isolate_root.join("test_ed25519_key");

        let socket_file = setup_driver_socket_file(&isolate_root)
            .await
            .expect("setting up fake driver socket file");

        let test_env = builder
            .user_config("ssh.pub", json!([&pub_key]))
            .user_config("ssh.priv", json!([&priv_key]))
            .user_config(usb_driver_api::CONFIG_USB_SOCKET_PATH, json!(socket_file))
            .user_config(ffx_config::keys::USB_ENABLED, json!(true))
            .build()
            .unwrap();
        // Do not generate the keys - so they are missing.

        let recorder = Arc::new(Mutex::new(DoctorRecorder::new()));
        let mut handler = DefaultDoctorStepHandler::new(
            recorder,
            Box::new(MockWriter::new()),
            Colors::disabled(),
        );
        let mut ledger = DoctorLedger::<MockWriter>::new(
            MockWriter::new(),
            Box::new(FakeLedgerView::new()),
            LedgerViewMode::Verbose,
        );

        let mock_driver_finder = default_mock_driver_finder();
        doctor(
            &mut handler,
            &mut ledger.root_guard(),
            "",
            std::time::Duration::from_millis(2000),
            VersionInfo::default(),
            &test_env.context,
            record_params_no_record(),
            mock_driver_finder,
            FakeGChecker,
            None,
        )
        .await
        .unwrap();

        let output = ledger.writer.get_data();
        assert!(
            output.contains("Private key") && output.contains("does not exist"),
            "Output missing expected SSH key missing error: {}",
            output
        );
    }

    #[fuchsia::test]
    async fn test_invalid_filter_finds_no_targets() {
        let mut builder = ffx_config::test_env();
        let isolate_root = builder.isolate_root();
        let socket_file = setup_driver_socket_file(&isolate_root)
            .await
            .expect("setting up fake driver socket file");

        let test_env = builder
            .user_config(usb_driver_api::CONFIG_USB_SOCKET_PATH, json!(socket_file))
            .build()
            .unwrap();

        let recorder = Arc::new(Mutex::new(DoctorRecorder::new()));
        let mut handler = DefaultDoctorStepHandler::new(
            recorder,
            Box::new(MockWriter::new()),
            Colors::disabled(),
        );
        let mut ledger = DoctorLedger::<MockWriter>::new(
            MockWriter::new(),
            Box::new(FakeLedgerView::new()),
            LedgerViewMode::Verbose,
        );

        let mock_driver_finder = default_mock_driver_finder();
        doctor(
            &mut handler,
            &mut ledger.root_guard(),
            "non-existent-target-specifier-12345",
            std::time::Duration::from_millis(2000),
            VersionInfo::default(),
            &test_env.context,
            record_params_no_record(),
            mock_driver_finder,
            FakeGChecker,
            None,
        )
        .await
        .unwrap();

        let output = ledger.writer.get_data();
        assert!(
            output.contains("[✗] Searching for targets")
                && output.contains("[✗] No targets found!"),
            "Output missing target search failure: {}",
            output
        );
    }

    #[fuchsia::test]
    async fn test_record_mode_missing_log_root_fails() {
        let temp = tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let (fake_recorder, mut params) = record_params_with_temp(root);
        params.log_root = None;
        missing_field_test(fake_recorder, params).await;
    }

    #[fuchsia::test]
    async fn test_record_mode_missing_output_dir_fails() {
        let temp = tempdir().unwrap();
        let root = temp.path().to_path_buf();
        let (fake_recorder, mut params) = record_params_with_temp(root);
        params.output_dir = None;
        missing_field_test(fake_recorder, params).await;
    }
}
