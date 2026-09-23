// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use fidl::endpoints::Proxy;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_netemul_guest as fnetemul_guest;
use fidl_fuchsia_virtualization_guest_interaction as fguest_interaction;
use futures_util::io::AsyncReadExt as _;

/// A controller for managing a single virtualized guest.
///
/// `Controller` instantiates a guest on creation and exposes
/// methods for communicating with the guest. The guest lifetime
/// is tied to the controller's; dropping the controller will shutdown
/// the guest.
pub struct Controller {
    // Option lets us simplify the implementation of `Drop` by taking
    // the GuestProxy and converting to a SynchronousGuestProxy.
    guest: Option<fnetemul_guest::GuestProxy>,
    name: String,
}

impl<'a> std::fmt::Debug for Controller {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::result::Result<(), std::fmt::Error> {
        let Self { guest: _, name } = self;
        f.debug_struct("Controller").field("name", name).finish_non_exhaustive()
    }
}

impl Controller {
    /// Instantiates a guest and installs it on the provided `network`. If `mac` is provided,
    /// the guest will be given the mac address; otherwise one will be picked by virtio.
    /// Returns an error if the sandbox already contains a guest.
    pub async fn new(
        name: impl Into<String>,
        network: &TestNetwork<'_>,
        mac: Option<fnet::MacAddress>,
    ) -> Result<Controller> {
        let name = name.into();
        let controller_proxy =
            fuchsia_component::client::connect_to_protocol::<fnetemul_guest::ControllerMarker>()
                .with_context(|| {
                    format!("failed to connect to guest controller protocol for guest {}", name)
                })?;

        let network_client =
            network.get_client_end_clone().await.context("failed to get network client end")?;
        let guest = controller_proxy
            .create_guest(&name, network_client, mac.as_ref())
            .await
            .with_context(|| format!("create_guest FIDL error for guest {}", name))?
            .map_err(|err| {
                anyhow::anyhow!(format!("create guest error for guest {}: {:?}", name, err))
            })?;
        Ok(Controller { guest: Some(guest.into_proxy()), name })
    }

    fn proxy(&self) -> &fnetemul_guest::GuestProxy {
        self.guest.as_ref().expect("guest_proxy was empty")
    }

    /// Copies the file located at `local_path` within the namespace of the executing process
    /// to `remote_path` on the guest.
    pub async fn put_file(&self, local_path: &str, remote_path: &str) -> Result {
        let (file_client_end, file_server_end) =
            fidl::endpoints::create_endpoints::<fio::FileMarker>();
        fdio::open(&local_path, fio::PERM_READABLE, file_server_end.into_channel())
            .with_context(|| format!("failed to open file '{}'", local_path))?;
        let status = self
            .proxy()
            .put_file(file_client_end, remote_path)
            .await
            .with_context(|| format!("put_file FIDL error for guest {}", self.name))?;
        zx::Status::ok(status).with_context(|| {
            format!(
                "put_file for guest {} failed for file at local path {} and remote path {}",
                self.name, local_path, remote_path
            )
        })
    }

    /// Copies the file located at `remote_path` on the guest to `local_path` within the
    /// namespace of the current process.
    pub async fn get_file(&self, local_path: &str, remote_path: &str) -> Result {
        let (file_client_end, file_server_end) =
            fidl::endpoints::create_endpoints::<fio::FileMarker>();
        fdio::open(
            &local_path,
            fio::PERM_WRITABLE | fio::Flags::FLAG_MAYBE_CREATE,
            file_server_end.into_channel(),
        )
        .with_context(|| format!("failed to open file '{}'", local_path))?;
        let status = self
            .proxy()
            .get_file(remote_path, file_client_end)
            .await
            .with_context(|| format!("get_file FIDL error for guest {}", self.name))?;
        zx::Status::ok(status).with_context(|| {
            format!(
                "get_file for guest {} failed for file at local path {} and remote path {}",
                self.name, local_path, remote_path
            )
        })
    }

    /// Executes `command` on the guest with environment variables held in
    /// `env`, writing `input` into the remote process's `stdin` and logs
    /// the remote process's stdout and stderr.
    ///
    /// Returns an error if the executed command's exit code is non-zero.
    pub async fn exec_with_output_logged(
        &self,
        command: &str,
        env: Vec<fguest_interaction::EnvironmentVariable>,
        input: Option<&str>,
    ) -> Result<()> {
        let (return_code, stdout, stderr) = self.exec(command, env, input).await?;
        log::info!(
            "command `{}` for guest {} output\nstdout: {}\nstderr: {}",
            command,
            self.name,
            stdout,
            stderr
        );
        if return_code != 0 {
            return Err(anyhow!(
                "command `{}` for guest {} failed with return code: {}",
                command,
                self.name,
                return_code,
            ));
        }
        Ok(())
    }

    /// Executes `command` on the guest with environment variables held in `env`, writing
    /// `input` into the remote process's `stdin` and returning the remote process's
    /// (stdout, stderr).
    pub async fn exec(
        &self,
        command: &str,
        env: Vec<fguest_interaction::EnvironmentVariable>,
        input: Option<&str>,
    ) -> Result<(i32, String, String)> {
        let (stdout_local, stdout_remote) = zx::Socket::create_stream();
        let (stderr_local, stderr_remote) = zx::Socket::create_stream();

        let (command_listener_client, command_listener_server) =
            fidl::endpoints::create_proxy::<fguest_interaction::CommandListenerMarker>();
        let (stdin_local, stdin_remote) = match input {
            Some(input) => {
                let (stdin_local, stdin_remote) = zx::Socket::create_stream();
                (Some((stdin_local, input)), Some(stdin_remote))
            }
            None => (None, None),
        };
        self.proxy()
            .execute_command(
                command,
                &env,
                stdin_remote,
                Some(stdout_remote),
                Some(stderr_remote),
                command_listener_server,
            )
            .with_context(|| format!("execute_command FIDL error for guest {}", self.name))?;

        let mut async_stdout = fuchsia_async::Socket::from_socket(stdout_local);
        let mut async_stderr = fuchsia_async::Socket::from_socket(stderr_local);

        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        let stdout_fut = pin!(
            async_stdout
                .read_to_end(&mut stdout_buf)
                .map(|res| res.context("failed to read from stdout"))
                .fuse()
        );
        let stderr_fut = pin!(
            async {
                async_stderr
                    .read_to_end(&mut stderr_buf)
                    .await
                    .context("failed to read from socket")
            }
            .fuse()
        );

        let mut command_listener_stream = command_listener_client.take_event_stream();
        let listener_fut = pin!(
            async {
                loop {
                    let event = command_listener_stream
                        .try_next()
                        .await
                        .with_context(|| {
                            format!(
                                "failed to get next CommandListenerEvent for guest {}",
                                self.name
                            )
                        })?
                        .with_context(|| {
                            format!("empty CommandListenerEvent for guest {}", self.name)
                        })?;
                    match event {
                        fguest_interaction::CommandListenerEvent::OnStarted { status } => {
                            zx::Status::ok(status).with_context(|| {
                                format!(
                                    "error starting exec for guest {} and command {}",
                                    self.name, command
                                )
                            })?;

                            if let Some((stdin_local, to_write)) = stdin_local.as_ref() {
                                assert_eq!(
                                    stdin_local.write(to_write.as_bytes())?,
                                    to_write.as_bytes().len()
                                );
                            }
                        }
                        fguest_interaction::CommandListenerEvent::OnTerminated {
                            status,
                            return_code,
                        } => {
                            zx::Status::ok(status).with_context(|| {
                                format!(
                                    "error returning from exec for guest {} and command {}",
                                    self.name, command
                                )
                            })?;

                            return Ok(return_code);
                        }
                    }
                }
            }
            .fuse()
        );

        // Scope required to limit the lifetime of pinned futures.
        let return_code = {
            // Poll the stdout and stderr sockets in parallel while waiting for the remote
            // process to terminate. This avoids deadlock in case the remote process blocks
            // on writing to stdout/stderr.
            let (_, return_code, _): (usize, _, usize) =
                futures::try_join!(stderr_fut, listener_fut, stdout_fut)?;
            return_code
        };

        let stdout = String::from_utf8(stdout_buf).context("failed to convert stdout to string")?;
        let stderr = String::from_utf8(stderr_buf).context("failed to convert stderr to string")?;

        Ok((return_code, stdout, stderr))
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let guest = fnetemul_guest::GuestSynchronousProxy::new(
            self.guest
                .take()
                .expect("guest proxy was empty")
                .into_channel()
                .expect("failed to convert to FIDL channel")
                .into_zx_channel(),
        );

        guest.shutdown(zx::MonotonicInstant::INFINITE).expect("shutdown FIDL error");
    }
}
