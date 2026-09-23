// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common_utils::common::macros::{fx_err_and_bail, with_line};
use anyhow::Error;
use fidl_fuchsia_bluetooth_a2dp::{AudioModeMarker, AudioModeProxy, Role};
use fuchsia_component::client;
use fuchsia_sync::Mutex;
use log::info;

#[derive(Debug)]
pub struct A2dpFacade {
    audio_mode_proxy: Mutex<Option<AudioModeProxy>>,
}

/// Perform Bluetooth A2DP functions for both Sink and Source.
impl A2dpFacade {
    pub fn new() -> A2dpFacade {
        A2dpFacade { audio_mode_proxy: Mutex::new(None) }
    }

    /// Initialize the proxy to the AudioMode service.
    pub async fn init_audio_mode_proxy(&self) -> Result<(), Error> {
        let tag = "A2dpFacade::init_audio_mode_proxy";
        let mut proxy_locked = self.audio_mode_proxy.lock();
        if proxy_locked.is_some() {
            info!(
                tag = &with_line!(tag);
                "Current A2DP AudioMode proxy: {0:?}", self.audio_mode_proxy
            );
            return Ok(());
        }
        match client::connect_to_protocol::<AudioModeMarker>() {
            Ok(proxy) => {
                *proxy_locked = Some(proxy);
                Ok(())
            }
            Err(err) => {
                fx_err_and_bail!(
                    &with_line!(tag),
                    format_err!("Failed to create A2DP AudioMode proxy: {err}")
                );
            }
        }
    }

    /// Updates the A2DP Audio Role of the active host device.
    ///
    /// # Arguments
    /// * `role` - The new role to assume. If this role is already set, this is a no-op.
    pub async fn set_role(&self, role: Role) -> Result<(), Error> {
        let tag = "A2dpFacade::set_role";
        let proxy_opt = self.audio_mode_proxy.lock().clone();

        match proxy_opt {
            Some(proxy) => {
                proxy.set_role(role).await?;
                info!("new A2DP audio mode set: {:?}", role);
                Ok(())
            }
            None => {
                fx_err_and_bail!(&with_line!(tag), "no A2DP Audio Mode Proxy detected");
            }
        }
    }
}
