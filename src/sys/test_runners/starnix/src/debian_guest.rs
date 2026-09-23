// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow, bail};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_virtualization::{BlockSpec, GuestConfig};
use fidl_fuchsia_virtualization_guest_interaction::{
    CommandListenerEvent, CommandListenerMarker, EnvironmentVariable, GuestType,
    InteractiveGuestMarker, InteractiveGuestProxy,
};
use fuchsia_async::{DurationExt, TimeoutExt};
use fuchsia_component::client::connect_to_protocol;
use futures::TryStreamExt;
use std::cell::OnceCell;
use std::path::Path;
use std::sync::Mutex;

const EXECUTE_TIMEOUT_SECONDS: i64 = 180;

#[derive(Debug)]
pub struct DataMount {
    /// The block spec for this data mount.
    pub block_spec: BlockSpec,
    /// An executable shell script for mounting this data mount.
    pub mount_script_file: ClientEnd<fio::FileMarker>,
}

impl DataMount {
    pub fn new(block_spec: BlockSpec, mount_script_file: ClientEnd<fio::FileMarker>) -> Self {
        Self { block_spec, mount_script_file }
    }
}

pub struct DebianGuest {
    instance_name: String,
    /// The proxy for interacting with the guest. This should be accessed by the `interactive_guest`
    /// helper function, to aid with locking and ensuring that the guest is ready for interaction.
    guest_proxy: OnceCell<Mutex<InteractiveGuestProxy>>,
    data_mount: Mutex<Option<DataMount>>,
}

impl DebianGuest {
    /// Creates a new instance of the DebianGuest. The actual bootstrapping of a guest is done
    /// lazily. This is because the lifecycle of the DebianGuest needs to live through the Starnix
    /// test runner framework, but not all tests will actually need the guest. Thus, we construct
    /// the DebianGuest while refraining from bootstrap until the guest is first interacted with.
    ///
    /// # Arguments
    /// * `instance_name` - An instance name, which serves as the tag for log output.
    pub fn new(instance_name: String) -> DebianGuest {
        DebianGuest { instance_name, guest_proxy: OnceCell::new(), data_mount: Mutex::new(None) }
    }

    /// Configures an optional data mount to attach to the guest upon initialization. Note that if the
    /// DebianGuest is already running, configuring this has no effect unless the guest is rebooted.
    /// Upon first initiatization, the provided block will be mounted and the executable script run.
    pub fn configure_data_mount(&self, data_mount: DataMount) {
        *self.data_mount.lock().unwrap() = Some(data_mount);
    }

    /// Gets a handle to the proxy, while also lazily bootstrapping the guest if necessary.
    async fn interactive_guest(&self) -> InteractiveGuestProxy {
        match self.guest_proxy.get() {
            Some(proxy_mutex) => proxy_mutex.lock().unwrap().clone(),
            None => {
                log::info!(tag = self.instance_name.as_str();
                    "Interaction requested, lazily starting the guest instance."
                );

                let (mount_block, mount_script) = self
                    .data_mount
                    .lock()
                    .unwrap()
                    .take()
                    .map(|data_mount| (vec![data_mount.block_spec], data_mount.mount_script_file))
                    .unzip();

                let mut cfg = GuestConfig::default();
                cfg.virtio_gpu = Some(false);
                cfg.virtio_sound = Some(false);
                cfg.virtio_sound_input = Some(false);
                cfg.virtio_rng = Some(false);
                cfg.virtio_balloon = Some(false);
                cfg.virtio_mem = Some(false);
                cfg.default_net = Some(false);
                cfg.block_devices = mount_block;

                let guest_proxy = connect_to_protocol::<InteractiveGuestMarker>()
                    .expect("Error connecting to InteractiveGuest");
                guest_proxy
                    .start(GuestType::Debian, &self.instance_name, cfg)
                    .await
                    .expect("Debian guest failed to start!");
                log::info!(tag = self.instance_name.as_str(); "Guest instance started successfully.");

                let return_proxy = guest_proxy.clone();
                let proxy_mutex = Mutex::new(guest_proxy);
                self.guest_proxy
                    .set(proxy_mutex)
                    .expect("Unexpected race condition while bootstrapping the guest proxy.");

                // The initialization of the mount requires interacting with the guest_proxy, and
                // will recursively call interactive_guest() to obtain said proxy. Therefore, the
                // mounting needs to be done only after the previous initialization logic finishes.
                if let Some(mount_script) = mount_script {
                    Box::pin(self.initialize_mount(mount_script))
                        .await
                        .expect("Failed to initialize guest data mount!");
                }

                return_proxy
            }
        }
    }

    async fn initialize_mount(
        &self,
        mount_script: ClientEnd<fio::FileMarker>,
    ) -> Result<(), Error> {
        const GUEST_MOUNT_SCRIPT_PATH: &str = "/tmp/mount_syscall_deps.sh";

        self.push_data_to_guest(mount_script, Path::new(GUEST_MOUNT_SCRIPT_PATH)).await?;
        self.execute(&format!("/bin/sh {}", GUEST_MOUNT_SCRIPT_PATH), &[], None, None, None)
            .await?;

        log::info!(tag = self.instance_name.as_str(); "Mount initializated successfully.");
        Ok(())
    }

    /// Pushes data from `source` to the guest at `destination`.
    ///
    /// # Arguments
    /// * `source` - The source file to copy from.
    /// * `destination` - The destination path in the guest's filesystem.
    pub async fn push_data_to_guest(
        &self,
        source: ClientEnd<fio::FileMarker>,
        destination: &Path,
    ) -> Result<(), Error> {
        log::info!(tag = self.instance_name.as_str(); "Pushing data to guest (destination: {})", destination.display());

        let dest_str = destination
            .to_str()
            .ok_or_else(|| anyhow!("Destination path is not valid UTF-8: {:?}", destination))?;
        let guest_proxy = self.interactive_guest().await;
        let response = guest_proxy
            .put_file(source, dest_str)
            .await
            .context("FIDL call to InteractiveGuest::PutFile has failed.")?;

        if let Err(status) = zx::Status::ok(response) {
            bail!("PutFile operation failed with status: {:?}", status);
        }

        log::info!(tag = self.instance_name.as_str();
            "Successfully pushed data to guest (destination: {})",
            destination.display()
        );

        Ok(())
    }

    /// Fetches a file from the guest.
    ///
    /// # Arguments
    /// * `remote_path` - The path to the file in the guest's filesystem.
    /// * `local_file_proxy` - The local file proxy to write the contents to.
    pub async fn get_file(
        &self,
        remote_path: &Path,
        local_file_proxy: ClientEnd<fio::FileMarker>,
    ) -> Result<(), Error> {
        log::info!(tag = self.instance_name.as_str(); "Fetching file from guest (remote_path: {})", remote_path.display());
        let remote_path_str = remote_path
            .to_str()
            .ok_or_else(|| anyhow!("Remote path is not valid UTF-8: {:?}", remote_path))?;
        let guest_proxy = self.interactive_guest().await;

        let response = guest_proxy
            .get_file(remote_path_str, local_file_proxy)
            .await
            .context("FIDL call to GetFile failed")?;

        if let Err(status) = zx::Status::ok(response) {
            bail!("GetFile operation failed with status: {:?}", status);
        }
        Ok(())
    }

    /// Executes a command on the guest, returning the command's exit code upon successful execution
    /// and an Error if the command was unable to be executed on the guest.
    ///
    /// # Arguments
    /// * `command`: The command string to execute (e.g., "/bin/ls -l /tmp").
    /// * `env_vars`: Environment vars to set for the execution context.
    /// * `stdin`: An optional `zx::Socket` for providing standard input to the command.
    /// * `stdout`: An optional client end for receiving stdout from the command.
    /// * `stderr`: An optional client end for receiving stderr from the command.
    pub async fn execute(
        &self,
        command: &str,
        env_vars: &[EnvironmentVariable],
        stdin: Option<zx::Socket>,
        stdout: Option<zx::Socket>,
        stderr: Option<zx::Socket>,
    ) -> Result<i32, Error> {
        log::info!(tag = self.instance_name.as_str(); "Executing command on guest: {}", command);
        let guest_proxy = self.interactive_guest().await;
        let (command_listener_client, command_listener_server) =
            fidl::endpoints::create_proxy::<CommandListenerMarker>();

        guest_proxy
            .execute_command(command, env_vars, stdin, stdout, stderr, command_listener_server)
            .context("FIDL call to ExecuteCommand failed")?;

        let mut event_stream = command_listener_client.take_event_stream();

        let execution_future = async move {
            while let Some(event) = event_stream.try_next().await? {
                match event {
                    CommandListenerEvent::OnStarted { status } => match zx::Status::ok(status) {
                        Ok(()) => {
                            log::info!(tag = self.instance_name.as_str(); "Command '{}'\n...started successfully", command)
                        }
                        Err(status) => {
                            bail!("Command '{}'\n...failed to start: {:?}", command, status)
                        }
                    },
                    CommandListenerEvent::OnTerminated { status, return_code } => {
                        let term_status = zx::Status::err_from_raw(status);
                        log::info!(tag = self.instance_name.as_str();
                            "Command '{}'\n...terminated with status {:?}, return code {}",
                            command,
                            term_status,
                            return_code
                        );
                        if let Err(status) = zx::Status::ok(status) {
                            bail!("Command '{}' failed with status: {:?}", command, status);
                        }
                        return Ok(return_code);
                    }
                }
            }

            panic!("Execution result stream closed before OnTerminated event!");
        };

        let timeout_duration = zx::MonotonicDuration::from_seconds(EXECUTE_TIMEOUT_SECONDS);
        execution_future
            .on_timeout(timeout_duration.after_now(), || {
                Err(anyhow!(
                    "Command execution '{}'\n...timed out after {} seconds",
                    command,
                    EXECUTE_TIMEOUT_SECONDS
                ))
            })
            .await
    }

    /// Shuts down the guest.
    pub async fn shutdown(&self) -> Result<(), Error> {
        match self.guest_proxy.get() {
            Some(proxy) => {
                log::info!(tag = self.instance_name.as_str(); "Shutting down guest instance.");
                let proxy_clone = proxy.lock().unwrap().clone();
                proxy_clone.shutdown().await.context("FIDL call to Shutdown failed")
            }
            None => {
                log::info!(tag = self.instance_name.as_str(); "Guest was never bootstrapped, shutdown is unnecessary.");
                Ok(())
            }
        }
    }
}
