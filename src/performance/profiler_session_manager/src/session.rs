// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_cpu_profiler::{Config, SessionMarker, SessionProxy};
use fuchsia_component::client::connect_to_protocol;
use log::{error, info};
use std::fs::File;
use std::io::Write;
use zx;

#[derive(Debug)]
pub enum SessionError {
    Io(std::io::Error),
    Fidl(fidl::Error),
    Config(String),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Io(e) => write!(f, "IO error: {}", e),
            SessionError::Fidl(e) => write!(f, "FIDL error: {}", e),
            SessionError::Config(msg) => write!(f, "Configuration error: {}", msg),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<std::io::Error> for SessionError {
    fn from(err: std::io::Error) -> Self {
        SessionError::Io(err)
    }
}

impl From<fidl::Error> for SessionError {
    fn from(err: fidl::Error) -> Self {
        SessionError::Fidl(err)
    }
}

pub type Result<T> = std::result::Result<T, SessionError>;

pub struct BackgroundSession {
    pub task_id: u64,
    proxy: SessionProxy,
    drain_task: Option<fuchsia_async::Task<()>>,
}

impl BackgroundSession {
    pub async fn start(task_id: u64, config: Config) -> Result<Self> {
        info!("Connecting to fuchsia.cpu.profiler.Session for task {}", task_id);
        let proxy = connect_to_protocol::<SessionMarker>()
            .map_err(|e| SessionError::Config(format!("Connect fail: {:?}", e)))?;

        let (rx, tx) = zx::Socket::create_stream();

        let req = fidl_fuchsia_cpu_profiler::SessionConfigureRequest {
            output: Some(tx),
            config: Some(config),
            ..Default::default()
        };
        proxy
            .configure(req)
            .await?
            .map_err(|e| SessionError::Config(format!("Configuration failed: {:?}", e)))?;

        info!("Starting Session proxy for task {}", task_id);

        let start_req = fidl_fuchsia_cpu_profiler::SessionStartRequest {
            buffer_results: Some(true),
            ..Default::default()
        };
        proxy
            .start(&start_req)
            .await?
            .map_err(|e| SessionError::Config(format!("Start failed: {:?}", e)))?;

        let task_id_clone = task_id;
        let drain_task = fuchsia_async::Task::spawn(async move {
            if let Err(e) = Self::drain_socket_to_file(task_id_clone, rx).await {
                error!("Task {} failed to drain socket: {:?}", task_id_clone, e);
            }
        });

        Ok(Self { task_id, proxy, drain_task: Some(drain_task) })
    }

    pub async fn stop_and_stream(
        &mut self,
        output: zx::Socket,
    ) -> Result<fidl_fuchsia_cpu_profiler::SessionResult> {
        info!("Stopping Session proxy for task {}", self.task_id);

        // Let the profiler fully stop dumping data to our Read socket.
        let session_result = self
            .proxy
            .stop()
            .await
            .map_err(|e| SessionError::Config(format!("Stop failed: {:?}", e)))?;

        let path = format!("/profiles/session_{}.fxt", self.task_id);

        // Wait for the file drainer task to sync and close.
        if let Some(task) = self.drain_task.take() {
            task.await;
        }

        // To avoid bloating the size of this component, we don't introduce fuchsia_fs and
        // fuchsia_io here. Instead we just use the standard library to read the file on a separate
        // thread, and write directly to the zx::Socket.
        match std::fs::File::open(&path) {
            Ok(file) => {
                info!("Streaming cached profile {path} to output socket");
                let output = output; // transfer ownership to the thread

                std::thread::spawn(move || {
                    use std::io::Read;
                    let mut file = file;
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        match file.read(&mut buf[..]) {
                            Ok(0) => break, // EOF
                            Ok(n) => {
                                let mut written = 0;
                                while written < n {
                                    match output.write(&buf[written..n]) {
                                        Ok(bytes_written) => {
                                            written += bytes_written;
                                        }
                                        Err(zx::Status::SHOULD_WAIT) => {
                                            // The socket is full, wait for it to become writable
                                            match output.as_handle_ref().wait_one(
                                                zx::Signals::SOCKET_WRITABLE
                                                    | zx::Signals::SOCKET_PEER_CLOSED,
                                                zx::MonotonicInstant::INFINITE,
                                            ) {
                                                zx::WaitResult::Ok(_) => {}
                                                zx::WaitResult::Err(e) => {
                                                    error!("Failed waiting for socket: {:?}", e);
                                                    return; // thread abort
                                                }
                                                zx::WaitResult::TimedOut(_)
                                                | zx::WaitResult::Canceled(_) => {
                                                    error!("Socket wait timed out or canceled");
                                                    return; // thread abort
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error!("Failed writing to socket: {:?}", e);
                                            return; // thread abort
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                error!("Failed to read cached profile: {:?}", e);
                                break;
                            }
                        }
                    }
                });

                // Cleanup temp file
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => {
                error!("Could not open cached profile for task {}: {:?}", self.task_id, e);
            }
        }
        Ok(session_result)
    }

    #[cfg(test)]
    pub fn new_for_test(task_id: u64, proxy: SessionProxy) -> Self {
        Self { task_id, proxy, drain_task: None }
    }

    pub async fn abort(&mut self) -> Result<()> {
        let _ = self
            .proxy
            .reset()
            .await
            .map_err(|e| SessionError::Config(format!("Reset failed: {:?}", e)))?;
        // Dropping the task cancels the execution
        self.drain_task.take();
        // Cleanup temp file
        let path = format!("/profiles/session_{}.fxt", self.task_id);
        let _ = std::fs::remove_file(&path);
        Ok(())
    }

    async fn drain_socket_to_file(task_id: u64, socket: zx::Socket) -> Result<()> {
        let path = format!("/profiles/session_{}.fxt", task_id);
        info!("Buffering profile data for task {} to {}", task_id, path);

        let mut file = File::create(&path)?;
        let mut async_socket = fuchsia_async::Socket::from_socket(socket);

        let mut buffer = [0u8; 4096];
        loop {
            match futures::AsyncReadExt::read(&mut async_socket, &mut buffer).await {
                Ok(0) => {
                    info!("Socket closed for task {}", task_id);
                    break;
                }
                Ok(n) => {
                    file.write_all(&buffer[..n])?;
                }
                Err(e) => {
                    error!("Read error from socket for task {}: {:?}", task_id, e);
                    break;
                }
            }
        }

        file.sync_all()?;
        Ok(())
    }
}
