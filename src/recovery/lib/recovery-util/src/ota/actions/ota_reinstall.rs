// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ota::controller::SendEvent;
use crate::ota::state_machine::Event;
use fuchsia_async::Task;
use ota_lib::{OtaManager, OtaStatus};
use std::sync::Arc;

/// Starts an OTA reinstall
pub struct OtaReinstallAction {}

impl OtaReinstallAction {
    /// Runs the OTA reinstall process in a background Task.
    ///
    /// The device is expected to have a network connection. If there is no network connection, the
    /// OTA will fail and the Task will send Event::Error.
    ///
    /// The caller is expected to make calls to OtaManager to complete the OTA process. Afterwards,
    /// the OTA task created by this method will send either Event::Progress(100) or Event:Error.
    pub fn run(event_sender: Box<dyn SendEvent>, ota_manager: Arc<dyn OtaManager>) {
        // This is split into two parts for testing, allowing a factory to be injected and to
        // allow the test to await completion of the task.
        Task::local(run_ota(event_sender, ota_manager)).detach();
    }
}

async fn run_ota(mut event_sender: Box<dyn SendEvent>, ota_manager: Arc<dyn OtaManager>) {
    event_sender.send(Event::Reinstall); // Might need to document this or fix naming to be explicit
    let res = ota_manager.start_and_wait_for_result().await;

    let event = match res {
        Ok(_) => {
            println!("OtaReinstallAction: OTA Success!");
            // TODO(b/253084947) start_and_wait_for_result should return actual OTA result
            // Currently returns OK(()) or Err(..)
            Event::OtaStatusReceived(OtaStatus::Succeeded)
        }
        Err(e) => {
            println!("OTA Error..... {:?}", e);
            Event::Error(e.to_string())
        }
    };

    event_sender.send(event);
}

#[cfg(test)]
mod tests {
    use super::OtaReinstallAction;
    use crate::ota::controller::{MockSendEvent, SendEvent};
    use crate::ota::state_machine::{Event, OtaStatus};
    use anyhow::{Error, format_err};
    use async_trait::async_trait;
    use futures::StreamExt;
    use futures::channel::mpsc;
    use ota_lib::OtaManager;
    use std::sync::Arc;

    struct FakeOtaManager {
        ota_fails: bool,
    }

    impl FakeOtaManager {
        pub fn new() -> Self {
            Self { ota_fails: false }
        }
        pub fn new_failing() -> Self {
            Self { ota_fails: true }
        }
    }

    #[async_trait]
    impl OtaManager for FakeOtaManager {
        async fn start_and_wait_for_result(&self) -> Result<(), Error> {
            if self.ota_fails { Err(format_err!("ota failed")) } else { Ok(()) }
        }
        async fn stop(&self) -> Result<(), Error> {
            Ok(())
        }
        async fn complete_ota(&self, _status: OtaStatus) {}
    }

    #[fuchsia::test]
    async fn ota_reinstall_sends_status_event_on_successful_ota() {
        let (mut tx, mut on_handle_event) = mpsc::channel::<Event>(10);
        let mut event_sender = MockSendEvent::new();
        event_sender.expect_send().times(2).returning(move |event| {
            tx.try_send(event).unwrap();
        });
        let event_sender: Box<dyn SendEvent> = Box::new(event_sender);

        let ota_manager = Arc::new(FakeOtaManager::new());
        OtaReinstallAction::run(event_sender, ota_manager);
        assert_eq!(Event::Reinstall, on_handle_event.next().await.unwrap());
        assert_eq!(
            Event::OtaStatusReceived(OtaStatus::Succeeded),
            on_handle_event.next().await.unwrap()
        );
    }

    #[fuchsia::test]
    async fn ota_reinstall_sends_error_event_on_failed_ota() {
        let (mut tx, mut on_handle_event) = mpsc::channel::<Event>(10);
        let mut event_sender = MockSendEvent::new();
        event_sender.expect_send().times(2).returning(move |event| {
            tx.try_send(event).unwrap();
        });
        let event_sender: Box<dyn SendEvent> = Box::new(event_sender);

        let ota_manager = Arc::new(FakeOtaManager::new_failing());
        OtaReinstallAction::run(event_sender, ota_manager);
        assert_eq!(Event::Reinstall, on_handle_event.next().await.unwrap());
        assert_eq!(Event::Error("ota failed".to_string()), on_handle_event.next().await.unwrap());
    }
}
