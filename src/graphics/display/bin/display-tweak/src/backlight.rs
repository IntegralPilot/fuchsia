// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error};
use argh::FromArgs;
use fidl_fuchsia_hardware_backlight as backlight;
use fuchsia_component::client::Service;

use crate::utils::{self, on_off_to_bool};

/// Obtains a handle to the backlight service at the default hard-coded path.
async fn open_backlight() -> Result<backlight::DeviceProxy, Error> {
    log::trace!("Opening backlight device");
    let device = Service::open(backlight::ServiceMarker)
        .context("Failed to open service")?
        .watch_for_any()
        .await
        .context("Failed to find instance")?
        .connect_to_backlight()
        .context("Failed to connect to backlight service")?;
    log::info!("Opening backlight");
    Ok(device)
}

#[derive(Debug)]
struct Backlight {
    device: backlight::DeviceProxy,
}

/// Read and change the panel's backlight configuration.
#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "backlight")]
pub struct BacklightCmd {
    /// change the panel's brightness - valid values are from 0.0 to 1.0
    #[argh(option, long = "brightness")]
    set_brightness: Option<f64>,

    /// turn the panel's backlight on or off
    #[argh(option, long = "power", from_str_fn(on_off_to_bool))]
    set_power: Option<bool>,
}

impl Backlight {
    pub async fn new() -> Result<Backlight, Error> {
        let device = open_backlight().await?;
        Ok(Backlight { device })
    }

    pub async fn read_and_modify_state(&mut self, args: &BacklightCmd) -> Result<(), Error> {
        log::trace!("Querying backlight state");
        let backlight_state = utils::flatten_zx_error(self.device.get_state_normalized().await)
            .context("Failed to get current backlight state")?;
        println!("Current panel backlight state: {:?}", backlight_state);

        let new_state = backlight::State {
            backlight_on: args.set_power.unwrap_or(backlight_state.backlight_on),
            brightness: args.set_brightness.unwrap_or(backlight_state.brightness),
        };

        // Don't issue a FIDL call that wouldn't make any change. This avoids an
        // unintuitive race condition where the changes of another brightness
        // management agent are discarded.
        if args.set_power.is_none() && args.set_brightness.is_none() {
            return Ok(());
        }

        log::trace!("Setting new backlight state");
        utils::flatten_zx_error(self.device.set_state_normalized(&new_state).await)
            .context("Failed to apply new backlight settings")?;

        println!("New panel backlight state: {:?}", new_state);

        Ok(())
    }
}

impl BacklightCmd {
    pub async fn exec(&self) -> Result<(), Error> {
        let mut backlight = Backlight::new().await?;
        backlight.read_and_modify_state(self).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use googletest::{expect_eq, expect_that, gtest, matchers};

    use futures::{StreamExt, future};

    #[gtest]
    #[fuchsia::test]
    async fn read_and_modify_state_no_changes() {
        let (device, mut backlight_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<backlight::DeviceMarker>();
        let mut backlight = Backlight { device };

        let test_future = async move {
            let args = BacklightCmd { set_brightness: None, set_power: None };
            expect_that!(
                backlight.read_and_modify_state(&args).await,
                matchers::ok(matchers::anything())
            );
        };
        let service_future = async move {
            match backlight_request_stream.next().await.unwrap() {
                Ok(backlight::DeviceRequest::GetStateNormalized { responder }) => {
                    responder
                        .send(Ok(&backlight::State { backlight_on: true, brightness: 0.5 }))
                        .unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            }
        };
        future::join(test_future, service_future).await;
    }

    #[gtest]
    #[fuchsia::test]
    async fn read_and_modify_state_power_change() {
        let (device, mut backlight_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<backlight::DeviceMarker>();
        let mut backlight = Backlight { device };

        let test_future = async move {
            let args = BacklightCmd { set_brightness: None, set_power: Some(false) };
            expect_that!(
                backlight.read_and_modify_state(&args).await,
                matchers::ok(matchers::anything())
            );
        };
        let service_future = async move {
            match backlight_request_stream.next().await.unwrap() {
                Ok(backlight::DeviceRequest::GetStateNormalized { responder }) => {
                    responder
                        .send(Ok(&backlight::State { backlight_on: true, brightness: 0.5 }))
                        .unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            }
            match backlight_request_stream.next().await.unwrap() {
                Ok(backlight::DeviceRequest::SetStateNormalized { state, responder }) => {
                    expect_eq!(state.backlight_on, false);
                    expect_eq!(state.brightness, 0.5);
                    responder.send(Ok(())).unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            }
        };
        future::join(test_future, service_future).await;
    }

    #[gtest]
    #[fuchsia::test]
    async fn read_and_modify_state_brightness_change() {
        let (device, mut backlight_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<backlight::DeviceMarker>();
        let mut backlight = Backlight { device };

        let test_future = async move {
            let args = BacklightCmd { set_brightness: Some(1.0), set_power: None };
            expect_that!(
                backlight.read_and_modify_state(&args).await,
                matchers::ok(matchers::anything())
            );
        };
        let service_future = async move {
            match backlight_request_stream.next().await.unwrap() {
                Ok(backlight::DeviceRequest::GetStateNormalized { responder }) => {
                    responder
                        .send(Ok(&backlight::State { backlight_on: true, brightness: 0.5 }))
                        .unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            }
            match backlight_request_stream.next().await.unwrap() {
                Ok(backlight::DeviceRequest::SetStateNormalized { state, responder }) => {
                    expect_eq!(state.backlight_on, true);
                    expect_eq!(state.brightness, 1.0);
                    responder.send(Ok(())).unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            }
        };
        future::join(test_future, service_future).await;
    }

    #[gtest]
    #[fuchsia::test]
    async fn read_and_modify_state_read_error() {
        let (device, mut backlight_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<backlight::DeviceMarker>();
        let mut backlight = Backlight { device };

        let test_future = async move {
            let args = BacklightCmd { set_brightness: Some(1.0), set_power: None };
            let result = backlight.read_and_modify_state(&args).await;
            expect_that!(
                result,
                matchers::err(matchers::displays_as(matchers::eq(
                    "Failed to get current backlight state"
                )))
            );
        };
        let service_future = async move {
            match backlight_request_stream.next().await.unwrap() {
                Ok(backlight::DeviceRequest::GetStateNormalized { responder }) => {
                    responder.send(Err(zx::sys::ZX_ERR_NOT_SUPPORTED)).unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            }
        };
        future::join(test_future, service_future).await;
    }

    #[gtest]
    #[fuchsia::test]
    async fn read_and_modify_state_modify_error() {
        let (device, mut backlight_request_stream) =
            fidl::endpoints::create_proxy_and_stream::<backlight::DeviceMarker>();
        let mut backlight = Backlight { device };

        let test_future = async move {
            let args = BacklightCmd { set_brightness: Some(1.0), set_power: None };
            let result = backlight.read_and_modify_state(&args).await;
            expect_that!(
                result,
                matchers::err(matchers::displays_as(matchers::eq(
                    "Failed to apply new backlight settings"
                )))
            );
        };
        let service_future = async move {
            match backlight_request_stream.next().await.unwrap() {
                Ok(backlight::DeviceRequest::GetStateNormalized { responder }) => {
                    responder
                        .send(Ok(&backlight::State { backlight_on: true, brightness: 0.5 }))
                        .unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            }
            match backlight_request_stream.next().await.unwrap() {
                Ok(backlight::DeviceRequest::SetStateNormalized { responder, .. }) => {
                    responder.send(Err(zx::sys::ZX_ERR_NOT_SUPPORTED)).unwrap();
                }
                request => panic!("Unexpected request: {:?}", request),
            }
        };
        future::join(test_future, service_future).await;
    }
}
