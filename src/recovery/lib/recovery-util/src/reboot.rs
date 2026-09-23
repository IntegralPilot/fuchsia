// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use async_trait::async_trait;
use fidl_fuchsia_hardware_power_statecontrol as powercontrol;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
#[cfg(test)]
use mockall::automock;
use zx::{MonotonicDuration, Status as zx_status};

#[cfg_attr(test, automock)]
#[async_trait(?Send)]
pub trait RebootHandler {
    /// Request a reboot with optional delay in seconds. This is currently not cancellable and does not return an error result.
    /// The caller will be responsible for handling which thread to schedule this request on.
    async fn reboot(&self, delay_seconds: Option<u64>) -> Result<(), Error>;
}

#[derive(Default)]
pub struct RebootImpl;

impl RebootImpl {
    async fn request_reboot_with_proxy(
        &self,
        delay_seconds: Option<u64>,
        proxy: powercontrol::AdminProxy,
    ) -> Result<(), Error> {
        println!("Rebooting after {:?} seconds...", delay_seconds.unwrap_or(0));

        if let Some(delay) = delay_seconds {
            fasync::Timer::new(fasync::MonotonicInstant::after(MonotonicDuration::from_seconds(
                delay.try_into()?,
            )))
            .await;
        }

        // TODO(b/239569913): Update with a recovery-specific reboot reason.
        proxy
            .shutdown(&powercontrol::ShutdownOptions {
                action: Some(powercontrol::ShutdownAction::Reboot),
                reasons: Some(vec![powercontrol::ShutdownReason::FactoryDataReset]),
                ..Default::default()
            })
            .await?
            .map_err(zx_status::err_from_raw)?;
        Ok(())
    }
}

#[async_trait(?Send)]
impl RebootHandler for RebootImpl {
    async fn reboot(&self, delay_seconds: Option<u64>) -> Result<(), Error> {
        let proxy = connect_to_protocol::<powercontrol::AdminMarker>()?;
        self.request_reboot_with_proxy(delay_seconds, proxy).await
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fidl_fuchsia_hardware_power_statecontrol as powercontrol;
    use fidl_fuchsia_hardware_power_statecontrol::{
        ShutdownAction, ShutdownOptions, ShutdownReason,
    };
    use fuchsia_async as fasync;
    use fuchsia_async::TimeoutExt;
    use futures::channel::mpsc;
    use futures::{StreamExt, TryStreamExt};

    // Reboot tests - this functionality is only exercised in recovery OTA flows.
    fn create_mock_powercontrol_server()
    -> Result<(powercontrol::AdminProxy, mpsc::Receiver<powercontrol::ShutdownOptions>), Error>
    {
        let (mut sender, receiver) = mpsc::channel(1);
        let (proxy, mut request_stream) =
            fidl::endpoints::create_proxy_and_stream::<powercontrol::AdminMarker>();

        fasync::Task::local(async move {
            while let Some(request) =
                request_stream.try_next().await.expect("failed to read mock request")
            {
                match request {
                    powercontrol::AdminRequest::Shutdown { options, responder } => {
                        sender.start_send(options).unwrap();
                        let result: powercontrol::AdminShutdownResult = { Ok(()) };
                        responder.send(result).ok();
                    }
                    _ => {
                        panic!("Mock server not configured to handle request");
                    }
                }
            }
        })
        .detach();

        Ok((proxy, receiver))
    }

    #[fuchsia::test]
    async fn test_reboot_reason_no_delay() {
        let (proxy, mut receiver) = create_mock_powercontrol_server().unwrap();

        let reboot = RebootImpl::default();
        reboot.request_reboot_with_proxy(None, proxy).await.unwrap();

        let options =
            receiver.next().on_timeout(MonotonicDuration::from_seconds(5), || None).await.unwrap();

        assert_eq!(
            options,
            ShutdownOptions {
                action: Some(ShutdownAction::Reboot),
                reasons: Some(vec![ShutdownReason::FactoryDataReset]),
                ..Default::default()
            }
        );
    }

    #[fuchsia::test]
    async fn test_reboot_with_delay() {
        let delay_seconds = 1;
        let (proxy, mut receiver) = create_mock_powercontrol_server().unwrap();

        let start_time = fasync::MonotonicInstant::now();
        let reboot = RebootImpl::default();
        reboot.request_reboot_with_proxy(Some(delay_seconds), proxy).await.unwrap();

        let options =
            receiver.next().on_timeout(MonotonicDuration::from_seconds(5), || None).await.unwrap();

        let end_time = fasync::MonotonicInstant::now();

        assert!((end_time - start_time).into_seconds() >= delay_seconds.try_into().unwrap());
        assert_eq!(
            options,
            ShutdownOptions {
                action: Some(ShutdownAction::Reboot),
                reasons: Some(vec![ShutdownReason::FactoryDataReset]),
                ..Default::default()
            }
        );
    }
}
