// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::crash::CrashReporter;
use crate::ota::actions::factory_reset::FactoryResetAction;
use crate::ota::actions::finalize_reinstall::FinalizeReinstallAction;
use crate::ota::actions::get_wifi_networks::GetWifiNetworksAction;
use crate::ota::actions::ota_reinstall::OtaReinstallAction;
use crate::ota::actions::reboot_device::RebootAction;
use crate::ota::actions::set_sharing_consent::SetSharingConsentAction;
use crate::ota::actions::wifi_connect::WifiConnectAction;
use crate::ota::controller::EventSender;
use crate::ota::state_machine::{State, StateHandler};
use fuchsia_async as fasync;
use ota_lib::OtaManager;
use std::rc::Rc;
use std::sync::Arc;

/// This file initiates all the non-ui, background actions that are required to satisfy
/// the OTA UX UI slides. In general some states cause a background task to be run which
/// may  produce one or more state machine events. Actions may reboot the device.
pub struct Action {
    event_sender: EventSender,
    ota_manager: Arc<dyn OtaManager>,
    crash_reporter: Option<Rc<CrashReporter>>,
}

impl Action {
    pub fn new(
        event_sender: EventSender,
        ota_manager: Arc<dyn OtaManager>,
        crash_reporter: Option<Rc<CrashReporter>>,
    ) -> Self {
        Self { event_sender, ota_manager, crash_reporter }
    }
}

impl StateHandler for Action {
    fn handle_state(&mut self, state: State) {
        let event_sender = Box::new(self.event_sender.clone());
        match state {
            State::GetWiFiNetworks => GetWifiNetworksAction::run(event_sender),
            State::Connecting(network, password) => {
                WifiConnectAction::run(event_sender, network, password, self.crash_reporter.clone())
            }
            State::Rebooting(delay) => RebootAction::run(event_sender, delay),
            State::ReinstallConfirm { desired: user_data_sharing_consent, reported } => {
                SetSharingConsentAction::run(event_sender, user_data_sharing_consent, reported)
            }
            State::ExecuteReinstall => {
                OtaReinstallAction::run(event_sender, self.ota_manager.clone())
            }
            State::ReinstallRunning { status: Some(status), .. } => {
                // TODO(b/253084947): Remove call to tell ota_manager the OTA is complete
                let ota_manager = self.ota_manager.clone();
                fasync::Task::local(async move {
                    ota_manager.complete_ota(status).await;
                })
                .detach();
            }
            State::FinalizeReinstall(status) => FinalizeReinstallAction::run(event_sender, status),
            State::FactoryReset => FactoryResetAction::run(event_sender),
            // We ignore all other states
            _ => {}
        };
    }
}

#[cfg(test)]
mod test {
    use super::{Action, StateHandler};
    use crate::ota::controller::EventSender;
    use crate::ota::state_machine::{Event, OtaStatus, State};
    use anyhow::Error;
    use async_trait::async_trait;
    use fuchsia_sync::Mutex;
    use futures::channel::{mpsc, oneshot};
    use ota_lib::OtaManager;
    use std::sync::Arc;

    struct FakeOtaManager {
        sender: Mutex<Option<oneshot::Sender<OtaStatus>>>,
    }

    impl FakeOtaManager {
        pub fn new() -> (Arc<Self>, oneshot::Receiver<OtaStatus>) {
            let (tx, rx) = oneshot::channel();
            (Arc::new(Self { sender: Mutex::new(Some(tx)) }), rx)
        }
    }

    #[async_trait]
    impl OtaManager for FakeOtaManager {
        async fn start_and_wait_for_result(&self) -> Result<(), Error> {
            Ok(())
        }
        async fn stop(&self) -> Result<(), Error> {
            Ok(())
        }
        async fn complete_ota(&self, status: OtaStatus) {
            self.sender.lock().take().expect("only one event expected").send(status).unwrap();
        }
    }

    #[fuchsia::test]
    async fn reinstall_progress_with_ota_status_completes_ota() {
        let (sender, _receiver) = mpsc::channel::<Event>(10);
        let event_sender = EventSender::new(sender);

        // Verify that status propagates to complete_ota by trying a few different values.
        for status in [OtaStatus::Succeeded, OtaStatus::Failed, OtaStatus::Cancelled] {
            let (ota_manager, on_complete_ota) = FakeOtaManager::new();
            let mut action = Action::new(event_sender.clone(), ota_manager, None);

            action.handle_state(State::ReinstallRunning {
                status: Some(status.clone()),
                progress: 100,
            });
            assert_eq!(status, on_complete_ota.await.unwrap());
        }
    }
}
