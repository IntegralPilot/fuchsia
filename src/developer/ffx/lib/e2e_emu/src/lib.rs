// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_stream::stream;
use diagnostics_data::LogsData;
use ffx_config::environment::ExecutableKind;
use ffx_config::{ConfigMap, EnvironmentContext};
use ffx_executor::FfxExecutor;
use ffx_isolate::Isolate;
use futures::channel::mpsc::TrySendError;
use futures::{Stream, StreamExt};
use log::info;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::env;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use tempfile::TempDir;

use discovery;
use ffx_target::{Resolution, TargetInfoQuery};
use target_behavior::{ConnectionBehavior, target_interface};

#[derive(Debug, thiserror::Error)]
pub enum IsolatedEmulatorError {
    #[error("Failed to create temporary directory: {0}")]
    TempDirCreate(#[source] std::io::Error),

    #[error("Failed to create ffx isolate: {0}")]
    IsolateCreate(#[source] ffx_isolate::IsolateError),

    #[error("Ffx command {args:?} failed with stderr: {stderr}")]
    FfxCommandFailed { args: Vec<String>, stderr: String },

    #[error("No target found for emulator {}", emu_name)]
    NoTargetFound { emu_name: String },

    #[error("Failed during setup step '{step}': {source}")]
    SetupFailed { step: String, source: Box<IsolatedEmulatorError> },

    #[error("Failed to create log file at {path}: {source}")]
    LogFileCreate { path: String, source: std::io::Error },

    #[error("Failed to spawn log streaming command: {0}")]
    LogStreamSpawn(#[source] std::io::Error),

    #[error("Invalid Fuchsia repository name '{name}': {details}")]
    InvalidRepositoryName { name: String, details: String },

    #[error("JSON parsing failed: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("Child process stdout is missing")]
    NoStdout,

    #[error("Could not resolve target")]
    ResolutionError(#[from] ffx_target::FfxTargetCrateError),

    #[error("Command spawn failed: {0}")]
    CommandSpawn(#[source] std::io::Error),
}

/// An isolated environment for testing ffx against a running emulator.
pub struct IsolatedEmulator {
    emu_name: String,
    package_server_name: String,
    ffx_isolate: Isolate,
    children: Mutex<Vec<std::process::Child>>,

    // We need to hold the below variables but not interact with them.
    _temp_dir: TempDir,
}

impl IsolatedEmulator {
    pub(crate) const DEFAULT_STARTUP_TIMEOUT_SECONDS: u32 = 120;

    /// Create an isolated ffx environment and start an emulator in it using the default product
    /// bundle and package repository from the Fuchsia build directory. Streams logs in the
    /// background and allows resolving packages from universe.
    pub async fn start(name: &str) -> Result<Self, IsolatedEmulatorError> {
        Self::start_internal(name, None, None, Self::DEFAULT_STARTUP_TIMEOUT_SECONDS, true).await
    }

    /// Create an isolated ffx environment and start an emulator in it with serial number generation
    /// enabled or disabled.
    ///
    /// Note: This is a convenience helper for tests that do not need to customize the package
    /// repository or symbol index (which are resolved from the environment).
    pub async fn start_with_serial_enabled(
        name: &str,
        serial_enabled: bool,
    ) -> Result<Self, IsolatedEmulatorError> {
        Self::start_internal(
            name,
            None,
            None,
            Self::DEFAULT_STARTUP_TIMEOUT_SECONDS,
            serial_enabled,
        )
        .await
    }

    // This is private to be used for testing with a path to a different package repo. Path
    // to amber-files is optional for testing to ensure that other successful tests are actually
    // matching a developer workflow.
    async fn start_internal(
        name: &str,
        amber_files_path: Option<&str>,
        symbol_index_path: Option<&str>,
        startup_timeout_seconds: u32,
        serial_enabled: bool,
    ) -> Result<Self, IsolatedEmulatorError> {
        let emu_name = format!("{name}-emu");

        let package_repository_path = match amber_files_path {
            Some(path) => path.to_string(),
            None => std::env::var("PACKAGE_REPOSITORY_PATH")
                .expect("PACKAGE_REPOSITORY_PATH env var must be set"),
        };
        let symbol_index_path = match symbol_index_path {
            Some(path) => Some(path.to_string()),
            None => std::env::var("SYMBOL_INDEX_PATH").ok(),
        };

        info!(name:% = name; "making ffx isolate");
        let temp_dir = tempfile::TempDir::new().map_err(IsolatedEmulatorError::TempDirCreate)?;

        // Start with the non-isolated environment context - then build the isolate.
        let env_context = EnvironmentContext::detect(
            ExecutableKind::Test,
            ConfigMap::new(),
            &env::current_dir().expect("current directory"),
            None,
            false,
        )
        .expect("new detected context");

        // Create paths to the files to hold the ssh key pair.
        // The key is not generated here, since ffx will generate the
        // key if it is missing when starting an emulator or flashing a device.
        // If a private key is supplied, it is used, but the public key path
        // is still in the temp dir.
        let ssh_priv_key = temp_dir.path().join("ssh_private_key");
        let ssh_pub_key = temp_dir.path().join("ssh_public_key");

        let ffx_isolate = Isolate::new_in_test(name, ssh_priv_key.clone(), &env_context)
            .await
            .map_err(IsolatedEmulatorError::IsolateCreate)?;

        // Workaround for analytics panic in isolated environment for Googlers
        let metrics_dir = ffx_isolate.dir().join("metrics_home/.fuchsia/metrics");
        let _ = std::fs::write(metrics_dir.join("analytics-status-internal"), "0");

        let package_server_name = format!("repo-{name}-{}", std::process::id());
        let this = Self {
            emu_name,
            package_server_name,
            ffx_isolate,
            _temp_dir: temp_dir,
            children: Mutex::new(vec![]),
        };

        // now we have our isolate and can call ffx commands to configure our env and start an emu
        this.ffx(&["config", "set", "ssh.priv", &ssh_priv_key.to_string_lossy()]).await.map_err(
            |e| IsolatedEmulatorError::SetupFailed {
                step: "ssh.priv".to_string(),
                source: Box::new(e),
            },
        )?;
        this.ffx(&["config", "set", "ssh.pub", &ssh_pub_key.to_string_lossy()]).await.map_err(
            |e| IsolatedEmulatorError::SetupFailed {
                step: "ssh.pub".to_string(),
                source: Box::new(e),
            },
        )?;
        this.ffx(&["config", "set", "log.level", "debug"]).await.map_err(|e| {
            IsolatedEmulatorError::SetupFailed {
                step: "log.level".to_string(),
                source: Box::new(e),
            }
        })?;
        if let Some(symbol_index_path) = symbol_index_path {
            this.ffx(&["debug", "symbol-index", "add", &symbol_index_path]).await.map_err(|e| {
                IsolatedEmulatorError::SetupFailed {
                    step: "symbol-index".to_string(),
                    source: Box::new(e),
                }
            })?;
        }

        this.ffx(&["config", "set", "emu.serial_number.enabled", &serial_enabled.to_string()])
            .await
            .map_err(|e| IsolatedEmulatorError::SetupFailed {
                step: "emu.serial_number.enabled".to_string(),
                source: Box::new(e),
            })?;

        info!("starting emulator {}", this.emu_name);
        let emulator_log = this.ffx_isolate.log_dir().join("emulator.log").display().to_string();
        let product_bundle_path = std::env::var("PRODUCT_BUNDLE_PATH")
            .expect("PRODUCT_BUNDLE_PATH env var must be set -- run this test with 'fx test'");

        // start the emulator. The start command returns when the emulator has started and an RCS connection
        // can be made, or when the timeout was reached.
        this.ffx(&[
            "emu",
            "start",
            "--headless",
            "--net",
            "user",
            "--name",
            &this.emu_name,
            "--log",
            &*emulator_log,
            "--startup-timeout",
            &format!("{}", startup_timeout_seconds),
            "--kernel-args",
            "TERM=dumb",
            &product_bundle_path,
        ])
        .await
        .map_err(|e| IsolatedEmulatorError::SetupFailed {
            step: "emu start".to_string(),
            source: Box::new(e),
        })?;

        info!("streaming system logs to output directory");
        let mut system_logs_command = this
            .ffx_isolate
            .make_ffx_cmd(&this.make_args(&["log", "--severity", "TRACE", "--no-color"]))
            .map_err(IsolatedEmulatorError::IsolateCreate)?;

        let log_path_system = this.ffx_isolate.log_dir().join("system.log");
        let emulator_system_log = std::fs::File::create(&log_path_system).map_err(|e| {
            IsolatedEmulatorError::LogFileCreate {
                path: log_path_system.display().to_string(),
                source: e,
            }
        })?;
        system_logs_command.stdout(emulator_system_log);

        let log_path_err = this.ffx_isolate.log_dir().join("system_err.log");
        let emulator_stderr_log = std::fs::File::create(&log_path_err).map_err(|e| {
            IsolatedEmulatorError::LogFileCreate {
                path: log_path_err.display().to_string(),
                source: e,
            }
        })?;
        system_logs_command.stderr(emulator_stderr_log);

        this.children
            .lock()
            .unwrap()
            .push(system_logs_command.spawn().map_err(IsolatedEmulatorError::LogStreamSpawn)?);

        // serve packages by creating a repository and a server, then registering the server
        if let Err(e) = fuchsia_url::RepositoryUrl::parse(&format!(
            "fuchsia-pkg://{}",
            this.package_server_name
        )) {
            let details = if this.package_server_name.contains("_") {
                format!("underscores are not allowed: {e}")
            } else {
                e.to_string()
            };
            return Err(IsolatedEmulatorError::InvalidRepositoryName {
                name: this.package_server_name.clone(),
                details,
            });
        }
        this.ffx(&[
            "repository",
            "server",
            "start",
            "--background",
            "--no-device",
            // ask the kernel to give us a random unused port
            "--address",
            "[::]:0",
            "--repository",
            &this.package_server_name,
            "--repo-path",
            &package_repository_path,
        ])
        .await
        .map_err(|e| IsolatedEmulatorError::SetupFailed {
            step: "repository server start".to_string(),
            source: Box::new(e),
        })?;

        this.ffx(&[
            "target",
            "repository",
            "register",
            "--repository",
            &this.package_server_name,
            "--alias",
            "fuchsia.com",
        ])
        .await
        .map_err(|e| IsolatedEmulatorError::SetupFailed {
            step: "target repository register".to_string(),
            source: Box::new(e),
        })?;

        Ok(this)
    }

    pub fn env_context(&self) -> &EnvironmentContext {
        self.ffx_isolate.env_context()
    }

    /// Get the name of the emulator instance.
    pub fn emu_name(&self) -> &str {
        &self.emu_name
    }

    /// Acquire a Fuchsia Host Objects (FHO) environment for use with this
    /// isolated instance.
    pub fn fho_env(&self) -> fho::FhoEnvironment {
        fho::FhoEnvironment::new_with_args(self.env_context(), &self.make_args(&[])[..])
    }

    /// Sets up a fake direct connector for the emulator to avoid ambiguity in discovery.
    pub async fn setup_fake_direct_connector(
        &self,
        fho_env: &fho::FhoEnvironment,
    ) -> Result<(), IsolatedEmulatorError> {
        let query = TargetInfoQuery::NodenameOrId(self.emu_name().to_string());
        let targets =
            ffx_target::list_targets(fho_env.environment_context(), query, false, false, false)
                .await?;
        let target_info = targets.first().ok_or_else(|| IsolatedEmulatorError::NoTargetFound {
            emu_name: self.emu_name().to_string(),
        })?;

        let th = discovery::TargetHandle {
            node_name: target_info.nodename.clone(),
            state: discovery::TargetState::Product {
                addrs: target_info.addresses.clone().into_iter().map(|a| a.into()).collect(),
                serial: target_info.serial_number.clone(),
            },
            manual: false,
        };

        let resolution = Resolution::from_target_handle(th)?;
        let behavior = ConnectionBehavior::fake_direct_connector(resolution);

        target_interface(&fho_env).set_behavior_for_test(behavior);
        Ok(())
    }

    fn make_args<'a>(&'a self, args: &[&'a str]) -> Vec<&str> {
        let mut prefixed = vec!["--target", &self.emu_name];
        prefixed.extend(args);
        prefixed
    }

    /// Run an ffx command, logging stdout & stderr as INFO messages.
    pub async fn ffx(&self, args: &[&str]) -> Result<(), IsolatedEmulatorError> {
        let output = self
            .ffx_isolate
            .ffx(&self.make_args(args))
            .await
            .map_err(IsolatedEmulatorError::IsolateCreate)?;
        if !output.stdout.is_empty() {
            info!("stdout:\n{}", output.stdout);
        }
        if !output.stderr.is_empty() {
            info!("stderr:\n{}", output.stderr);
        }
        if !output.status.success() {
            return Err(IsolatedEmulatorError::FfxCommandFailed {
                args: args.iter().map(|s| s.to_string()).collect(),
                stderr: output.stderr,
            });
        }
        Ok(())
    }

    /// Like [`IsolatedEmulator::ffx`], but runs synchronously, blocking the
    /// current thread until the ffx command exits.
    pub fn ffx_sync(&self, args: &[&str]) -> Result<(), IsolatedEmulatorError> {
        let output = self
            .ffx_isolate
            .ffx_sync(&self.make_args(args))
            .map_err(IsolatedEmulatorError::IsolateCreate)?;
        if !output.stdout.is_empty() {
            info!("stdout:\n{}", output.stdout);
        }
        if !output.stderr.is_empty() {
            info!("stderr:\n{}", output.stderr);
        }
        if !output.status.success() {
            return Err(IsolatedEmulatorError::FfxCommandFailed {
                args: args.iter().map(|s| s.to_string()).collect(),
                stderr: output.stderr,
            });
        }
        Ok(())
    }

    /// Run an ffx command, returning stdout and logging stderr as an INFO message.
    pub async fn ffx_output(&self, args: &[&str]) -> Result<String, IsolatedEmulatorError> {
        let output = self
            .ffx_isolate
            .ffx(&self.make_args(args))
            .await
            .map_err(IsolatedEmulatorError::IsolateCreate)?;
        if !output.stderr.is_empty() {
            info!("stderr:\n{}", output.stderr);
        }
        if !output.status.success() {
            return Err(IsolatedEmulatorError::FfxCommandFailed {
                args: args.iter().map(|s| s.to_string()).collect(),
                stderr: output.stderr,
            });
        }
        Ok(output.stdout)
    }

    /// Run an ffx command with JSON machine output, returning T parsed from stdout and logging any
    /// stderr as an INFO message.
    pub async fn ffx_json<T: DeserializeOwned>(
        &self,
        args: &[&str],
    ) -> Result<T, IsolatedEmulatorError> {
        let mut all_args = vec!["--machine", "json"];
        all_args.extend(args);
        let output = self.ffx_output(&all_args).await?;
        Ok(serde_json::from_str(&output)?)
    }

    /// Create an ffx command, which allows for streaming stdout/stderr.
    pub async fn ffx_cmd_capture(&self, args: &[&str]) -> Result<Command, IsolatedEmulatorError> {
        let mut cmd = self
            .ffx_isolate
            .make_ffx_cmd(&self.make_args(args))
            .map_err(IsolatedEmulatorError::IsolateCreate)?;
        cmd.stdout(Stdio::piped());
        Ok(cmd)
    }

    fn make_ssh_args<'a>(command: &[&'a str]) -> Vec<&'a str> {
        let mut args = vec!["target", "ssh", "--"];
        args.extend(command);
        args
    }

    /// Run an ssh command, logging stdout & stderr as INFO messages.
    pub async fn ssh(&self, command: &[&str]) -> Result<(), IsolatedEmulatorError> {
        self.ffx(&Self::make_ssh_args(command)).await
    }

    /// Run an ssh command, returning stdout and logging stderr as an INFO message.
    pub async fn ssh_output(&self, command: &[&str]) -> Result<String, IsolatedEmulatorError> {
        self.ffx_output(&Self::make_ssh_args(command)).await
    }

    async fn log_stream(
        &self,
        mut receiver: futures::channel::mpsc::UnboundedReceiver<String>,
        reader_task: fuchsia_async::Task<Result<(), TrySendError<String>>>,
    ) -> impl Stream<Item = Result<LogsData, IsolatedEmulatorError>> {
        /// ffx log wraps each line from archivist in its own JSON object, unwrap those here
        #[derive(Deserialize)]
        struct FfxMachineLogLine {
            data: FfxTargetLog,
        }
        #[derive(Deserialize)]
        struct FfxTargetLog {
            #[serde(rename = "TargetLog")]
            target_log: LogsData,
        }

        stream! {
            while let Some(line) = receiver.next().await {
                if line.is_empty() {
                    continue;
                }
                let ffx_message = serde_json::from_str::<FfxMachineLogLine>(&line)
                    .map_err(IsolatedEmulatorError::JsonParse)?;
                yield Ok(ffx_message.data.target_log);
            }
            drop(reader_task)
        }
    }

    /// Collect the logs for a particular component.
    pub async fn log_stream_for_moniker(
        &self,
        moniker: &str,
    ) -> Result<impl Stream<Item = Result<LogsData, IsolatedEmulatorError>>, IsolatedEmulatorError>
    {
        let mut output =
            self.ffx_cmd_capture(&["--machine", "json", "log", "--moniker", moniker]).await?;

        let mut child = output.spawn().map_err(IsolatedEmulatorError::CommandSpawn)?;
        let stdout = child.stdout.take().ok_or(IsolatedEmulatorError::NoStdout)?;
        self.children.lock().unwrap().push(child);
        let mut reader = BufReader::new(stdout);
        let (sender, receiver) = futures::channel::mpsc::unbounded();
        let reader_task = fuchsia_async::Task::local(fuchsia_async::unblock(move || {
            let mut output = String::new();
            while let Ok(_) = reader.read_line(&mut output) {
                sender.unbounded_send(output)?;
                output = String::new();
            }
            Result::<(), TrySendError<String>>::Ok(())
        }));
        Ok(self.log_stream(receiver, reader_task).await)
    }

    /// Collect the logs for a particular component.
    pub async fn logs_for_moniker(
        &self,
        moniker: &str,
    ) -> Result<Vec<LogsData>, IsolatedEmulatorError> {
        /// ffx log wraps each line from archivist in its own JSON object, unwrap those here
        #[derive(Deserialize)]
        struct FfxMachineLogLine {
            data: FfxTargetLog,
        }
        #[derive(Deserialize)]
        struct FfxTargetLog {
            #[serde(rename = "TargetLog")]
            target_log: LogsData,
        }

        let output =
            self.ffx_output(&["--machine", "json", "log", "--moniker", moniker, "dump"]).await?;

        let mut parsed = vec![];
        for line in output.lines() {
            if line.is_empty() {
                continue;
            }
            let ffx_message = serde_json::from_str::<FfxMachineLogLine>(line)?;
            parsed.push(ffx_message.data.target_log);
        }
        Ok(parsed)
    }

    pub fn stop_sync(&self) {
        self.ffx_sync(&["repository", "server", "stop", &self.package_server_name]).unwrap_or_else(
            |e| {
                log::warn!("failed to stop repository server: {e:?}");
            },
        );
        self.ffx_sync(&["emu", "stop", &self.emu_name]).unwrap_or_else(|e| {
            log::warn!("failed to stop emulator: {e:?}");
        });
    }
}

impl Drop for IsolatedEmulator {
    fn drop(&mut self) {
        if !self.children.lock().unwrap().is_empty() {
            // allow children to clean up, including streaming a few logs out
            std::thread::sleep(std::time::Duration::from_secs(1));
            let mut children = self.children.lock().unwrap();
            for child in children.iter_mut() {
                child.kill().ok();
            }
        }

        self.stop_sync();

        info!(
            "Tearing down isolated emulator instance. Logs are in {}.",
            self.ffx_isolate.log_dir().display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::pin;

    #[fuchsia::test]
    async fn public_apis_succeed() {
        let amber_files_path = std::env::var("PACKAGE_REPOSITORY_PATH")
            .expect("PACKAGE_REPOSITORY_PATH env var must be set -- run this test with 'fx test'");
        let emu = IsolatedEmulator::start_internal(
            "e2e-emu-public-apis",
            Some(&amber_files_path),
            None,
            IsolatedEmulator::DEFAULT_STARTUP_TIMEOUT_SECONDS,
            true,
        )
        .await
        .expect("Couldn't start emulator");

        info!("Checking target monotonic time to ensure we can connect and get stdout");
        let time = emu.ffx_output(&["target", "get-time"]).await.unwrap();
        time.trim().parse::<u64>().expect("should have gotten a timestamp back");

        info!("Checking that the emulator instance writes a system log.");
        let system_log_path = emu.ffx_isolate.log_dir().join("system.log");
        loop {
            let contents = std::fs::read_to_string(&system_log_path).unwrap();
            if !contents.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }

        info!("Checking that we can read streaming logs.");
        let mut remote_control_logs =
            pin!(emu.log_stream_for_moniker("/core/remote-control").await.unwrap());
        remote_control_logs.next().await.unwrap().unwrap();

        info!("Checking that we can read RCS' logs.");
        let remote_control_logs = emu.logs_for_moniker("/core/remote-control").await.unwrap();
        assert_eq!(remote_control_logs.is_empty(), false);
    }

    #[fuchsia::test]
    async fn resolve_package_from_server() {
        let amber_files_path = std::env::var("PACKAGE_REPOSITORY_PATH").expect(
            "TEST_PACKAGE_REPOSITORY_PATH env var must be set -- run this test with 'fx test'",
        );
        let test_package_name = std::env::var("TEST_PACKAGE_NAME")
            .expect("TEST_PACKAGE_NAME env var must be set -- run this test with 'fx test'");
        let test_package_url = format!("fuchsia-pkg://fuchsia.com/{test_package_name}");
        let emu = IsolatedEmulator::start_internal(
            "pkg-resolve",
            Some(&amber_files_path),
            None,
            IsolatedEmulator::DEFAULT_STARTUP_TIMEOUT_SECONDS,
            true,
        )
        .await
        .unwrap();
        emu.ssh(&["pkgctl", "resolve", &test_package_url]).await.unwrap();
    }
}
