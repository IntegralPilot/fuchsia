// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ota::controller::SendEvent;
use crate::ota::state_machine::Event;
use crate::reboot::{RebootHandler, RebootImpl};
use fuchsia_async as fasync;
use ota_lib::OtaStatus;

pub struct FinalizeReinstallAction {}

impl FinalizeReinstallAction {
    pub fn run(event_sender: Box<dyn SendEvent>, status: OtaStatus) {
        let reboot_handler = Box::new(RebootImpl::default());
        let task = Self::run_with_proxies(event_sender, status, reboot_handler);
        // TODO(b/261822338): Actions should yield more control over their tasks
        fasync::Task::local(task).detach();
    }

    async fn run_with_proxies(
        mut event_sender: Box<dyn SendEvent>,
        status: OtaStatus,

        reboot_handler: Box<dyn RebootHandler>,
    ) {
        match status {
            OtaStatus::Succeeded => {
                if let Err(e) = reboot_handler.reboot(None).await {
                    event_sender.send(Event::Error(format!("Failed to reboot: {:?}", e)));
                    eprintln!("Failed to reboot: {:?}", e);
                }
            }
            OtaStatus::Failed | OtaStatus::Cancelled => {
                event_sender.send(Event::Error("OTA failed or cancelled".to_string()));
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::ota::controller::MockSendEvent;
    use crate::reboot::MockRebootHandler;
    use anyhow::bail;
    use mockall::predicate;

    fn create_mocks() -> (Box<MockSendEvent>, Box<MockRebootHandler>) {
        let event_sender = Box::new(MockSendEvent::new());
        let reboot_handler = Box::new(MockRebootHandler::new());

        (event_sender, reboot_handler)
    }

    #[fuchsia::test]
    async fn reboot_is_successful() {
        let (mut event_sender, mut reboot_handler) = create_mocks();

        event_sender.expect_send().never().return_const(());
        reboot_handler.expect_reboot().once().return_once(move |_| Ok(()));

        FinalizeReinstallAction::run_with_proxies(
            event_sender,
            OtaStatus::Succeeded,
            reboot_handler,
        )
        .await;
    }

    #[fuchsia::test]
    async fn failed_reboot_sends_error() {
        let (mut event_sender, mut reboot_handler) = create_mocks();

        let error_predicate = predicate::function(|e: &Event| matches!(e, Event::Error(_)));
        event_sender.expect_send().with(error_predicate).once().return_const(());
        reboot_handler.expect_reboot().once().return_once(move |_| bail!("ignored error string"));

        FinalizeReinstallAction::run_with_proxies(
            event_sender,
            OtaStatus::Succeeded,
            reboot_handler,
        )
        .await;
    }

    #[fuchsia::test]
    async fn failed_ota_sends_error() {
        let (mut event_sender, mut reboot_handler) = create_mocks();

        let error_predicate = predicate::function(|e: &Event| matches!(e, Event::Error(_)));
        event_sender.expect_send().with(error_predicate).once().return_const(());
        reboot_handler.expect_reboot().never();

        FinalizeReinstallAction::run_with_proxies(event_sender, OtaStatus::Failed, reboot_handler)
            .await;
    }

    #[fuchsia::test]
    async fn cancelled_ota_sends_error() {
        let (mut event_sender, mut reboot_handler) = create_mocks();

        let error_predicate = predicate::function(|e: &Event| matches!(e, Event::Error(_)));
        event_sender.expect_send().with(error_predicate).once().return_const(());
        reboot_handler.expect_reboot().never();

        FinalizeReinstallAction::run_with_proxies(
            event_sender,
            OtaStatus::Cancelled,
            reboot_handler,
        )
        .await;
    }
}
