// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fastboot_interface::{
    Fastboot, FastbootError, FastbootInterface, FlashError, RebootEvent, StageError,
    UploadProgress, Variable,
};
use crate::interface_factory::InterfaceFactory;
use crate::stream::{StreamCommand, StreamOp};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Duration;
use fastboot::UploadError::*;
use fastboot::command::{ClientVariable, Command};
use fastboot::reply::Reply;
use fastboot::{
    BUFFER_SIZE, FastbootContext, UploadProgressListener, download, read_and_log_info_with_timeout,
    send, send_with_listener, send_with_timeout, upload, upload_from_reader,
    upload_with_read_timeout,
};
use futures::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use std::fmt::Debug;
use std::fs::File;
use tokio::sync::mpsc::Sender;

///////////////////////////////////////////////////////////////////////////////
// FastbootProxy
//

#[derive(Debug)]
pub struct FastbootProxy<T: AsyncRead + AsyncWrite + Unpin + Send> {
    #[allow(dead_code)]
    target_id: String,
    interface: Option<T>,
    interface_factory: Box<dyn InterfaceFactory<T> + Send>,
    ctx: FastbootContext,
}

fn handle_timeout_as_okay(
    r: Result<Reply, fastboot::FastbootError>,
) -> Result<Reply, fastboot::FastbootError> {
    match r {
        Err(fastboot::FastbootError::Read(fastboot::ReadError::Timeout)) => {
            log::debug!("Timed out waiting for bootloader response; assuming it's okay");
            Ok(Reply::Okay("".to_string()))
        }
        _ => r,
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin + Debug + Send> FastbootInterface for FastbootProxy<T> {}

#[derive(Debug)]
struct VariableListener(Sender<Variable>);

impl VariableListener {
    fn new(listener: Sender<Variable>) -> Self {
        Self(listener)
    }
}

#[async_trait]
impl fastboot::InfoListener for VariableListener {
    async fn on_info(&self, info: String) -> Result<(), fastboot::ReadError> {
        if let Some((name, val)) = info.split_once(':') {
            log::debug!("Got a variable string: {}", info);
            self.0
                .send(Variable { name: name.to_string(), value: val.to_string() })
                .await
                .map_err(|e| {
                    fastboot::ReadError::Io(std::io::Error::new(std::io::ErrorKind::Other, e))
                })?;
        } else {
            log::warn!("Expected to get a variable string. Got: {}", info);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ProgressListener<'a>(&'a Sender<UploadProgress>);

impl<'a> ProgressListener<'a> {
    fn new(listener: &'a Sender<UploadProgress>) -> Self {
        Self(listener)
    }
}

#[async_trait]
impl fastboot::UploadProgressListener for ProgressListener<'_> {
    async fn on_started(&self, size: usize) -> Result<(), fastboot::UploadError> {
        let size = size.try_into().map_err(|e| {
            fastboot::UploadError::CouldNotVerifyUpload(fastboot::ReadError::Io(
                std::io::Error::new(std::io::ErrorKind::InvalidData, e),
            ))
        })?;
        self.0.send(UploadProgress::OnStarted { size }).await.map_err(|e| {
            fastboot::UploadError::CouldNotWriteToInterface(std::io::Error::new(
                std::io::ErrorKind::Other,
                e,
            ))
        })?;
        Ok(())
    }
    async fn on_progress(&self, bytes_written: u64) -> Result<(), fastboot::UploadError> {
        self.0.send(UploadProgress::OnProgress { bytes_written }).await.map_err(|e| {
            fastboot::UploadError::CouldNotWriteToInterface(std::io::Error::new(
                std::io::ErrorKind::Other,
                e,
            ))
        })?;
        Ok(())
    }
    async fn on_error(&self, error: &fastboot::UploadError) -> Result<(), fastboot::UploadError> {
        self.0.send(UploadProgress::OnError { error: anyhow!(error.to_string()) }).await.map_err(
            |e| {
                fastboot::UploadError::CouldNotWriteToInterface(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e,
                ))
            },
        )?;
        Ok(())
    }
    async fn on_finished(&self) -> Result<(), fastboot::UploadError> {
        self.0.send(UploadProgress::OnFinished).await.map_err(|e| {
            fastboot::UploadError::CouldNotWriteToInterface(std::io::Error::new(
                std::io::ErrorKind::Other,
                e,
            ))
        })?;
        Ok(())
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin + Debug + Send> FastbootProxy<T> {
    pub fn new(
        target_id: String,
        interface: T,
        interface_factory: impl InterfaceFactory<T> + Send + 'static,
    ) -> Self {
        Self {
            target_id,
            interface: Some(interface),
            interface_factory: Box::new(interface_factory),
            ctx: FastbootContext::new(),
        }
    }

    async fn reconnect(&mut self) -> Result<(), crate::interface_factory::InterfaceFactoryError> {
        // Explicitly here.
        self.interface = None;

        // Wait for it to show up again
        log::debug!("About to rediscover target");
        self.interface_factory.rediscover().await?;

        //Reconnect
        self.interface.replace(self.interface_factory.open().await?);
        log::warn!("Reconnected");
        Ok(())
    }

    async fn interface(
        &mut self,
    ) -> Result<&mut T, crate::interface_factory::InterfaceFactoryError> {
        if self.interface.is_none() {
            self.interface.replace(self.interface_factory.open().await?);
        }
        Ok(self.interface.as_mut().expect("interface interface not available"))
    }

    async fn flash_uploaded(
        &mut self,
        partition_name: &str,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        // Flash the uploaded file
        let command = Command::Flash(partition_name.to_string());
        let send_reply =
            send_with_timeout(self.ctx.clone(), command.clone(), self.interface().await?, timeout)
                .await;
        match send_reply {
            Ok(reply) => match reply {
                Reply::Okay(_) => Ok(()),
                Reply::Fail(s) => Err(FastbootError::FlashError(FlashError::FlashFailed {
                    partition: partition_name.to_string(),
                    message: s,
                })),
                r @ _ => Err(FastbootError::UnexpectedReply {
                    method: command.to_string(),
                    reply: r.to_string(),
                }),
            },
            Err(ref e) => {
                let e: &fastboot::FastbootError = e;
                if let fastboot::FastbootError::Read(fastboot::ReadError::Timeout) = e {
                    let message = format!(
                        "Time out while waiting on a response from the device. \n\
                            The current timeout is {}.  Try increacing the timeout",
                        timeout
                    );
                    Err(FastbootError::FlashError(FlashError::TimeoutError(message)))
                } else {
                    Err(FastbootError::FlashError(FlashError::Error(anyhow::anyhow!(
                        e.to_string()
                    ))))
                }
            }
        }
    }
}

async fn handle_command<T: AsyncRead + AsyncWrite + Unpin>(
    ctx: &FastbootContext,
    cmd: &Command,
    interface: &mut T,
    timeout: Duration,
) -> Result<(), FastbootError> {
    match send_with_timeout(ctx.clone(), cmd.clone(), interface, timeout).await? {
        Reply::Okay(_) => Ok(()),
        Reply::Fail(message) => {
            Err(FlashError::StreamFailed { command: cmd.clone(), message }.into())
        }
        r => Err(FastbootError::UnexpectedReply { method: cmd.to_string(), reply: r.to_string() }),
    }
}

async fn log_err(
    err: fastboot::UploadError,
    progress_listener: &ProgressListener<'_>,
) -> Result<(), fastboot::FastbootError> {
    log::error!("{}", err);
    progress_listener.on_error(&err).await.map_err(|e| fastboot::FastbootError::from(e))?;
    return Err(err.into());
}

async fn wait_for_ack<T: AsyncRead + AsyncWrite + Unpin>(
    interface: &mut T,
    progress_listener: &ProgressListener<'_>,
    timeout: Duration,
) -> Result<(), fastboot::FastbootError> {
    match read_and_log_info_with_timeout(interface, timeout).await {
        Ok(Reply::Okay(_)) => Ok(()),
        Ok(reply) => log_err(UnexpectedReply { reply }, progress_listener).await,
        Err(err) => log_err(CouldNotVerifyUpload(err.into()), progress_listener).await,
    }
}

async fn upload_data<T: AsyncRead + AsyncWrite + Unpin>(
    ctx: &FastbootContext,
    data: &[u8],
    interface: &mut T,
    progress_listener: &ProgressListener<'_>,
    timeout: Duration,
    mut bytes_offset: u64,
) -> Result<(), fastboot::FastbootError> {
    let expected = data.len().try_into().unwrap();
    let reply =
        send_with_timeout(ctx.clone(), Command::Download(expected), interface, timeout).await?;
    let Reply::Data(received) = reply else {
        return log_err(UnexpectedReply { reply }, progress_listener).await;
    };
    if received != expected {
        return log_err(WrongSizeResponse { received, expected }, progress_listener).await;
    }

    log::trace!("fastboot: starting upload of {} bytes", data.len());
    // Don't just write_all all the data so that we get incremental progress updates.
    for chunk in data.chunks(BUFFER_SIZE) {
        log::trace!("fastboot: uploading {} bytes", chunk.len());
        if let Err(e) = interface.write_all(chunk).await {
            return log_err(CouldNotWriteToInterface(e), progress_listener).await;
        }

        bytes_offset += u64::try_from(chunk.len()).unwrap();
        progress_listener.on_progress(bytes_offset).await?;
    }

    wait_for_ack(interface, progress_listener, timeout).await?;
    log::trace!("fastboot: uploaded {} bytes", expected);
    Ok(())
}

#[async_trait]
impl<T: AsyncRead + AsyncWrite + Unpin + Debug + Send> Fastboot for FastbootProxy<T> {
    async fn get_var(&mut self, name: &str) -> core::result::Result<String, FastbootError> {
        let command = Command::GetVar(ClientVariable::Oem(name.to_string()));
        match send(self.ctx.clone(), command.clone(), self.interface().await?).await {
            Ok(r) => match r {
                Reply::Okay(v) => {
                    log::trace!("Got var {}. Content: '{:?}", name, v);
                    Ok(v.trim_end_matches('\0').to_string())
                }
                Reply::Fail(message) => {
                    Err(FastbootError::GetVariableError { variable: name.to_string(), message })
                }
                r @ _ => Err(FastbootError::UnexpectedReply {
                    method: command.to_string(),
                    reply: r.to_string(),
                }),
            },
            Err(e) => Err(e.into()),
        }
    }

    async fn get_all_vars(&mut self, listener: Sender<Variable>) -> Result<(), FastbootError> {
        let variable_listener = VariableListener::new(listener);
        let command = Command::GetVar(ClientVariable::All);
        match send_with_listener(
            self.ctx.clone(),
            command.clone(),
            self.interface().await?,
            &variable_listener,
        )
        .await?
        {
            Reply::Okay(_) => Ok(()),
            Reply::Fail(s) => Err(FastbootError::GetAllVarsFailed(s)),
            r @ _ => Err(FastbootError::UnexpectedReply {
                method: command.to_string(),
                reply: r.to_string(),
            }),
        }
    }

    async fn flash(
        &mut self,
        partition_name: &str,
        path: &str,
        listener: Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        let file_to_flash = File::open(path).map_err(FlashError::from)?;
        let size = file_to_flash.metadata().map_err(FlashError::from)?.len();
        let size = u32::try_from(size).map_err(|e| FlashError::InvalidFileSize(e))?;
        let progress_listener = ProgressListener::new(&listener);
        let upload_reply = upload_with_read_timeout(
            self.ctx.clone(),
            size,
            file_to_flash,
            self.interface().await?,
            &progress_listener,
            timeout,
        )
        .await?;
        match upload_reply {
            Reply::Okay(s) => log::debug!("Received response from download command: {}", s),
            Reply::Fail(s) => {
                return Err(FastbootError::StageError(StageError::UploadFailed {
                    path: partition_name.to_string(),
                    message: s,
                }));
            }
            r @ _ => {
                return Err(FastbootError::UnexpectedReply {
                    method: Command::Download(size).to_string(),
                    reply: r.to_string(),
                });
            }
        };
        self.flash_uploaded(partition_name, timeout).await
    }

    async fn flash_from_reader(
        &mut self,
        partition_name: &str,
        size: u32,
        reader: &mut (dyn std::io::Read + Send),
        listener: Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        let progress_listener = ProgressListener::new(&listener);
        let upload_reply = upload_from_reader(
            self.ctx.clone(),
            size,
            reader,
            self.interface().await?,
            &progress_listener,
            timeout,
        )
        .await?;
        match upload_reply {
            Reply::Okay(s) => log::debug!("Received response from download command: {}", s),
            Reply::Fail(s) => {
                return Err(FastbootError::StageError(StageError::UploadFailed {
                    path: partition_name.to_string(),
                    message: s,
                }));
            }
            r @ _ => {
                return Err(FastbootError::UnexpectedReply {
                    method: Command::Download(size).to_string(),
                    reply: r.to_string(),
                });
            }
        };
        self.flash_uploaded(partition_name, timeout).await
    }

    async fn erase(&mut self, partition_name: &str) -> Result<(), FastbootError> {
        let command = Command::Erase(partition_name.to_string());
        let reply = send(self.ctx.clone(), command.clone(), self.interface().await?).await?;
        match reply {
            Reply::Okay(_) => {
                log::debug!("Successfully erased parition: {}", partition_name);
                Ok(())
            }
            Reply::Fail(s) => {
                return Err(FastbootError::ErasePartitionFailed {
                    partition: partition_name.to_string(),
                    message: s,
                });
            }
            r @ _ => Err(FastbootError::UnexpectedReply {
                method: command.to_string(),
                reply: r.to_string(),
            }),
        }
    }

    async fn boot(&mut self) -> Result<(), FastbootError> {
        // Note: the target may not successfully send a response when asked to boot,
        // so let's use a short time-out, and treat a timeout error as a success.
        let reply = handle_timeout_as_okay(
            send_with_timeout(
                self.ctx.clone(),
                Command::Boot,
                self.interface().await?,
                Duration::seconds(3),
            )
            .await,
        )?;
        match reply {
            Reply::Okay(_) => {
                log::debug!("Successfully sent boot");
                Ok(())
            }

            Reply::Fail(s) => return Err(FastbootError::BootFailed(s)),
            r @ _ => Err(FastbootError::UnexpectedReply {
                method: Command::Reboot.to_string(),
                reply: r.to_string(),
            }),
        }
    }

    async fn reboot(&mut self) -> Result<(), FastbootError> {
        // Note: the target may not successfully send a response when asked to reboot,
        // so let's use a short time-out, and treat a timeout error as a success.
        let reply = handle_timeout_as_okay(
            send_with_timeout(
                self.ctx.clone(),
                Command::Reboot,
                self.interface().await?,
                Duration::seconds(3),
            )
            .await,
        )?;
        match reply {
            Reply::Okay(_) => {
                log::debug!("Successfully sent reboot");
                Ok(())
            }
            Reply::Fail(s) => return Err(FastbootError::RebootFailed(s)),
            r @ _ => Err(FastbootError::UnexpectedReply {
                method: Command::Reboot.to_string(),
                reply: r.to_string(),
            }),
        }
    }

    async fn reboot_bootloader(
        &mut self,
        listener: Sender<RebootEvent>,
    ) -> Result<(), FastbootError> {
        // Note: the target may not successfully send a response when asked to reboot-bootloader,
        // so let's use a short time-out, and treat a timeout error as a success.
        let reply = handle_timeout_as_okay(
            send_with_timeout(
                self.ctx.clone(),
                Command::RebootBootLoader,
                self.interface().await?,
                Duration::seconds(3),
            )
            .await,
        )?;
        match reply {
            Reply::Okay(_) => {
                log::debug!("Successfully sent reboot bootloader");
                let send_res = listener.send(RebootEvent::OnReboot).await;
                if send_res.is_err() {
                    log::debug!(
                        "reboot_bootloader hit error sending the reboot event to caller: {:#?}",
                        send_res
                    );
                }
            }
            Reply::Fail(s) => return Err(FastbootError::RebootBootloaderFailed { message: s }),
            r @ _ => {
                return Err(FastbootError::UnexpectedReply {
                    method: Command::RebootBootLoader.to_string(),
                    reply: r.to_string(),
                });
            }
        };
        // Once the target is rebooted, reconnect
        self.reconnect().await?;
        Ok(())
    }

    async fn continue_boot(&mut self) -> Result<(), FastbootError> {
        // Note: the target may not successfully send a response when asked to continue,
        // so let's use a short time-out, and treat a timeout error as a success.
        let reply = handle_timeout_as_okay(
            send_with_timeout(
                self.ctx.clone(),
                Command::Continue,
                self.interface().await?,
                Duration::seconds(3),
            )
            .await,
        )?;
        match reply {
            Reply::Okay(_) => {
                log::debug!("Successfully sent continue");
                Ok(())
            }
            Reply::Fail(s) => return Err(FastbootError::ContinueBootFailed(s)),
            r @ _ => Err(FastbootError::UnexpectedReply {
                method: Command::Continue.to_string(),
                reply: r.to_string(),
            }),
        }
    }

    async fn get_staged(&mut self, path: &str) -> Result<(), FastbootError> {
        match download(self.ctx.clone(), &path.to_string(), self.interface().await?).await? {
            Reply::Okay(_) => {
                log::debug!("Successfully downloaded to \"{}\"", path);
                Ok(())
            }
            Reply::Fail(message) => {
                return Err(FastbootError::DownloadFailed { path: path.to_string(), message });
            }
            r @ _ => Err(FastbootError::UnexpectedReply {
                method: Command::Upload.to_string(),
                reply: r.to_string(),
            }),
        }
    }

    async fn stage(
        &mut self,
        path: &str,
        listener: Sender<UploadProgress>,
    ) -> Result<(), FastbootError> {
        let progress_listener = ProgressListener::new(&listener);
        let file_to_stage = File::open(path).map_err(StageError::from)?;
        let size = file_to_stage.metadata().map_err(StageError::from)?.len();
        let size = u32::try_from(size).map_err(|e| StageError::InvalidFileSize(e))?;
        log::debug!("uploading file size: {}", size);
        match upload(
            self.ctx.clone(),
            size,
            file_to_stage,
            self.interface().await?,
            &progress_listener,
        )
        .await?
        {
            Reply::Okay(s) => {
                log::debug!("Received response from download command: {}", s);
                Ok(())
            }
            Reply::Fail(s) => {
                return Err(FastbootError::StageError(StageError::UploadFailed {
                    path: path.to_string(),
                    message: s,
                }));
            }
            r @ _ => Err(FastbootError::UnexpectedReply {
                method: Command::Download(size).to_string(),
                reply: r.to_string(),
            }),
        }
    }

    async fn set_active(&mut self, slot: &str) -> Result<(), FastbootError> {
        // Note: the target may not successfully send a response when asked to set active,
        // so let's use a short time-out, and treat a timeout error as a success.
        // See b/405436515 for more information.
        let command = Command::SetActive(slot.to_string());
        let reply = handle_timeout_as_okay(
            send_with_timeout(
                self.ctx.clone(),
                command.clone(),
                self.interface().await?,
                Duration::seconds(3),
            )
            .await,
        )?;
        match reply {
            Reply::Okay(_) => {
                log::debug!("Successfully sent set_active");
                Ok(())
            }
            Reply::Fail(message) => {
                return Err(FastbootError::SetActiveFailed { slot: slot.to_string(), message });
            }
            r @ _ => Err(FastbootError::UnexpectedReply {
                method: command.to_string(),
                reply: r.to_string(),
            }),
        }
    }

    async fn oem(&mut self, command: &str) -> Result<(), FastbootError> {
        let command = Command::Oem(command.to_string());
        match send(self.ctx.clone(), command.clone(), self.interface().await?).await? {
            Reply::Okay(_) => {
                log::debug!("Successfully sent oem command \"{}\"", command);
                Ok(())
            }
            Reply::Fail(message) => {
                return Err(FastbootError::OemCommandFailed {
                    command: command.to_string(),
                    message,
                });
            }
            r @ _ => Err(FastbootError::UnexpectedReply {
                method: command.to_string(),
                reply: r.to_string(),
            }),
        }
    }

    async fn stream<'a>(
        &mut self,
        name: &str,
        stream_command: StreamCommand,
        listener: &Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        let progress_listener = ProgressListener::new(listener);
        let StreamCommand { offset_bytes, op } = stream_command;
        let ctx = self.ctx.clone();
        let interface = self.interface().await?;

        let (finishing_cmd, length) = match op {
            StreamOp::Fill { val, length_bytes } => {
                log::trace!("fastboot: filling {} bytes", length_bytes);
                progress_listener
                    .on_progress(offset_bytes + length_bytes)
                    .await
                    .map_err(|e| fastboot::FastbootError::from(e))?;

                (
                    Command::StreamFill {
                        partition: name.to_owned(),
                        offset_bytes,
                        length_bytes,
                        val,
                    },
                    length_bytes,
                )
            }
            StreamOp::Flash { data, crc32 } => {
                upload_data(&ctx, &data, interface, &progress_listener, timeout, offset_bytes)
                    .await?;
                (
                    Command::StreamFlash { partition: name.to_owned(), offset_bytes, crc32 },
                    u64::try_from(data.len()).unwrap(),
                )
            }
        };

        handle_command(&ctx, &finishing_cmd, interface, timeout).await?;
        log::trace!("fastboot: streamed {length} bytes to {name}");
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    use crate::interface_factory::{InterfaceFactoryBase, InterfaceFactoryError};
    use fastboot::test_transport::TestTransport;
    use pretty_assertions::assert_eq;
    use rand::RngExt as _;
    use rand::rngs::SmallRng;
    use std::io::{Read, Seek, SeekFrom, Write};
    use tempfile::{NamedTempFile, TempDir};
    use tokio::sync::mpsc;
    use tokio::sync::mpsc::Receiver;

    #[derive(Default, Debug, Clone)]
    struct TestTransportFactory {}

    #[async_trait]
    impl InterfaceFactoryBase<TestTransport> for TestTransportFactory {
        async fn open(&mut self) -> Result<TestTransport, InterfaceFactoryError> {
            Ok(TestTransport::new())
        }

        async fn close(&self) {}

        async fn rediscover(&mut self) -> Result<(), InterfaceFactoryError> {
            Ok(())
        }
    }

    impl InterfaceFactory<TestTransport> for TestTransportFactory {}

    ///////////////////////////////////////////////////////////////////////////
    //  get_var
    //

    #[fuchsia::test]
    async fn test_get_var() -> Result<()> {
        {
            let mut test_transport = TestTransport::new();
            test_transport.push(Reply::Okay("0.4\0\0".to_string()));
            let mut fastboot_client = FastbootProxy::<TestTransport> {
                target_id: "foo".to_string(),
                interface: Some(test_transport),
                interface_factory: Box::new(TestTransportFactory {}),
                ctx: FastbootContext::new(),
            };

            assert_eq!(fastboot_client.get_var(&"version").await?, "0.4");
        }
        {
            let mut test_transport = TestTransport::new();
            test_transport.push(Reply::Fail("variable doesnt exist".to_string()));
            let mut fastboot_client = FastbootProxy::<TestTransport> {
                target_id: "foo".to_string(),
                interface: Some(test_transport),
                interface_factory: Box::new(TestTransportFactory {}),
                ctx: FastbootContext::new(),
            };

            assert_eq!(fastboot_client.target_id, "foo");
            assert!(fastboot_client.get_var("version").await.is_err())
        }
        {
            let mut test_transport = TestTransport::new();
            test_transport.push(Reply::Data(1234));
            let mut fastboot_client = FastbootProxy::<TestTransport> {
                target_id: "foo".to_string(),
                interface: Some(test_transport),
                interface_factory: Box::new(TestTransportFactory {}),
                ctx: FastbootContext::new(),
            };

            assert_eq!(fastboot_client.target_id, "foo");
            assert!(fastboot_client.get_var("version").await.is_err())
        }
        Ok(())
    }

    ///////////////////////////////////////////////////////////////////////////
    //  get_all_vars
    //

    #[fuchsia::test]
    async fn test_get_all_vars() -> Result<()> {
        let (var_client, mut var_server): (Sender<Variable>, Receiver<Variable>) = mpsc::channel(3);
        let mut test_transport = TestTransport::new();
        test_transport.extend([
            Reply::Info("sis:corona".to_string()),
            Reply::Info("cav:babs".to_string()),
            Reply::Info("name:ianthe".to_string()),
            Reply::Okay("Done".to_string()),
        ]);
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        let _ = fastboot_client.get_all_vars(var_client).await?;
        assert_eq!(
            var_server.recv().await,
            Some(Variable { name: "sis".to_string(), value: "corona".to_string() })
        );

        assert_eq!(
            var_server.recv().await,
            Some(Variable { name: "cav".to_string(), value: "babs".to_string() })
        );
        assert_eq!(
            var_server.recv().await,
            Some(Variable { name: "name".to_string(), value: "ianthe".to_string() })
        );
        assert!(var_server.recv().await.is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_get_all_vars_error() -> Result<()> {
        let (var_client, mut var_server): (Sender<Variable>, Receiver<Variable>) = mpsc::channel(2);
        let mut test_transport = TestTransport::new();
        test_transport
            .extend([Reply::Info("alt:kiriona".to_string()), Reply::Fail("Done".to_string())]);
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };
        assert!(fastboot_client.get_all_vars(var_client).await.is_err());
        assert_eq!(
            var_server.recv().await,
            Some(Variable { name: "alt".to_string(), value: "kiriona".to_string() })
        );
        assert!(var_server.recv().await.is_none());
        Ok(())
    }

    ///////////////////////////////////////////////////////////////////////////
    // oem
    //

    #[fuchsia::test]
    async fn test_oem_ok() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Info("info line 1".to_string()));
        test_transport.push(Reply::Okay("done".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert_eq!(fastboot_client.target_id, "foo");
        assert!(fastboot_client.oem("version").await.is_ok());
        Ok(())
    }
    #[fuchsia::test]
    async fn test_oem_fail() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Fail("this command failed".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert_eq!(fastboot_client.target_id, "foo");
        assert!(fastboot_client.oem("version").await.is_err());

        Ok(())
    }
    #[fuchsia::test]
    async fn test_oem_bail_unexpected_response() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Data(1234));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert_eq!(fastboot_client.target_id, "foo");
        assert!(fastboot_client.oem("version").await.is_err());

        Ok(())
    }

    ///////////////////////////////////////////////////////////////////////////
    // erase
    //

    #[fuchsia::test]
    async fn test_erase_ok() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Okay("done".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert_eq!(fastboot_client.target_id, "foo");
        fastboot_client.erase("slotA").await?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_erase_fail() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Fail("could not erase".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert!(fastboot_client.erase("slotB").await.is_err());

        Ok(())
    }
    #[fuchsia::test]
    async fn test_erase_bail_unexpected_response() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Data(1234));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert!(fastboot_client.erase("slotC").await.is_err());

        Ok(())
    }

    ///////////////////////////////////////////////////////////////////////////
    // boot
    //

    #[fuchsia::test]
    async fn test_boot_ok() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Okay("done".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert_eq!(fastboot_client.target_id, "foo");
        fastboot_client.boot().await?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_boot_fail() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Fail("could not boot".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert!(fastboot_client.boot().await.is_err());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_boot_unexpected_response() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Data(1234));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert!(fastboot_client.boot().await.is_err());

        Ok(())
    }

    ///////////////////////////////////////////////////////////////////////////
    // reboot
    //

    #[fuchsia::test]
    async fn test_reboot_ok() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Okay("done".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert_eq!(fastboot_client.target_id, "foo");
        fastboot_client.reboot().await?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_reboot_fail() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Fail("could not reboot".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert!(fastboot_client.reboot().await.is_err());

        Ok(())
    }
    #[fuchsia::test]
    async fn test_reboot_unexpected_response() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Data(1234));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert!(fastboot_client.reboot().await.is_err());

        Ok(())
    }

    ///////////////////////////////////////////////////////////////////////////
    // reboot_bootloader
    //

    #[fuchsia::test]
    async fn test_reboot_bootloader_ok() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Okay("done".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        let (var_client, mut var_server): (Sender<RebootEvent>, Receiver<RebootEvent>) =
            mpsc::channel(3);

        assert_eq!(fastboot_client.target_id, "foo");
        fastboot_client.reboot_bootloader(var_client).await?;

        assert_eq!(var_server.recv().await, Some(RebootEvent::OnReboot));
        Ok(())
    }

    #[fuchsia::test]
    async fn test_reboot_bootloader_fail() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Fail("could not reboot bootloader".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        let (var_client, mut var_server): (Sender<RebootEvent>, Receiver<RebootEvent>) =
            mpsc::channel(3);
        assert!(fastboot_client.reboot_bootloader(var_client).await.is_err());

        assert!(var_server.recv().await.is_none());

        Ok(())
    }
    #[fuchsia::test]
    async fn test_reboot_bootloader_unexpected_response() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Data(1234));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        let (var_client, mut var_server): (Sender<RebootEvent>, Receiver<RebootEvent>) =
            mpsc::channel(3);
        assert!(fastboot_client.reboot_bootloader(var_client).await.is_err());

        assert!(var_server.recv().await.is_none());

        Ok(())
    }

    ///////////////////////////////////////////////////////////////////////////
    // continue_boot
    //

    #[fuchsia::test]
    async fn test_continue_boot_ok() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Okay("done".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        fastboot_client.continue_boot().await?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_continue_boot_fail() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Fail("could not continue boot".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert!(fastboot_client.continue_boot().await.is_err());

        Ok(())
    }
    #[fuchsia::test]
    async fn test_continue_boot_unexpected_response() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Data(1234));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert!(fastboot_client.continue_boot().await.is_err());

        Ok(())
    }

    ///////////////////////////////////////////////////////////////////////////
    // set_active
    //

    #[fuchsia::test]
    async fn test_set_active_ok() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Okay("done".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert_eq!(fastboot_client.target_id, "foo");
        fastboot_client.set_active("slotA").await?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_set_active_fail() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Fail("could not set active".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert!(fastboot_client.set_active("slotB").await.is_err());

        Ok(())
    }
    #[fuchsia::test]
    async fn test_set_active_bail_unexpected_response() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Data(1234));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert!(fastboot_client.set_active("slotC").await.is_err());

        Ok(())
    }

    ///////////////////////////////////////////////////////////////////////////
    // get_staged
    //

    #[fuchsia::test]
    async fn test_get_staged_ok() -> Result<()> {
        let tmpdir = TempDir::new().unwrap();

        // Generate temporary file
        let (mut file, temp_path) = NamedTempFile::new_in(&tmpdir).unwrap().into_parts();

        let mut test_transport = TestTransport::new();
        test_transport.extend([
            Reply::Data(12),                 // Upload Response (size)
            Reply::Data(1234),               // Upload Response
            Reply::Okay("Done".to_string()), // Upload Done
        ]);

        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        fastboot_client.get_staged(temp_path.to_str().unwrap()).await?;

        let mut buf = Vec::<u8>::new();
        file.read_to_end(&mut buf)?;
        assert_eq!(buf, "DATA000004D2".as_bytes());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_get_staged_fail() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Fail("could not get staged".to_string()));
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        assert!(fastboot_client.get_staged("slotB").await.is_err());

        Ok(())
    }

    ///////////////////////////////////////////////////////////////////////////
    // stage
    //

    #[fuchsia::test]
    async fn test_stage_ok() -> Result<()> {
        let tmpdir = TempDir::new().unwrap();
        // Generate a large temporary file
        let (mut file, temp_path) = NamedTempFile::new_in(&tmpdir).unwrap().into_parts();
        let mut rng: SmallRng = rand::make_rng();
        let mut buf = Vec::<u8>::new();
        buf.resize(1 * 4096, 0);
        rng.fill(&mut buf[..]);
        file.write_all(&buf).unwrap();
        file.flush().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut test_transport = TestTransport::new();
        test_transport.extend([
            Reply::Data(4096),               // Download Response
            Reply::Okay("done".to_string()), // Download Okay
        ]);
        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        let (var_client, mut var_server): (Sender<UploadProgress>, Receiver<UploadProgress>) =
            mpsc::channel(3);

        fastboot_client.stage(temp_path.to_str().unwrap(), var_client).await?;

        assert!(matches!(var_server.recv().await, Some(UploadProgress::OnStarted { size: 4096 })));
        assert!(matches!(
            var_server.recv().await,
            Some(UploadProgress::OnProgress { bytes_written: 4096 })
        ));
        assert!(matches!(var_server.recv().await, Some(UploadProgress::OnFinished)));
        assert!(var_server.recv().await.is_none());

        Ok(())
    }

    #[fuchsia::test]
    async fn test_stage_fail() -> Result<()> {
        let tmpdir = TempDir::new().unwrap();

        // Generate a large temporary file
        let (mut file, temp_path) = NamedTempFile::new_in(&tmpdir).unwrap().into_parts();
        let mut rng: SmallRng = rand::make_rng();
        let mut buf = Vec::<u8>::new();
        buf.resize(1 * 4096, 0);
        rng.fill(&mut buf[..]);
        file.write_all(&buf).unwrap();
        file.flush().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Fail("could not stage".to_string()));

        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        let (var_client, mut var_server): (Sender<UploadProgress>, Receiver<UploadProgress>) =
            mpsc::channel(2);

        assert!(fastboot_client.stage(temp_path.to_str().unwrap(), var_client).await.is_err());

        assert!(var_server.recv().await.is_none());
        Ok(())
    }

    ///////////////////////////////////////////////////////////////////////////
    // flash
    //

    #[fuchsia::test]
    async fn test_flash_ok() -> Result<()> {
        let tmpdir = TempDir::new().unwrap();

        // Generate a large temporary file
        let (mut file, temp_path) = NamedTempFile::new_in(&tmpdir).unwrap().into_parts();
        let mut rng: SmallRng = rand::make_rng();
        let mut buf = Vec::<u8>::new();
        buf.resize(1 * 4096, 0);
        rng.fill(&mut buf[..]);
        file.write_all(&buf).unwrap();
        file.flush().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        let mut test_transport = TestTransport::new();
        test_transport.extend([
            Reply::Data(4096),           // Download
            Reply::Okay("".to_string()), // Download Ok
            Reply::Okay("".to_string()), // Flash Ok
        ]);

        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        let (var_client, mut var_server): (Sender<UploadProgress>, Receiver<UploadProgress>) =
            mpsc::channel(3);

        fastboot_client
            .flash("partition1", temp_path.to_str().unwrap(), var_client, Duration::seconds(1))
            .await?;

        assert!(matches!(var_server.recv().await, Some(UploadProgress::OnStarted { size: 4096 })));
        assert!(matches!(
            var_server.recv().await,
            Some(UploadProgress::OnProgress { bytes_written: 4096 })
        ));
        assert!(matches!(var_server.recv().await, Some(UploadProgress::OnFinished)));
        assert!(var_server.recv().await.is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_flash_fail() -> Result<()> {
        let tmpdir = TempDir::new().unwrap();

        // Generate a large temporary file
        let (mut file, temp_path) = NamedTempFile::new_in(&tmpdir).unwrap().into_parts();
        let mut rng: SmallRng = rand::make_rng();
        let mut buf = Vec::<u8>::new();
        buf.resize(1 * 4096, 0);
        rng.fill(&mut buf[..]);
        file.write_all(&buf).unwrap();
        file.flush().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();

        let mut test_transport = TestTransport::new();
        test_transport.push(Reply::Fail("could not stage".to_string()));

        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        let (var_client, mut var_server): (Sender<UploadProgress>, Receiver<UploadProgress>) =
            mpsc::channel(2);

        assert!(
            fastboot_client
                .flash("partition1", temp_path.to_str().unwrap(), var_client, Duration::seconds(1))
                .await
                .is_err()
        );

        assert!(var_server.recv().await.is_none());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_stream_non_empty_okay_replies() -> Result<()> {
        let mut test_transport = TestTransport::new();
        test_transport.extend([
            Reply::Data(4),
            Reply::Okay("0.005s".to_string()), // Non-empty download ack
            Reply::Okay("flashed ok".to_string()), // Non-empty stream-flash ack
            Reply::Okay("filled ok".to_string()), // Non-empty stream-fill ack
        ]);

        let mut fastboot_client = FastbootProxy::<TestTransport> {
            target_id: "foo".to_string(),
            interface: Some(test_transport),
            interface_factory: Box::new(TestTransportFactory {}),
            ctx: FastbootContext::new(),
        };

        let (progress_tx, mut progress_rx) = mpsc::channel(4);

        let flash_cmd = StreamCommand {
            offset_bytes: 0,
            op: StreamOp::Flash { data: bytes::Bytes::from_static(&[1, 2, 3, 4]), crc32: 0x1234 },
        };
        fastboot_client.stream("zircon_a", flash_cmd, &progress_tx, Duration::seconds(1)).await?;

        let fill_cmd =
            StreamCommand { offset_bytes: 4, op: StreamOp::Fill { val: 0, length_bytes: 4096 } };
        fastboot_client.stream("zircon_a", fill_cmd, &progress_tx, Duration::seconds(1)).await?;

        assert_eq!(progress_rx.recv().await, Some(UploadProgress::OnProgress { bytes_written: 4 }));
        assert_eq!(
            progress_rx.recv().await,
            Some(UploadProgress::OnProgress { bytes_written: 4100 })
        );
        Ok(())
    }
}
