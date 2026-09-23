// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::analytics::PointOfFailure;
use crate::helpers::rediscover_helper;
use anyhow::{Context as _, Result};
use async_trait::async_trait;
use discovery::{FastbootConnectionState, TargetHandle, TargetState};
use ffx_config::EnvironmentContext;
use ffx_diagnostics_analytics::{ResultExt, mark_point_of_failure};
use ffx_fastboot_interface::interface_factory::{
    InterfaceFactory, InterfaceFactoryBase, InterfaceFactoryError,
};
use ffx_fastboot_transport_interface::udp::{UdpNetworkInterface, open};
use fuchsia_async::Timer;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

///////////////////////////////////////////////////////////////////////////////
// UdpFactory
//

#[derive(Debug, Clone)]
pub struct UdpFactory {
    target_name: String,
    fastboot_devices_file_path: Option<PathBuf>,
    addr: SocketAddr,
    open_retries: Option<u64>,
    retry_wait_seconds: u64,
    context: EnvironmentContext,
}

impl UdpFactory {
    pub fn new(
        context: &EnvironmentContext,
        target_name: String,
        fastboot_devices_file_path: Option<PathBuf>,
        addr: SocketAddr,
        open_retries: Option<u64>,
        retry_wait_seconds: u64,
    ) -> Self {
        Self {
            context: context.clone(),
            target_name,
            fastboot_devices_file_path,
            addr,
            open_retries,
            retry_wait_seconds,
        }
    }
}

impl Drop for UdpFactory {
    fn drop(&mut self) {
        futures::executor::block_on(async move {
            self.close().await;
        });
    }
}

#[async_trait]
impl InterfaceFactoryBase<UdpNetworkInterface> for UdpFactory {
    async fn open(&mut self) -> Result<UdpNetworkInterface, InterfaceFactoryError> {
        let wait_duration = Duration::from_secs(self.retry_wait_seconds);
        let mut try_count = 1;
        loop {
            if let Some(max_retries) = self.open_retries {
                if try_count > max_retries {
                    let err = InterfaceFactoryError::ConnectionError(
                        "UDP".to_string(),
                        self.addr,
                        max_retries,
                    );
                    mark_point_of_failure(PointOfFailure::FactoryOpenError("udp".into(), &err))
                        .await;
                    break Err(err);
                }
            }
            try_count += 1;
            match open(self.addr)
                .await
                .with_context(|| format!("connecting via UDP to Fastboot address: {}", self.addr))
            {
                Ok(interface) => return Ok(interface),
                Err(e) => {
                    log::debug!(
                        "Attempt {}. Got error connecting to fastboot address: {}",
                        try_count - 1,
                        e,
                    );
                    Timer::new(wait_duration).await;
                }
            }
        }
    }

    async fn close(&self) {
        log::debug!("Closing Fastboot UDP Factory for: {}", self.addr);
    }

    async fn rediscover(&mut self) -> Result<(), InterfaceFactoryError> {
        rediscover_helper(
            &self.context,
            &self.fastboot_devices_file_path,
            &self.target_name,
            filter_target,
            &mut |connection_state| {
                match connection_state {
                    FastbootConnectionState::Udp(addrs) => {
                        self.addr = addrs.iter().find_map(|x| x.try_into().ok()).unwrap();
                    }
                    s @ _ => {
                        return Err(InterfaceFactoryError::RediscoverTargetNotInCorrectTransport(
                            self.target_name.clone(),
                            "UDP".to_string(),
                            s.to_string(),
                        ));
                    }
                }
                Ok(())
            },
        )
        .await
        .map_err(|e| InterfaceFactoryError::from(e))
        .or_else_analytics(|e| PointOfFailure::FactoryRediscoveryError("udp".into(), e).into())
        .await
    }
}

impl InterfaceFactory<UdpNetworkInterface> for UdpFactory {}

fn filter_target(handle: &TargetHandle) -> bool {
    match &handle.state {
        TargetState::Fastboot(ts)
            if matches!(ts.connection_state, FastbootConnectionState::Udp(_)) =>
        {
            log::trace!("Filtered and found target handle: {}", handle);
            true
        }
        state @ _ => {
            log::debug!("Target state {} is not  UDP Fastboot... skipping", state);
            false
        }
    }
}
