// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use futures::io::{AsyncRead, AsyncWrite};
use std::net::SocketAddr;
use thiserror::Error;

///////////////////////////////////////////////////////////////////////////////
// Factory
//

#[derive(Debug, Error)]
pub enum InterfaceFactoryError {
    #[error("USB discovery error: {0}")]
    Usb(#[from] usb_fastboot_discovery::UsbDiscoveryError),

    #[error("{}", .0)]
    InterfaceOpenError(#[from] anyhow::Error),
    #[error("{}", .0)]
    RediscoverTargetError(String),
    #[error("When rediscovering target: {}, expected target to be rediscovered in Fastboot mode. Got: {}", .0, .1)]
    RediscoverTargetNotInFastboot(String, String),
    #[error("When rediscovering target: {}, expected target to be rediscovered in Fastboot over {} mode. Got: {}", .0, .1, .2)]
    RediscoverTargetNotInCorrectTransport(String, String, String),
    #[error("Could not connect via {} to fastboot address: {} after {} tries", .0, .1, .2)]
    ConnectionError(String, SocketAddr, u64),
}

#[async_trait]
pub trait InterfaceFactoryBase<T: AsyncRead + AsyncWrite + Unpin + Send> {
    async fn open(&mut self) -> Result<T, InterfaceFactoryError>;
    async fn close(&self);
    async fn rediscover(&mut self) -> Result<(), InterfaceFactoryError>;
}

#[async_trait]
pub trait InterfaceFactory<T: AsyncRead + AsyncWrite + Unpin + Send>:
    std::fmt::Debug + InterfaceFactoryBase<T>
{
}
