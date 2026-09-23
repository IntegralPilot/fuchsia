// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ota::controller::SendEvent;
use crate::ota::state_machine::Event;
use fidl_fuchsia_recovery::{FactoryResetMarker, FactoryResetProxy};
use fuchsia_async::{self as fasync};
use fuchsia_component::client::connect_to_protocol;
use zx::sys::ZX_OK;
use zx_status::{self as zx_status};

/// Asynchronously performs a factory reset.
/// It reboots almost instantly
pub struct FactoryResetAction {}

impl FactoryResetAction {
    pub fn run(event_sender: Box<dyn SendEvent>) {
        let proxy = connect_to_protocol::<FactoryResetMarker>().unwrap();
        let task = Self::run_with_proxy(event_sender, proxy);
        // TODO(b/261822338): Actions should yield more control over their tasks
        fasync::Task::local(task).detach();
    }

    async fn run_with_proxy(mut event_sender: Box<dyn SendEvent>, proxy: FactoryResetProxy) {
        // Wait 2 seconds so the UI transition is less jarring
        fasync::Timer::new(fasync::MonotonicDuration::from_seconds(2)).await;
        println!("recovery: Executing factory reset command");
        let result = proxy.reset().await;
        match result {
            Ok(status) if status != ZX_OK => {
                event_sender.send(Event::Error(format!(
                    "Factory Reset failed: {:?}",
                    zx_status::Status::err_from_raw(status)
                )));
            }
            Ok(_) => { /* ignore success */ }
            Err(error) => {
                event_sender.send(Event::Error(format!("Factory Reset failed: {:?}", error)));
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::ota::controller::MockSendEvent;
    use anyhow::Error;
    use fidl_fuchsia_recovery::FactoryResetRequest;
    use fuchsia_async::{
        TimeoutExt, {self as fasync},
    };
    use futures::channel::mpsc;
    use futures::{StreamExt, TryStreamExt};
    use mockall::predicate::{self, eq};
    use zx::MonotonicDuration;

    const RESET_CALLED: i32 = 123456;

    // For future reference this test structure comes from
    // fxr/753732/4/src/recovery/lib/recovery-util/src/reboot.rs#49
    fn create_mock_factory_reset_server(
        return_status: i32,
    ) -> Result<(FactoryResetProxy, mpsc::Receiver<i32>), Error> {
        let (mut sender, receiver) = mpsc::channel(1);
        let (proxy, mut request_stream) =
            fidl::endpoints::create_proxy_and_stream::<FactoryResetMarker>();

        fasync::Task::local(async move {
            while let Some(request) =
                request_stream.try_next().await.expect("failed to read mock request")
            {
                match request {
                    FactoryResetRequest::Reset { responder } => {
                        sender.start_send(RESET_CALLED).unwrap();
                        responder.send(return_status).ok();
                    }
                }
            }
        })
        .detach();
        Ok((proxy, receiver))
    }

    #[fuchsia::test]
    async fn test_reset_called() {
        let mut event_sender = MockSendEvent::new();
        event_sender.expect_send().with(eq(Event::Cancel)).times(0).return_const(());
        let event_sender: Box<dyn SendEvent> = Box::new(event_sender);

        let (proxy, mut receiver) = create_mock_factory_reset_server(ZX_OK).unwrap();
        FactoryResetAction::run_with_proxy(event_sender, proxy).await;
        let status =
            receiver.next().on_timeout(MonotonicDuration::from_seconds(5), || None).await.unwrap();
        assert_eq!(status, RESET_CALLED);
    }

    #[fuchsia::test]
    async fn test_reset_error() {
        let mut event_sender = MockSendEvent::new();
        event_sender.expect_send().with(eq(Event::Cancel)).times(0).return_const(());
        let error_check = predicate::function(|e: &Event| matches!(e, Event::Error(_)));
        event_sender.expect_send().with(error_check).once().return_const(());
        let event_sender: Box<dyn SendEvent> = Box::new(event_sender);

        let (proxy, mut receiver) =
            create_mock_factory_reset_server(zx_status::Status::ACCESS_DENIED.into_raw()).unwrap();
        FactoryResetAction::run_with_proxy(event_sender, proxy).await;
        let status =
            receiver.next().on_timeout(MonotonicDuration::from_seconds(5), || None).await.unwrap();
        assert_eq!(status, RESET_CALLED);
    }
}
