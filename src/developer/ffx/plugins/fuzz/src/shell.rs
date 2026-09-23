// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fuzzer::Fuzzer;
use crate::reader::{ParsedCommand, Reader};
use anyhow::{Context as _, Error, Result, anyhow, bail};
use errors::ffx_bail;
use fdomain_client::fidl::{DiscoverableProtocolMarker as _, Proxy as _};
use fdomain_fuchsia_developer_remotecontrol as rcs;
use fdomain_fuchsia_fuzzer as fuzz;
use ffx_config::EnvironmentContext;
use ffx_fuzz_args::*;
use fidl_fuchsia_sys2 as fsys;
use fuchsia_fuzzctl_fdomain::{
    Manager, MonotonicDuration, OutputSink, Writer, get_corpus_type, get_fuzzer_urls,
};
use futures::{FutureExt, pin_mut, select};
use serde_json::json;
use std::cell::RefCell;
use std::fs;
use std::sync::{Arc, Mutex};
use termion::{clear, cursor};
use url::Url;

/// The default output directory variable used by `Shell::attach`.
pub const DEFAULT_FUZZING_OUTPUT_VARIABLE: &str = "fuzzer.output";

/// Interactive fuzzing shell.
pub struct Shell<R: Reader, O: OutputSink> {
    tests_json: Option<String>,
    remote_control: rcs::RemoteControlProxy,
    state: Arc<Mutex<FuzzerState>>,
    fuzzer: RefCell<Option<Fuzzer<O>>>,
    reader: RefCell<R>,
    writer: Writer<O>,
}

/// Indicates what the shell should do after trying to execute a command.
#[derive(Debug, PartialEq)]
pub enum NextAction {
    /// An `execute_*` subroutine did not handle the command; the next candidate should be tried.
    Retry(FuzzShellCommand),

    /// The command was handled, and the user should be prompted for the next command.
    Prompt,

    /// The fuzzer output was previously paused, and should be resumed.
    Resume,

    /// The command was handled as a user request to exit the shell.
    Exit,
}

impl<R: Reader, O: OutputSink> Shell<R, O> {
    /// Creates a shell that executes a sequence of fuzzer-related commands.
    ///
    /// The shell may be interactive or scripted, depending on its `reader`. It will produce output
    /// using its `writer`. The fuzzers available for the shell to interact with are discovered
    /// by examining the `tests_json`.
    pub fn new(
        tests_json: Option<String>,
        remote_control: rcs::RemoteControlProxy,
        mut reader: R,
        writer: &Writer<O>,
    ) -> Self {
        let state = Arc::new(Mutex::new(FuzzerState::Detached));
        reader.start(tests_json.clone(), Arc::clone(&state));
        Self {
            tests_json,
            remote_control,
            state,
            fuzzer: RefCell::new(None),
            reader: RefCell::new(reader),
            writer: writer.clone(),
        }
    }

    /// Runs a blocking loop executing commands from the `reader`.
    pub async fn run(&mut self, context: &EnvironmentContext) -> Result<()> {
        loop {
            if let Some(command) = self.prompt().await {
                let result = self.execute(context, command).await?;
                match result {
                    NextAction::Prompt => {}
                    NextAction::Exit => break,
                    _ => unreachable!(),
                };
            };
        }
        Ok(())
    }

    async fn prompt(&self) -> Option<FuzzShellCommand> {
        let parsed = {
            let mut reader = self.reader.borrow_mut();
            reader.prompt().await
        };
        match parsed {
            Some(ParsedCommand::Empty) | Some(ParsedCommand::Pause) => None,
            Some(ParsedCommand::Invalid(output)) => {
                self.writer.error(output);
                self.writer.println("Command is unrecognized or invalid for the current state.");
                self.writer.println("Try 'help' to list recognized.");
                self.writer.println("Try 'status' to check the current state.");
                None
            }
            Some(ParsedCommand::Usage(output)) => {
                self.writer.println(output);
                None
            }
            Some(ParsedCommand::Valid(command)) => Some(command),
            None => Some(FuzzShellCommand {
                command: FuzzShellSubcommand::Exit(ExitShellSubcommand {}),
            }),
        }
    }

    /// Executes a single command.
    /// Never returns `NextAction::Retry(_)`.
    pub async fn execute(
        &self,
        context: &EnvironmentContext,
        args: FuzzShellCommand,
    ) -> Result<NextAction> {
        // Dispatch based on current fuzzer state.
        let fuzzer = self.take_fuzzer();
        let next_action = match (self.get_state(), fuzzer.as_ref()) {
            (FuzzerState::Detached, None) => self.execute_detached(context, args).await,
            (FuzzerState::Idle, Some(fuzzer)) => self.execute_idle(args, fuzzer).await,
            (FuzzerState::Running, Some(_)) => unreachable!(),
            (state, fuzzer) => {
                unreachable!("invalid state: ({:?}, {:?}) for {:?}", state, fuzzer, args)
            }
        };
        // Replace the fuzzer unless the shell detached. Execution may have changed state.
        self.writer.resume();
        match (next_action, fuzzer) {
            (Ok(next_action), None) => Ok(next_action),
            (Ok(next_action), Some(fuzzer)) => {
                match self.get_state() {
                    FuzzerState::Detached => {}
                    FuzzerState::Idle | FuzzerState::Running => self.put_fuzzer(fuzzer),
                };
                Ok(next_action)
            }
            (Err(e), None) => self.report_error(e),
            (Err(e), Some(fuzzer)) => {
                let result = self.report_error(e);
                let url = fuzzer.url();
                let _ = self.stop(url.as_ref()).await;
                result
            }
        }
    }

    fn report_error(&self, e: Error) -> Result<NextAction> {
        let is_interactive = self.reader.borrow().is_interactive();
        let err_msg = format!("failed to execute command: {:?}", e);
        if is_interactive {
            self.writer.error(err_msg);
        } else {
            ffx_bail!("{}", err_msg);
        }
        Ok(NextAction::Prompt)
    }

    /// Handles commands that can be run in any state.
    /// May return `NextAction::Retry(_)`.
    async fn execute_any(&self, args: FuzzShellCommand) -> Result<NextAction> {
        match args.command {
            FuzzShellSubcommand::List(ListSubcommand { json_file, pattern }) => {
                let tests_json = json_file.or_else(|| self.tests_json.clone());
                let mut urls =
                    get_fuzzer_urls(&tests_json).context("failed to get URLs to list")?;
                if urls.is_empty() {
                    self.writer.println(
                        "Empty list: did you include '-fuzz-with' in your 'fx set' command ?",
                    );
                }
                if let Some(pattern) = pattern {
                    let globbed = glob::Pattern::new(&pattern)
                        .context("failed to create glob from pattern")?;
                    urls.retain(|url| globbed.matches(url.as_str()));
                }
                let urls: Vec<_> = urls.into_iter().map(|url| url.to_string()).collect();
                let urls = json!(urls);
                let fuzzers = serde_json::to_string_pretty(&urls)?;
                self.writer.println(fuzzers);
            }
            FuzzShellSubcommand::Clear(ClearShellSubcommand {}) => {
                self.writer.print(format!("{}{}", clear::All, cursor::Goto(1, 1)));
            }
            FuzzShellSubcommand::History(HistoryShellSubcommand {}) => {
                let mut count = 1;
                let history = self.reader.borrow().history();
                for entry in history.into_iter() {
                    self.writer.println(format!("{} {}", count, entry));
                    count += 1;
                }
            }
            _ => return Ok(NextAction::Retry(args)),
        };
        Ok(NextAction::Prompt)
    }

    /// Handles commands that can only be run when no fuzzer is attached.
    /// Never returns `NextAction::Retry(_)`.
    async fn execute_detached(
        &self,
        context: &EnvironmentContext,
        args: FuzzShellCommand,
    ) -> Result<NextAction> {
        let args = match self.execute_any(args).await {
            Ok(NextAction::Retry(args)) => args,
            other => return other,
        };
        match args.command {
            FuzzShellSubcommand::Attach(AttachShellSubcommand {
                url,
                output,
                no_stdout,
                no_stderr,
                no_syslog,
            }) => self
                .attach(context, &url, output, no_stdout, no_stderr, no_syslog)
                .await
                .context("failed to attach fuzzer"),
            FuzzShellSubcommand::Status(StatusShellSubcommand {}) => {
                self.writer.println("No fuzzer attached.");
                Ok(NextAction::Prompt)
            }
            FuzzShellSubcommand::Stop(StopSubcommand { url, .. }) => {
                match url {
                    Some(url) => self.stop(&url).await.context("failed to stop fuzzer")?,
                    None => self.writer.error("invalid command: no fuzzer attached."),
                };
                Ok(NextAction::Prompt)
            }
            FuzzShellSubcommand::Exit(ExitShellSubcommand {}) => {
                self.writer.println("Exiting...");
                Ok(NextAction::Exit)
            }
            _ => {
                self.writer.error("invalid command: no fuzzer attached.");
                Ok(NextAction::Prompt)
            }
        }
    }

    /// Handles commands that can be run when a fuzzer is idle or running.
    /// May return `NextAction::Retry(_)`.
    async fn execute_attached(
        &self,
        args: FuzzShellCommand,
        fuzzer: &Fuzzer<O>,
    ) -> Result<NextAction> {
        let args = match self.execute_any(args).await {
            Ok(NextAction::Retry(args)) => args,
            other => return other,
        };
        match args.command {
            FuzzShellSubcommand::Attach(AttachShellSubcommand { .. }) => {
                self.writer.error("invalid command: a fuzzer is already attached.");
            }
            FuzzShellSubcommand::Get(GetShellSubcommand { name }) => {
                fuzzer.get(name).await.context("failed to get option for fuzzer")?;
            }
            FuzzShellSubcommand::Add(AddShellSubcommand { input, seed }) => {
                fuzzer
                    .add(input, get_corpus_type(seed))
                    .await
                    .context("failed to add fuzz input to fuzzer corpus")?;
            }
            FuzzShellSubcommand::Status(StatusShellSubcommand {}) => {
                self.display_status(fuzzer).await.context("failed to display status for fuzzer")?;
            }
            FuzzShellSubcommand::Fetch(FetchShellSubcommand { seed }) => {
                let corpus_type = get_corpus_type(seed);
                fuzzer.fetch(corpus_type).await.context("failed to fetch corpus from fuzzer")?;
            }
            FuzzShellSubcommand::Detach(DetachShellSubcommand {}) => {
                self.writer.println(format!("Detaching from '{}'...", fuzzer.url()));
                self.detach();
                self.writer.println(format!("Detached."));
            }
            FuzzShellSubcommand::Stop(StopSubcommand { url, .. }) => {
                let url = url.unwrap_or_else(|| fuzzer.url().to_string());
                self.stop(&url).await.context("failed to stop fuzzer")?;
            }
            FuzzShellSubcommand::Exit(ExitShellSubcommand {}) => {
                self.writer.println("Exiting...");
                self.detach();
                return Ok(NextAction::Exit);
            }
            _ => return Ok(NextAction::Retry(args)),
        };
        Ok(NextAction::Prompt)
    }

    /// Handles commands that can only be run when a fuzzer is idle.
    /// Never returns `NextAction::Retry(_)`.
    async fn execute_idle(&self, args: FuzzShellCommand, fuzzer: &Fuzzer<O>) -> Result<NextAction> {
        let args = match self.execute_attached(args, fuzzer).await {
            Ok(NextAction::Retry(args)) => args,
            other => return other,
        };
        match args.command {
            FuzzShellSubcommand::Set(SetShellSubcommand { name, value }) => {
                fuzzer.set(&name, &value).await.context("failed to set option on fuzzer")?;
                return Ok(NextAction::Prompt);
            }
            FuzzShellSubcommand::Run(_)
            | FuzzShellSubcommand::Try(_)
            | FuzzShellSubcommand::Cleanse(_)
            | FuzzShellSubcommand::Minimize(_)
            | FuzzShellSubcommand::Merge(_) => {}
            _ => {
                self.writer.error("invalid command: no fuzzer running.");
                return Ok(NextAction::Prompt);
            }
        };
        let is_interactive = self.reader.borrow().is_interactive();
        if is_interactive {
            self.writer.println("Starting workflow...");
            self.writer.println("Press any key to pause fuzzer output.");
        }
        self.set_state(FuzzerState::Running);
        match args.command {
            FuzzShellSubcommand::Run(RunShellSubcommand { runs, time }) => {
                fuzzer.run(runs, time).await.context("fuzzing failed")?;
            }
            FuzzShellSubcommand::Try(TryShellSubcommand { input }) => {
                fuzzer.try_one(input).await.context("failed to try input")?;
            }
            FuzzShellSubcommand::Minimize(MinimizeShellSubcommand { input, runs, time }) => {
                fuzzer.minimize(input, runs, time).await.context("failed to minimize input")?;
            }
            FuzzShellSubcommand::Cleanse(CleanseShellSubcommand { input }) => {
                fuzzer.cleanse(input).await.context("failed to cleanse input")?;
            }
            FuzzShellSubcommand::Merge(MergeShellSubcommand {}) => {
                fuzzer.merge().await.context("failed to perform merge")?;
            }
            _ => unreachable!(),
        };
        self.finish_workflow(fuzzer).await
    }

    /// Handles commands that can only be run when a fuzzer is running.
    /// Never returns `NextAction::Retry(_)`.
    async fn execute_running(
        &self,
        args: FuzzShellCommand,
        fuzzer: &Fuzzer<O>,
    ) -> Result<NextAction> {
        let args = match self.execute_attached(args, fuzzer).await {
            Ok(NextAction::Retry(args)) => args,
            other => {
                return other;
            }
        };
        match args.command {
            FuzzShellSubcommand::Resume(ResumeShellSubcommand {}) => {
                self.writer.println("Resuming fuzzer output...");
                self.writer.println("Press any key to pause fuzzer output.");
                return Ok(NextAction::Resume);
            }
            _ => {
                self.writer.error("invalid command: a long-running workflow is in progress.");
            }
        };
        Ok(NextAction::Prompt)
    }

    // Subroutines used by the execution routines above.

    // Connects to a fuzzer given by the `url`.
    async fn attach(
        &self,
        context: &EnvironmentContext,
        url: &str,
        output: Option<String>,
        no_stdout: bool,
        no_stderr: bool,
        no_syslog: bool,
    ) -> Result<NextAction> {
        let url = Url::parse(url).context("invalid fuzzer URL")?;
        let output = match (output, context.get(DEFAULT_FUZZING_OUTPUT_VARIABLE)) {
            (Some(output), _) | (None, Ok(output)) => output,
            _ => {
                self.writer.error("output directory is not set.");
                self.writer.println("You can specify the location with the `--output` option.");
                self.writer.println("You can also set a default output directory using:");
                self.writer.println(format!(
                    "  `ffx config set {} <path>`",
                    DEFAULT_FUZZING_OUTPUT_VARIABLE
                ));
                return Ok(NextAction::Prompt);
            }
        };
        let metadata = fs::metadata(&output)
            .with_context(|| format!("invalid output directory: '{}'", output))?;
        if !metadata.is_dir() {
            bail!("not a directory: '{}'", output);
        }
        if metadata.permissions().readonly() {
            bail!("output directory is read-only: '{}'", output);
        }

        // Pre-emptively pause, and then resume if the fuzzer is currently running.
        self.writer.pause();
        self.writer.println(format!("Attaching to '{}'...", url));
        let manager = self.connect_to_manager().await.context("failed to connect to manager")?;
        let controller = manager.connect(&url).await.context("failed to connect to fuzzer")?;
        let mut fuzzer = Fuzzer::new(&url, controller, output, &self.writer);
        if !no_stdout {
            let output = fuzz::TestOutput::Stdout;
            let rx = manager.get_output(&url, output).await.context("failed to get stdout")?;
            fuzzer.set_output(rx, output).context("failed to set stdout")?;
        }
        if !no_stderr {
            let output = fuzz::TestOutput::Stderr;
            let rx = manager.get_output(&url, output).await.context("failed to get stderr")?;
            fuzzer.set_output(rx, output).context("failed to set stderr")?;
        }
        if !no_syslog {
            let output = fuzz::TestOutput::Syslog;
            let rx = manager.get_output(&url, output).await.context("failed to get syslog")?;
            fuzzer.set_output(rx, output).context("failed to set syslog")?;
        }
        let status = fuzzer.status().await.context("failed to get status from fuzzer")?;
        let result = match status.running {
            Some(true) => {
                // Fuzzer is running; resume. The next action and the fuzzer state depends on
                // whether the workflow runs to completion or the user pauses it and executes
                // additional commands.
                self.set_state(FuzzerState::Running);
                self.writer.println("Attached; fuzzer is running.");
                self.writer.println("Resuming fuzzer output...");
                self.writer.println("Press any key to pause fuzzer output.");
                self.finish_workflow(&fuzzer).await
            }
            _ => {
                // Fuzzer is idle; resume the output and drop into a prompt.
                self.set_state(FuzzerState::Idle);
                self.writer.resume();
                self.writer.println("Attached; fuzzer is idle.");
                Ok(NextAction::Prompt)
            }
        };
        self.put_fuzzer(fuzzer);
        result
    }

    // Returns status from the currently attached fuzzer, and updates the shell state accordingly.
    async fn get_status(&self, fuzzer: &Fuzzer<O>) -> Result<fuzz::Status> {
        let status = fuzzer.status().await.context("failed to get status from fuzzer")?;
        match status.running {
            Some(true) => self.set_state(FuzzerState::Running),
            _ => self.set_state(FuzzerState::Idle),
        };
        Ok(status)
    }

    // Prints status information from an attached fuzzer.
    async fn display_status(&self, fuzzer: &Fuzzer<O>) -> Result<()> {
        let status = self.get_status(fuzzer).await.context("failed to get status to display")?;
        match self.get_state() {
            FuzzerState::Idle => {
                self.writer.println(format!("{} is idle.", fuzzer.url()));
            }
            FuzzerState::Running => {
                self.writer.println(format!("{} is running.", fuzzer.url()));
                if let Some(runs) = status.runs {
                    self.writer.println(format!("  Runs performed: {}", runs));
                };
                if let Some(elapsed) = status.elapsed {
                    let seconds = MonotonicDuration::from_nanos(elapsed).into_seconds();
                    self.writer.println(format!("    Time elapsed: {} seconds", seconds));
                };
                if let (Some(covered_pcs), Some(covered_features)) =
                    (status.covered_pcs, status.covered_features)
                {
                    self.writer.println(format!(
                        "        Coverage: {} PCs, {} features",
                        covered_pcs, covered_features
                    ));
                };
                if let (Some(corpus_num_inputs), Some(corpus_total_size)) =
                    (status.corpus_num_inputs, status.corpus_total_size)
                {
                    self.writer.println(format!(
                        "     Corpus size: {} inputs, {} total bytes",
                        corpus_num_inputs, corpus_total_size
                    ));
                }
            }
            _ => unreachable!(),
        };
        Ok(())
    }

    async fn finish_workflow(&self, fuzzer: &Fuzzer<O>) -> Result<NextAction> {
        let is_interactive = self.reader.borrow().is_interactive();
        let workflow_fut = fuzzer.wait_for_artifact().fuse();
        let pause_fut = self.handle_pause(fuzzer).fuse();
        pin_mut!(workflow_fut, pause_fut);
        loop {
            select! {
                result = workflow_fut => {
                    // At this point, the `rustyline` editor is sitting in a blocking, uncancellable
                    // read of stdin at this point. Cancelling that read would require reworking
                    // `rustyline` to reduce its portability and include unsafe code to use a
                    // POSIX-style epoll API. It's simpler to just have the user press a key.
                    match result {
                        Ok(true) => {
                            // The workflow completed.
                            self.set_state(FuzzerState::Idle);
                            if is_interactive {
                              self.writer.println("Workflow complete. Press any key to continue...");
                            }
                        }
                        Ok(false) => {
                            // The workflow was stopped.
                            self.set_state(FuzzerState::Detached);
                        }
                        Err(e) => {
                            self.set_state(FuzzerState::Idle);
                            self.writer.error(format!("{:?}", e));
                            if is_interactive {
                              self.writer.println("Workflow failed. Press any key to continue...");
                            }
                        }
                    }
                }
                next_action = pause_fut => {
                    if next_action != NextAction::Resume {
                        return Ok(next_action);
                    }
                }
                complete => return Ok(NextAction::Prompt),
            }
        }
    }

    // Repeatedly waits for the output to be paused while a long-running workflow is executing.
    async fn handle_pause(&self, fuzzer: &Fuzzer<O>) -> NextAction {
        loop {
            {
                let mut reader = self.reader.borrow_mut();
                if !reader.is_interactive() {
                    return NextAction::Resume;
                }
                if let Err(e) = reader.pause().await {
                    self.writer.error(e);
                    return NextAction::Prompt;
                }
            }
            if self.get_state() != FuzzerState::Running {
                return NextAction::Prompt;
            }
            self.writer.pause();
            let next_action = self.while_paused(fuzzer).await;
            if next_action != NextAction::Resume {
                return next_action;
            }
            self.writer.resume();
        }
    }

    // Runs the prompt loop and executes the command while the output of a long-running workflow is
    // paused.
    async fn while_paused(&self, fuzzer: &Fuzzer<O>) -> NextAction {
        self.writer.println("Fuzzer output has been paused.");
        self.writer.println("To resume output, use the `resume` command.");
        loop {
            let result = match self.prompt().await {
                Some(command) => self.execute_running(command, fuzzer).await,
                None => Ok(NextAction::Prompt),
            };
            match result {
                Ok(NextAction::Retry(_)) => {
                    self.writer.error("invalid command: A long-running workflow is in progress.")
                }
                Ok(NextAction::Prompt) => {
                    // Executed "stop" or "detach".
                    if self.get_state() == FuzzerState::Detached {
                        return NextAction::Prompt;
                    }
                }
                Ok(next_action) => {
                    // Executed "resume" or "exit".
                    return next_action;
                }
                Err(e) => self.writer.error(e),
            };
        }
    }

    fn detach(&self) {
        match self.get_state() {
            FuzzerState::Idle => self.writer.println("Note: fuzzer is idle but still alive."),
            FuzzerState::Running => self.writer.println("Note: fuzzer will continue running."),
            _ => unreachable!(),
        };
        self.writer.println(format!("To reconnect later, use the 'attach' command."));
        self.writer.println(format!("To stop this fuzzer, use 'stop'. command"));
        self.set_state(FuzzerState::Detached);
    }

    async fn stop(&self, url: &str) -> Result<()> {
        let url = Url::parse(url).context("invalid fuzzer URL")?;
        self.writer.println(format!("Stopping '{}'...", url));
        self.set_state(FuzzerState::Detached);
        let manager = self.connect_to_manager().await.context("failed to connect to manager")?;
        let stopped = manager.stop(&url).await.context("manager failed to stop fuzzer")?;
        if stopped {
            self.writer.println("Stopped.");
        } else {
            self.writer.println("Fuzzer is not running.");
        }
        Ok(())
    }

    async fn connect_to_manager(&self) -> Result<Manager> {
        // Try to connect via fuchsia.developer.remotecontrol/RemoteControl.ConnectCapability.
        let (proxy, server) = self.remote_control.domain().create_proxy::<fuzz::ManagerMarker>();
        self.remote_control
            .connect_capability(
                "/core/fuzz-manager",
                fsys::OpenDirType::ExposedDir,
                fuzz::ManagerMarker::PROTOCOL_NAME,
                server.into_channel(),
            )
            .await?
            .map_err(|e| anyhow!("{:?}", e))
            .context("failed to connect to fuzz-manager")?;
        return Ok(Manager::new(proxy));
    }

    // Helper functions to make it easier to access and mutate `Arc` and `RefCell` fields, and to
    // limit the scope of the lock guards and borrows.

    fn get_state(&self) -> FuzzerState {
        self.state.lock().unwrap().clone()
    }

    fn set_state(&self, desired: FuzzerState) {
        let mut state_mut = self.state.lock().unwrap();
        *state_mut = desired
    }

    fn take_fuzzer(&self) -> Option<Fuzzer<O>> {
        self.fuzzer.borrow_mut().take()
    }

    fn put_fuzzer(&self, fuzzer: Fuzzer<O>) {
        let mut fuzzer_mut = self.fuzzer.borrow_mut();
        *fuzzer_mut = Some(fuzzer);
    }
}

#[cfg(test)]
mod test_fixtures {
    use super::Shell;
    use crate::reader::test_fixtures::ScriptReader;
    use anyhow::{Context as _, Result, anyhow};
    use fdomain_client::fidl::{DiscoverableProtocolMarker as _, ServerEnd};
    use fdomain_fuchsia_developer_remotecontrol as rcs;
    use fdomain_fuchsia_fuzzer::{self as fuzz, Result_ as FuzzResult};
    use ffx_config::EnvironmentContext;
    use ffx_fuzz_args::FuzzerState;
    use fidl_fuchsia_sys2 as fsys;
    use fuchsia_async as fasync;
    use fuchsia_fuzzctl_fdomain::MonotonicDuration;
    use fuchsia_fuzzctl_test_fdomain::{
        BufferSink, FakeController, TEST_URL, Test, create_task, serve_manager,
    };
    use futures::StreamExt;
    use std::fmt::Display;
    use std::path::PathBuf;
    use std::sync::Arc;
    use url::Url;

    /// Represents a set of test fakes used to test `Shell`.
    pub struct ShellScript {
        shell: Shell<ScriptReader, BufferSink>,
        _rcs_task: fasync::Task<()>,
        _client: Arc<fdomain_client::Client>,
        state: FuzzerState,
        output_dir: PathBuf,
        runs_indefinitely: bool,
    }

    impl ShellScript {
        /// Creates a shell with fakes suitable for testing.
        pub fn try_new(test: &mut Test) -> Result<Self> {
            let url = Url::parse(TEST_URL).context("failed to parse test URL")?;
            let urls = vec![&url];
            let tests_json = test
                .create_tests_json(urls.iter())
                .context("failed to write URLs for shell script")?;
            let tests_json = Some(tests_json.to_string_lossy().to_string());
            let client = test.domain();
            let (proxy, server_end) = client.create_proxy::<rcs::RemoteControlMarker>();
            let rcs_task = create_task(serve_rcs(server_end, test.clone()), test.writer());
            let reader = ScriptReader::new();
            let shell = Shell::new(tests_json, proxy, reader, test.writer());
            Ok(Self {
                shell,
                _rcs_task: rcs_task,
                _client: client,
                state: FuzzerState::Detached,
                output_dir: test.root_dir().to_path_buf(),
                runs_indefinitely: false,
            })
        }

        /// Bootstraps the `ShellScript` to emulate having a fuzzer in a long-running workflow.
        pub async fn create_running(test: &mut Test) -> Result<(Self, FakeController)> {
            let mut script = Self::try_new(test).context("failed to create shell script")?;
            script.runs_indefinitely = true;
            let fuzzer = script.attach(test);
            fuzzer.set_result(Ok(FuzzResult::NoErrors));
            script.add(test, "run");
            test.output_matches("Configuring fuzzer...");
            test.output_matches("Running fuzzer...");
            script.interrupt(test).await;
            Ok((script, fuzzer))
        }

        /// Adds input commands and output expectations for attaching to a fuzzer.
        pub fn attach(&mut self, test: &mut Test) -> FakeController {
            let cmdline = format!("attach {} -o {}", TEST_URL, self.output_dir.to_string_lossy());
            self.add(test, cmdline);
            test.output_matches(format!("Attaching to '{}'...", TEST_URL));
            test.output_matches("Attached; fuzzer is idle.");
            self.state = FuzzerState::Idle;
            test.controller()
        }

        /// Adds an input command to run as part of a test.
        pub fn add<S: AsRef<str> + Display>(&mut self, test: &mut Test, command: S) {
            // Handle special cases that don't complete interrupted workflows.
            let cmd = match command.as_ref().split_once(' ') {
                Some((cmd, _)) => cmd,
                None => command.as_ref(),
            };
            match cmd {
                "detach" | "stop" => {
                    self.state = FuzzerState::Detached;
                }
                "try" | "run" | "minimize" | "cleanse" | "merge" => {
                    if self.state == FuzzerState::Idle {
                        test.output_matches("Starting workflow...");
                        test.output_matches("Press any key to pause fuzzer output.");
                        self.state = FuzzerState::Running;
                    }
                }
                _ => {}
            };
            let mut reader = self.shell.reader.borrow_mut();
            reader.add(command);
        }

        /// Processes the previously `add`ed commands using the underlying `Shell`.
        pub async fn run(&mut self, context: &EnvironmentContext, test: &mut Test) -> Result<()> {
            // Handle the case where a workflow expected to fail.
            if test.controller().get_result().is_err() && self.state == FuzzerState::Running {
                self.state = FuzzerState::Detached;
            }

            // If this script is not expected to run indefinitely, but the state still indicates the
            // test is running after all other commands have been added, then this is a long-running
            // workflow that runs to completion.
            if !self.runs_indefinitely && self.state == FuzzerState::Running {
                let mut reader = self.shell.reader.borrow_mut();
                reader.interrupt(MonotonicDuration::from_millis(100)).await;
                test.output_matches("Workflow complete. Press any key to continue...");
                self.state = FuzzerState::Idle;
            }

            // The shell automatically exits on EOF.
            test.output_matches("Exiting...");
            self.detach_from_state(test, self.state);

            self.shell.run(context).await
        }

        /// Simulates the user pressing a key to interrupt output from a long-running workflow.
        pub async fn interrupt(&self, test: &mut Test) {
            let mut reader = self.shell.reader.borrow_mut();
            reader.interrupt(MonotonicDuration::from_millis(10)).await;
            test.output_matches("Fuzzer output has been paused.");
            test.output_matches("To resume output, use the `resume` command.");
        }

        /// Simulates detaching from a fuzzer, which may be running or idle.
        pub async fn detach(&mut self, test: &mut Test) {
            let state = self.state;
            self.add(test, "detach");
            test.output_matches(format!("Detaching from '{}'...", TEST_URL));
            self.detach_from_state(test, state);
            test.output_matches("Detached.");
        }

        fn detach_from_state(&mut self, test: &mut Test, state: FuzzerState) {
            self.state = FuzzerState::Detached;
            match state {
                FuzzerState::Idle => test.output_matches("Note: fuzzer is idle but still alive."),
                FuzzerState::Running => test.output_matches("Note: fuzzer will continue running."),
                _ => return,
            };
            test.output_matches("To reconnect later, use the 'attach' command.");
            test.output_matches("To stop this fuzzer, use 'stop'. command");
        }
    }

    async fn serve_rcs(server_end: ServerEnd<rcs::RemoteControlMarker>, test: Test) -> Result<()> {
        let mut stream = server_end.into_stream();
        let mut task = None;
        while let Some(request) = stream.next().await {
            match request {
                Ok(rcs::RemoteControlRequest::ConnectCapability {
                    moniker,
                    capability_set,
                    capability_name,
                    server_channel,
                    responder,
                }) => {
                    assert_eq!(moniker, "/core/fuzz-manager");
                    assert_eq!(capability_set, fsys::OpenDirType::ExposedDir);
                    assert_eq!(capability_name, fuzz::ManagerMarker::PROTOCOL_NAME);
                    let server_end = ServerEnd::<fuzz::ManagerMarker>::new(server_channel);
                    responder.send(Ok(()))?;
                    task =
                        Some(create_task(serve_manager(server_end, test.clone()), test.writer()));
                }
                Err(e) => return Err(anyhow!(e)),
                _ => todo!("not yet implemented"),
            }
        }
        if let Some(task) = task.take() {
            task.await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::DEFAULT_FUZZING_OUTPUT_VARIABLE;
    use super::test_fixtures::ShellScript;
    use anyhow::Result;
    use fdomain_fuchsia_fuzzer::{self as fuzz, Result_ as FuzzResult};
    use fuchsia_fuzzctl_fdomain::{MonotonicDuration, digest_path};
    use fuchsia_fuzzctl_test_fdomain::{TEST_URL, Test, verify_saved};
    use std::path::PathBuf;
    use zx_status as zx;

    #[fuchsia::test]
    async fn test_empty() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;
        script.add(&mut test, "");
        script.add(&mut test, "   ");
        script.add(&mut test, "# a comment");
        script.add(&mut test, "   # another comment");
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_invalid() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;
        script.add(&mut test, "foobar");
        test.output_includes("Unrecognized argument: foobar");
        test.output_matches("Command is unrecognized or invalid for the current state.");
        test.output_matches("Try 'help' to list recognized.");
        test.output_matches("Try 'status' to check the current state.");
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_list() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Can 'list' when detached. Try both empty and non-empty lists of URLs.
        let urls: Vec<&str> = vec![];
        test.create_tests_json(urls.iter())?;
        script.add(&mut test, "list");
        test.output_matches("Empty list: did you include '-fuzz-with' in your 'fx set' command ?");
        test.output_matches("[]");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        let mut script = ShellScript::try_new(&mut test)?;
        let urls = vec![
            "fuchsia-pkg://fuchsia.com/test-fuzzers#meta/foo-fuzzer.cm",
            "fuchsia-pkg://fuchsia.com/test-fuzzers#meta/bar-fuzzer.cm",
            "fuchsia-pkg://fuchsia.com/test-fuzzers#meta/baz-fuzzer.cm",
        ];
        test.create_tests_json(urls.iter())?;
        script.add(&mut test, "list");
        test.output_matches("[");
        test.output_matches(format!("\"{}\",", urls[0]));
        test.output_matches(format!("\"{}\",", urls[1]));
        test.output_matches(format!("\"{}\"", urls[2]));
        test.output_matches("]");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Can 'list' when idle. Include a pattern.
        script = ShellScript::try_new(&mut test)?;
        test.create_tests_json(urls.iter())?;
        let _fuzzer = script.attach(&mut test);
        script.add(&mut test, "list -p *ba?-fuzzer.cm");
        test.output_matches("[");
        test.output_matches(format!("\"{}\",", urls[1]));
        test.output_matches(format!("\"{}\"", urls[2]));
        test.output_matches("]");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Can 'list' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        test.create_tests_json(urls.iter())?;
        script.add(&mut test, "list");
        test.output_matches("[");
        test.output_matches(format!("\"{}\",", urls[0]));
        test.output_matches(format!("\"{}\",", urls[1]));
        test.output_matches(format!("\"{}\"", urls[2]));
        test.output_matches("]");
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    // This test is larger than some of the others as it is used to test autocomplation of commands,
    // options, files, and URLs.
    #[fuchsia::test]
    async fn test_attach() -> Result<()> {
        let env = ffx_config::test_init().expect("Unable to initialize ffx_config.");
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // URL must be valid
        script.add(&mut test, "attach invalid-url");
        test.output_includes("invalid fuzzer URL");

        // Output directory must be provided or set in config.
        script.add(&mut test, format!("attach {}", TEST_URL));
        test.output_includes("output directory is not set");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Output directory from config is checked.
        test = Test::try_new()?;
        script = ShellScript::try_new(&mut test)?;
        let mut badpath = PathBuf::from(test.root_dir());
        badpath.push("invalid");

        let env2 = ffx_config::test_env()
            .user_config(DEFAULT_FUZZING_OUTPUT_VARIABLE, serde_json::json!(&badpath))
            .build()
            .unwrap();
        script.add(&mut test, format!("attach {}", TEST_URL));
        test.output_includes("invalid output directory");

        // Provided output directory is checked.
        script.add(&mut test, format!("attach -o {} {}", badpath.to_string_lossy(), TEST_URL));
        test.output_includes("invalid output directory");

        // Provided output directory must be a directory.
        let output_dir = test.create_dir("output")?;
        let test_files = vec!["test1"];
        test.create_test_files(&output_dir, test_files.iter())?;
        let cmdline = format!("attach -o {}/test1 {}", output_dir.to_string_lossy(), TEST_URL);
        script.add(&mut test, cmdline);
        test.output_includes("not a directory");

        // Can 'attach' when detached.
        script.attach(&mut test);

        // Cannot 'attach' when attached.
        let cmdline = format!("attach -o {} {}", output_dir.to_string_lossy(), TEST_URL);
        script.add(&mut test, cmdline);
        test.output_includes("invalid command: a fuzzer is already attached.");
        script.run(&env2.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_get() -> Result<()> {
        let env = ffx_config::test_env().build()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Cannot 'get' when detached.
        script.add(&mut test, "get runs");
        test.output_includes("invalid command: no fuzzer attached.");

        // Can 'get' when idle.
        let _fuzzer = script.attach(&mut test);
        script.add(&mut test, "get runs");
        test.output_matches("runs: 0");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Can 'get' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, "get runs");
        test.output_matches("runs: 0");
        script.detach(&mut test).await;

        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_set() -> Result<()> {
        let env = ffx_config::test_env().build()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Cannot 'set' when detached.
        script.add(&mut test, "set runs 10");
        test.output_includes("invalid command: no fuzzer attached.");

        // Can 'set' when idle.
        let _fuzzer = script.attach(&mut test);
        script.add(&mut test, "set runs 10");
        test.output_matches("Configuring fuzzer...");
        test.output_matches("Option 'runs' set to 10");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Cannot 'set' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, "set runs 20");
        test.output_includes("invalid command: a long-running workflow is in progress.");
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_add() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;
        let corpus_dir = test.corpus_dir(fuzz::Corpus::Live);

        // 'add' can take a dir as an argument.
        let test_files = vec!["test1", "test2"];
        test.create_test_files(&corpus_dir, test_files.iter())?;

        // Cannot 'add' when detached.
        script.add(&mut test, format!("add {}", corpus_dir.to_string_lossy()));
        test.output_includes("invalid command: no fuzzer attached.");

        // Can 'add' when idle
        let _fuzzer = script.attach(&mut test);
        script.add(&mut test, format!("add {}", corpus_dir.to_string_lossy()));
        test.output_matches("Adding inputs to fuzzer corpus...");
        test.output_matches("Added 2 inputs totaling 10 bytes to the live corpus.");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Can 'add' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, format!("add {}", corpus_dir.to_string_lossy()));
        test.output_matches("Adding inputs to fuzzer corpus...");
        test.output_matches("Added 2 inputs totaling 10 bytes to the live corpus.");
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_run() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Cannot 'run' when detached. 'run' can take flags as arguments.
        script.add(&mut test, "run -r 20");
        test.output_includes("invalid command: no fuzzer attached.");

        // Can 'run' when idle.
        let fuzzer = script.attach(&mut test);
        script.add(&mut test, "run -r 20");
        test.output_matches("Configuring fuzzer...");
        test.output_matches("Running fuzzer...");
        fuzzer.set_result(Ok(FuzzResult::Death));
        fuzzer.set_input_to_send(b"hello");
        test.output_matches("An input to the fuzzer triggered a sanitizer violation.");
        let artifact = digest_path(test.artifact_dir(), Some(FuzzResult::Death), b"hello");
        test.output_matches(format!("Input saved to '{}'", artifact.to_string_lossy()));
        script.run(&env.context, &mut test).await?;
        let options = fuzzer.get_options();
        assert_eq!(options.runs, Some(20));
        verify_saved(&artifact, b"hello")?;
        test.verify_output()?;

        // Errors are propagated correctly.
        let mut script = ShellScript::try_new(&mut test)?;
        let fuzzer = script.attach(&mut test);
        script.add(&mut test, "run -r 20");
        test.output_matches("Configuring fuzzer...");
        test.output_matches("Running fuzzer...");
        fuzzer.set_result(Err(zx::Status::IO));
        test.output_includes("`fuchsia.fuzzer.Controller/Fuzz` returned: ZX_ERR_IO");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Cannot 'run' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, "run -t 20");
        test.output_includes("invalid command: a long-running workflow is in progress.");
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_try() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Cannot 'try' when detached. 'try' can take a hex value as an argument.
        script.add(&mut test, "try deadbeef");
        test.output_includes("invalid command: no fuzzer attached.");

        // Can 'try' when idle.
        let fuzzer = script.attach(&mut test);
        script.add(&mut test, "try deadbeef");
        test.output_matches("Trying an input of 4 bytes...");
        fuzzer.set_result(Ok(FuzzResult::Crash));
        test.output_matches("The input caused a process to crash.");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Errors are propagated correctly.
        let mut script = ShellScript::try_new(&mut test)?;
        let fuzzer = script.attach(&mut test);
        script.add(&mut test, "try deadbeef");
        test.output_matches("Trying an input of 4 bytes...");
        fuzzer.set_result(Err(zx::Status::INTERNAL));
        test.output_includes("`fuchsia.fuzzer.Controller/TryOne` returned: ZX_ERR_INTERNAL");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Cannot 'try' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, "try feedface");
        test.output_includes("invalid command: a long-running workflow is in progress.");
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_cleanse() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Cannot 'cleanse' when detached. 'cleanse' can take a hex value as an argument.
        script.add(&mut test, format!("cleanse {}", hex::encode("hello")));
        test.output_includes("invalid command: no fuzzer attached.");

        // Can 'cleanse' when idle.
        let fuzzer = script.attach(&mut test);
        script.add(&mut test, format!("cleanse {}", hex::encode("hello")));
        test.output_matches("Attempting to cleanse an input of 5 bytes...");
        fuzzer.set_input_to_send(b"world");
        let artifact = digest_path(test.artifact_dir(), Some(FuzzResult::Cleansed), b"world");
        test.output_matches(format!("Cleansed input written to '{}'", artifact.to_string_lossy()));
        script.run(&env.context, &mut test).await?;
        verify_saved(&artifact, b"world")?;
        test.verify_output()?;

        // Cannot 'cleanse' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, format!("cleanse {}", hex::encode("world")));
        test.output_includes("invalid command: a long-running workflow is in progress.");
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_minimize() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Cannot 'minimize' when detached. 'minimize' can take a hex value as an argument.
        script.add(&mut test, format!("minimize {}", hex::encode("hello")));
        test.output_includes("invalid command: no fuzzer attached.");

        // Can 'minimize' when idle.
        let fuzzer = script.attach(&mut test);
        script.add(&mut test, format!("minimize {} -t 10s", hex::encode("hello")));
        test.output_matches("Configuring fuzzer...");
        test.output_matches("Attempting to minimize an input of 5 bytes...");
        fuzzer.set_input_to_send(b"world");
        let artifact = digest_path(test.artifact_dir(), Some(FuzzResult::Minimized), b"world");
        test.output_matches(format!("Minimized input written to '{}'", artifact.to_string_lossy()));
        script.run(&env.context, &mut test).await?;
        verify_saved(&artifact, b"world")?;
        test.verify_output()?;

        // Cannot 'minimize' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, format!("cleanse {}", hex::encode("world")));
        test.output_includes("invalid command: a long-running workflow is in progress.");
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_merge() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Cannot 'merge' when detached.
        script.add(&mut test, format!("merge"));
        test.output_includes("invalid command: no fuzzer attached.");

        // Can 'merge' when idle.
        let fuzzer = script.attach(&mut test);
        script.add(&mut test, format!("merge"));
        fuzzer.set_input_to_send(b"foo");
        test.output_matches("Compacting fuzzer corpus...");
        test.output_matches("Merge complete.");
        test.output_matches("Retrieving fuzzer corpus...");
        test.output_matches("Retrieved 1 input totaling 3 bytes from the live corpus.");
        script.run(&env.context, &mut test).await?;
        let input = digest_path(test.corpus_dir(fuzz::Corpus::Live), None, b"foo");
        verify_saved(&input, b"foo")?;
        test.verify_output()?;

        // Cannot 'merge' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, format!("merge"));
        test.output_includes("invalid command: a long-running workflow is in progress.");
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_resume() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Cannot 'resume' when detached.
        script.add(&mut test, "resume");
        test.output_includes("invalid command: no fuzzer attached.");

        // Cannot 'resume' when idle.
        let _fuzzer = script.attach(&mut test);
        script.add(&mut test, "resume");
        test.output_includes("invalid command: no fuzzer running.");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Can 'resume' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, "resume");
        test.output_matches("Resuming fuzzer output...");
        test.output_matches("Press any key to pause fuzzer output.");
        script.interrupt(&mut test).await;
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_status() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Can get 'status' when detached.
        script.add(&mut test, "status");
        test.output_matches("No fuzzer attached.");

        // Can get 'status' when idle.
        let _fuzzer = script.attach(&mut test);
        script.add(&mut test, "status");
        test.output_matches(format!("{} is idle.", TEST_URL));
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Can get 'status' when running.
        let (mut script, fuzzer) = ShellScript::create_running(&mut test).await?;
        let status = fuzz::Status {
            // running: Some(true),
            runs: Some(1),
            elapsed: Some(MonotonicDuration::from_seconds(2).into_nanos()),
            covered_pcs: Some(3),
            covered_features: Some(4),
            corpus_num_inputs: Some(5),
            corpus_total_size: Some(6),
            ..Default::default()
        };
        fuzzer.set_status(status);
        script.add(&mut test, "status");
        test.output_matches(format!("{} is running.", TEST_URL));
        test.output_matches("Runs performed: 1");
        test.output_matches("Time elapsed: 2 seconds");
        test.output_matches("Coverage: 3 PCs, 4 features");
        test.output_matches("Corpus size: 5 inputs, 6 total bytes");
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_fetch() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Cannot 'fetch' when detached.
        script.add(&mut test, format!("fetch -s"));
        test.output_includes("invalid command: no fuzzer attached.");

        // Can 'fetch' when idle.
        let fuzzer = script.attach(&mut test);
        script.add(&mut test, format!("fetch -s"));
        fuzzer.set_input_to_send(b"bar");
        test.output_matches("Retrieving fuzzer corpus...");
        test.output_matches("Retrieved 1 input totaling 3 bytes from the seed corpus.");
        script.run(&env.context, &mut test).await?;
        let input = digest_path(test.corpus_dir(fuzz::Corpus::Seed), None, b"bar");
        verify_saved(&input, b"bar")?;
        test.verify_output()?;

        // Can 'fetch' when running.
        let (mut script, fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, format!("fetch -s"));
        fuzzer.set_input_to_send(b"bar");
        test.output_matches("Retrieving fuzzer corpus...");
        test.output_matches("Retrieved 1 input totaling 3 bytes from the seed corpus.");
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_detach() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Cannot 'detach' when detached.
        script.add(&mut test, "detach");
        test.output_includes("invalid command: no fuzzer attached.");

        // Can 'detach' when idle.
        script.attach(&mut test);
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Can 'detach' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.detach(&mut test).await;
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_stop() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Cannot 'stop' without a URL when detached.
        script.add(&mut test, "stop");
        test.output_includes("invalid command: no fuzzer attached.");

        // Trying to 'stop'  a stopped fuzzer using its URL doesn't do anything.
        script.add(&mut test, format!("stop {}", TEST_URL));
        test.output_matches(format!("Stopping '{}'...", TEST_URL));
        test.output_matches("Fuzzer is not running.");

        // Can 'stop' a detached fuzzer using its URL.
        script.attach(&mut test);
        script.detach(&mut test).await;
        script.add(&mut test, format!("stop {}", TEST_URL));
        test.output_matches(format!("Stopping '{}'...", TEST_URL));
        test.output_matches("Stopped.");

        // Can 'stop' when idle.
        script.attach(&mut test);
        script.add(&mut test, "stop");
        test.output_matches(format!("Stopping '{}'...", TEST_URL));
        test.output_matches("Stopped.");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Can 'stop' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, "stop");
        test.output_matches(format!("Stopping '{}'...", TEST_URL));
        test.output_matches("Stopped.");
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_exit() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Can 'exit' when detached. The "Exiting..." messages are expected automatically by
        // `script::run`.
        script.add(&mut test, "exit");
        script.add(&mut test, "not executed");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Can 'exit' when idle.
        script = ShellScript::try_new(&mut test)?;
        script.attach(&mut test);
        script.add(&mut test, "exit");
        script.add(&mut test, "not executed");
        script.run(&env.context, &mut test).await?;
        test.verify_output()?;

        // Can 'exit' when running.
        let (mut script, _fuzzer) = ShellScript::create_running(&mut test).await?;
        script.add(&mut test, "exit");
        script.add(&mut test, "not executed");
        script.run(&env.context, &mut test).await?;
        test.verify_output()
    }

    #[fuchsia::test]
    async fn test_clear() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Just make sure this doesn't crash.
        script.add(&mut test, "clear");

        script.run(&env.context, &mut test).await?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_history() -> Result<()> {
        let env = ffx_config::test_init()?;
        let mut test = Test::try_new()?;
        let mut script = ShellScript::try_new(&mut test)?;

        // Just make sure this doesn't crash.
        script.add(&mut test, "history");

        script.run(&env.context, &mut test).await?;
        Ok(())
    }
}
