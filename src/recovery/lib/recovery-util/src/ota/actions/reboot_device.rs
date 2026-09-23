// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ota::controller::SendEvent;
use crate::ota::state_machine::Event;
use crate::reboot::{RebootHandler, RebootImpl};
use fuchsia_async::Task;

/// Asynchronously performs a reboot with an optional delay.
pub struct RebootAction {}

impl RebootAction {
    pub fn run(event_sender: Box<dyn SendEvent>, delay_seconds: Option<u64>) {
        Self::run_with_handler(event_sender, Box::new(RebootImpl::default()), delay_seconds);
    }

    fn run_with_handler(
        mut event_sender: Box<dyn SendEvent>,
        reboot_handler: Box<dyn RebootHandler>,
        delay_seconds: Option<u64>,
    ) {
        let task = async move {
            #[cfg(feature = "debug_logging")]
            println!("Rebooting...");
            if let Err(e) = reboot_handler.reboot(delay_seconds).await {
                event_sender.send(Event::Error(format!("Failed to reboot: {:?}", e)));
            } else {
                event_sender.send(Event::Rebooting);
            }
        };
        Task::local(task).detach();
    }
}

#[cfg(test)]
mod test {
    use super::RebootAction;
    use crate::ota::controller::{MockSendEvent, SendEvent};
    use crate::ota::state_machine::Event;
    use crate::reboot::RebootHandler;
    use anyhow::{format_err, Error};
    use async_trait::async_trait;
    use fuchsia_async as fasync;
    use futures::future;

    struct FakeRebootImpl {
        reboot_result: Result<(), Error>,
    }

    impl FakeRebootImpl {
        fn new(reboot_result: Result<(), Error>) -> Self {
            Self { reboot_result }
        }
    }

    #[async_trait(? Send)]
    impl RebootHandler for FakeRebootImpl {
        async fn reboot(&self, _delay_seconds: Option<u64>) -> Result<(), Error> {
            return match self.reboot_result {
                Ok(_) => Ok(()),
                _ => Err(Error::msg("Forced failure")),
            };
        }
    }

    #[fuchsia::test]
    fn test_reboot() {
        let mut exec = fasync::TestExecutor::new();
        let mut event_sender = MockSendEvent::new();
        event_sender
            .expect_send()
            .withf(move |event| if let Event::Rebooting = event { true } else { false })
            .times(1)
            .return_const(());
        let event_sender: Box<dyn SendEvent> = Box::new(event_sender);
        RebootAction::run_with_handler(event_sender, Box::new(FakeRebootImpl::new(Ok(()))), None);
        let _ = exec.run_until_stalled(&mut future::pending::<()>());
    }

    #[fuchsia::test]
    fn test_reboot_failure() {
        let mut exec = fasync::TestExecutor::new();
        let mut event_sender = MockSendEvent::new();
        event_sender
            .expect_send()
            .withf(move |event| if let Event::Error(_) = event { true } else { false })
            .times(1)
            .return_const(());
        let event_sender: Box<dyn SendEvent> = Box::new(event_sender);
        RebootAction::run_with_handler(
            event_sender,
            Box::new(FakeRebootImpl::new(Err(format_err!("forced")))),
            None,
        );
        let _ = exec.run_until_stalled(&mut future::pending::<()>());
    }
}
