// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use chrono::Duration;
use fastboot::command::Command;
use tokio::sync::mpsc::Sender;

use crate::stream::StreamCommand;

pub trait FastbootInterface: std::fmt::Debug + Fastboot {}

#[async_trait]
pub trait Fastboot: Send {
    async fn get_var(&mut self, name: &str) -> Result<String, FastbootError>;

    async fn get_all_vars(&mut self, listener: Sender<Variable>) -> Result<(), FastbootError>;

    async fn flash(
        &mut self,
        partition_name: &str,
        path: &str,
        listener: Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError>;

    async fn flash_from_reader(
        &mut self,
        partition_name: &str,
        size: u32,
        reader: &mut (dyn std::io::Read + Send),
        listener: Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError>;

    async fn erase(&mut self, partition_name: &str) -> Result<(), FastbootError>;

    async fn boot(&mut self) -> Result<(), FastbootError>;

    async fn reboot(&mut self) -> Result<(), FastbootError>;

    async fn reboot_bootloader(
        &mut self,
        listener: Sender<RebootEvent>,
    ) -> Result<(), FastbootError>;

    async fn continue_boot(&mut self) -> Result<(), FastbootError>;

    async fn get_staged(&mut self, path: &str) -> Result<(), FastbootError>;

    async fn stage(
        &mut self,
        path: &str,
        listener: Sender<UploadProgress>,
    ) -> Result<(), FastbootError>;

    async fn set_active(&mut self, slot: &str) -> Result<(), FastbootError>;

    /// Sends OEM commands.
    ///
    /// On success returns the OKAY response, and all INFO responses.
    async fn oem(&mut self, command: &str) -> Result<(), FastbootError>;

    async fn stream<'a>(
        &mut self,
        partition_name: &str,
        stream_command: StreamCommand,
        listener: &Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError>;
}

// We sometimes get called with the Box<FastbootInterface> that
// ffx_fastboot_connection_factory::get_fastboot_interface() returns
#[async_trait]
impl<F: Fastboot + ?Sized> Fastboot for Box<F> {
    async fn get_var(&mut self, name: &str) -> Result<String, FastbootError> {
        (**self).get_var(name).await
    }

    async fn get_all_vars(&mut self, listener: Sender<Variable>) -> Result<(), FastbootError> {
        (**self).get_all_vars(listener).await
    }

    async fn flash(
        &mut self,
        partition_name: &str,
        path: &str,
        listener: Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        (**self).flash(partition_name, path, listener, timeout).await
    }

    async fn flash_from_reader(
        &mut self,
        partition_name: &str,
        size: u32,
        reader: &mut (dyn std::io::Read + Send),
        listener: Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        (**self).flash_from_reader(partition_name, size, reader, listener, timeout).await
    }

    async fn erase(&mut self, partition_name: &str) -> Result<(), FastbootError> {
        (**self).erase(partition_name).await
    }

    async fn boot(&mut self) -> Result<(), FastbootError> {
        (**self).boot().await
    }

    async fn reboot(&mut self) -> Result<(), FastbootError> {
        (**self).reboot().await
    }

    async fn reboot_bootloader(
        &mut self,
        listener: Sender<RebootEvent>,
    ) -> Result<(), FastbootError> {
        (**self).reboot_bootloader(listener).await
    }

    async fn continue_boot(&mut self) -> Result<(), FastbootError> {
        (**self).continue_boot().await
    }

    async fn get_staged(&mut self, path: &str) -> Result<(), FastbootError> {
        (**self).get_staged(path).await
    }

    async fn stage(
        &mut self,
        path: &str,
        listener: Sender<UploadProgress>,
    ) -> Result<(), FastbootError> {
        (**self).stage(path, listener).await
    }

    async fn set_active(&mut self, slot: &str) -> Result<(), FastbootError> {
        (**self).set_active(slot).await
    }

    async fn oem(&mut self, command: &str) -> Result<(), FastbootError> {
        (**self).oem(command).await
    }

    async fn stream<'a>(
        &mut self,
        partition_name: &str,
        stream_command: StreamCommand,
        listener: &Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        (**self).stream(partition_name, stream_command, listener, timeout).await
    }
}

impl<F: FastbootInterface + ?Sized> FastbootInterface for Box<F> {}

#[async_trait]
impl<F: Fastboot + ?Sized> Fastboot for &mut F {
    async fn get_var(&mut self, name: &str) -> Result<String, FastbootError> {
        (**self).get_var(name).await
    }

    async fn get_all_vars(&mut self, listener: Sender<Variable>) -> Result<(), FastbootError> {
        (**self).get_all_vars(listener).await
    }

    async fn flash(
        &mut self,
        partition_name: &str,
        path: &str,
        listener: Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        (**self).flash(partition_name, path, listener, timeout).await
    }

    async fn flash_from_reader(
        &mut self,
        partition_name: &str,
        size: u32,
        reader: &mut (dyn std::io::Read + Send),
        listener: Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        (**self).flash_from_reader(partition_name, size, reader, listener, timeout).await
    }

    async fn erase(&mut self, partition_name: &str) -> Result<(), FastbootError> {
        (**self).erase(partition_name).await
    }

    async fn boot(&mut self) -> Result<(), FastbootError> {
        (**self).boot().await
    }

    async fn reboot(&mut self) -> Result<(), FastbootError> {
        (**self).reboot().await
    }

    async fn reboot_bootloader(
        &mut self,
        listener: Sender<RebootEvent>,
    ) -> Result<(), FastbootError> {
        (**self).reboot_bootloader(listener).await
    }

    async fn continue_boot(&mut self) -> Result<(), FastbootError> {
        (**self).continue_boot().await
    }

    async fn get_staged(&mut self, path: &str) -> Result<(), FastbootError> {
        (**self).get_staged(path).await
    }

    async fn stage(
        &mut self,
        path: &str,
        listener: Sender<UploadProgress>,
    ) -> Result<(), FastbootError> {
        (**self).stage(path, listener).await
    }

    async fn set_active(&mut self, slot: &str) -> Result<(), FastbootError> {
        (**self).set_active(slot).await
    }

    async fn oem(&mut self, command: &str) -> Result<(), FastbootError> {
        (**self).oem(command).await
    }

    async fn stream<'a>(
        &mut self,
        partition_name: &str,
        stream_command: StreamCommand,
        listener: &Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        (**self).stream(partition_name, stream_command, listener, timeout).await
    }
}

impl<F: FastbootInterface + ?Sized> FastbootInterface for &mut F {}

#[derive(Debug, PartialEq)]
pub enum RebootEvent {
    OnReboot,
}

#[derive(Debug)]
pub enum UploadProgress {
    OnReady { partition: String, files: u64 },
    OnStarted { size: u64 },
    OnFinished,
    OnProgress { bytes_written: u64 },
    OnError { error: anyhow::Error },
}

impl PartialEq for UploadProgress {
    // Can't derive because anyhow::Error isn't PartialEq
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::OnReady { partition: p1, files: f1 },
                Self::OnReady { partition: p2, files: f2 },
            ) => p1 == p2 && f1 == f2,
            (Self::OnStarted { size: s1 }, Self::OnStarted { size: s2 }) => s1 == s2,
            (Self::OnFinished, Self::OnFinished) => true,
            (Self::OnProgress { bytes_written: b1 }, Self::OnProgress { bytes_written: b2 }) => {
                b1 == b2
            }
            (Self::OnError { error: e1 }, Self::OnError { error: e2 }) => {
                e1.to_string() == e2.to_string()
            }
            (_, _) => false,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Variable {
    pub name: String,
    pub value: String,
}

#[derive(thiserror::Error, Debug)]
pub enum FastbootError {
    #[error("Fastboot error: {0}")]
    Fastboot(#[from] fastboot::FastbootError),

    #[error("Interface factory error: {0}")]
    Factory(#[from] crate::interface_factory::InterfaceFactoryError),

    #[error("Failed to send progress update")]
    ProgressSendError,

    #[error("Unexpected reply from fastboot device for {}: {}", method, reply)]
    UnexpectedReply { method: String, reply: String },

    #[error("Failed to set_active slot {}: {}", slot, message)]
    SetActiveFailed { slot: String, message: String },

    #[error("Failed to get {}: {}", variable, message)]
    GetVariableError { variable: String, message: String },

    #[error("Download of staged file to {} failed: {}", path, message)]
    DownloadFailed { path: String, message: String },

    #[error("Failed to oem \"{}\": {}", command, message)]
    OemCommandFailed { command: String, message: String },

    #[error("Failed to boot: {}", .0)]
    BootFailed(String),

    #[error("Reboot failed: {}", .0)]
    RebootFailed(String),

    #[error("Continuing boot failed: {}", .0)]
    ContinueBootFailed(String),

    #[error("Failed to erase partition {}: {}", partition, message)]
    ErasePartitionFailed { partition: String, message: String },

    #[error("Failed to stage file {}", .0 )]
    StageError(#[from] StageError),

    #[error("Failed to stage file {}", .0 )]
    FlashError(#[from] FlashError),

    #[error("Failed to reboot to bootloader: {}", message)]
    RebootBootloaderFailed { message: String },

    #[error("Failed to get all variables: {}", .0 )]
    GetAllVarsFailed(String),

    #[error("Did not write correct number of bytes. Got {:?}. Want {}", written, expected)]
    ShortWrite { written: usize, expected: usize },
}

#[derive(thiserror::Error, Debug)]
pub enum FlashError {
    #[error("{}", .0)]
    Error(#[from] anyhow::Error),

    #[error("{}", .0)]
    IoError(#[from] std::io::Error),

    #[error("{}", .0)]
    InvalidFileSize(#[source] std::num::TryFromIntError),

    #[error("Failed to upload {}: {}", partition, message)]
    FlashFailed { partition: String, message: String },

    #[error("Stream command '{command}' failed: {message}")]
    StreamFailed { command: Command, message: String },

    #[error("{}", .0)]
    UploadFailed(#[from] StageError),

    #[error("{}", .0)]
    TimeoutError(String),
}

#[derive(thiserror::Error, Debug)]
pub enum StageError {
    #[error("{}", .0)]
    IoError(#[from] std::io::Error),

    #[error("{}", .0)]
    InvalidFileSize(#[source] std::num::TryFromIntError),

    #[error("Failed to upload {}: {}", path, message)]
    UploadFailed { path: String, message: String },
}
