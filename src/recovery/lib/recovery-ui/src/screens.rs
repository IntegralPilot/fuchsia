// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::check_network::CheckNetworkViewAssistant;
#[allow(unused)]
use crate::constants::constants::{
    ScreenSplit, ICON_FACTORY_RESET, ICON_REINSTALL_SOFTWARE, IMAGE_DEFAULT_SIZE,
    IMAGE_DEVICE_CONNECT, IMAGE_DEVICE_INSTALL, IMAGE_DEVICE_UPDATING, IMAGE_DEVICE_UPDATING_SIZE,
    IMAGE_PHONE_HOME_APP, IMAGE_PHONE_HOME_APP_SIZE, IMAGE_RESET_FAILED,
};
use crate::font;
use crate::generic_view::{ButtonInfo, GenericSplitViewAssistant};
use crate::keyboard::KeyboardViewAssistant;
use crate::network::NetworkViewAssistant;
use crate::proxy_view_assistant::ProxyMessages;
use crate::text_field::TextVisibility;
use carnelian::{make_message, AppSender, MessageTarget, ViewAssistantPtr, ViewKey};
use recovery_util::ota::state_machine::DataSharingConsent::Allow;
use recovery_util::ota::state_machine::{
    DataSharingConsent, Event, Operation, State, StateHandler,
};
use recovery_util::wlan::NetworkInfo;

pub struct Screens {
    app_sender: AppSender,
    view_key: ViewKey,
}

impl Screens {
    pub fn new(app_sender: AppSender, view_key: ViewKey) -> Self {
        Self { app_sender, view_key }
    }

    pub fn initial_screen(&self) -> ViewAssistantPtr {
        self.home()
    }

    fn state(&mut self, state: &State) {
        let new_view = match state {
            State::Home => self.home(),
            State::FactoryReset => self.progress(Operation::FactoryDataReset, 0),
            State::FactoryResetConfirm => self.factory_reset_confirm(),
            State::Failed(operation, error) => self.failed(operation, error),
            State::Reinstall => self.reinstall(),
            State::Rebooting(_) => self.rebooting(),
            State::GetWiFiNetworks => self.select_wifi(&Vec::new()),
            State::SelectWiFi(networks) => self.select_wifi(networks),
            State::EnterWiFi => {
                self.user_entry("Enter Network Name".to_string(), TextVisibility::Always)
            }
            State::EnterPassword(network) => self.user_entry(
                format!("Enter Password for {} ", network),
                TextVisibility::Toggleable(false),
            ),
            State::Connecting(network, password) => self.connecting(network, password),
            State::ConnectionFailed(network, password) => self.connection_failed(network, password),
            State::ReinstallConfirm { desired: user_data_sharing_consent, reported } => {
                self.reinstall_confirm(user_data_sharing_consent.clone(), reported.clone())
            }
            State::ExecuteReinstall => self.progress(Operation::Reinstall, 0),
            State::ReinstallRunning { progress, .. } => {
                // TODO(b/245415603): report actual progress values
                // Clamping progress to between 0-93%. The progress reports currently arrive as fake timed events
                // A device staying at 93% indicates OTA is still running
                let mapped_progess: f32 = (*progress as f32) * 0.93;
                self.progress(Operation::Reinstall, mapped_progess as u8)
            }
            State::FinalizeReinstall(_) => {
                // TODO(b/258285426): Evaluate UI impact of uploading metrics
                // Marking progress as 95 percent to indicate metrics uploading and reboot is in progress.
                self.progress(Operation::Reinstall, 95)
            }
        };
        self.app_sender.queue_message(
            MessageTarget::View(self.view_key),
            make_message(ProxyMessages::ReplaceViewAssistant(Some(new_view))),
        );
    }

    fn factory_reset_confirm(&self) -> ViewAssistantPtr {
        let view_assistant_ptr = Box::new(
            GenericSplitViewAssistant::new(
                self.app_sender.clone(),
                self.view_key,
                ScreenSplit::None,
                Some("Confirm factory reset".to_string()),
                Some(
                    "• This will clear your data from the device and can't be undone\n\
                     • Upon completion, the device will automatically restart"
                        .to_string(),
                ),
                None,
                Some(vec![
                    ButtonInfo::new("Cancel", None, false, true, Event::Cancel),
                    ButtonInfo::new(
                        "Start Factory Reset",
                        Some(ICON_FACTORY_RESET),
                        false,
                        false,
                        Event::StartFactoryReset,
                    ),
                ]),
                None,
                None,
                Some(IMAGE_DEVICE_UPDATING),
                Some(IMAGE_DEVICE_UPDATING_SIZE),
            )
            .unwrap(),
        );
        view_assistant_ptr
    }

    fn progress(&self, operation: Operation, percent: u8) -> ViewAssistantPtr {
        let title = match operation {
            // Factory resets are currently fast operations. If they take longer progress can be added here.
            Operation::FactoryDataReset => "Resetting".to_string(),
            Operation::Reinstall => format!("Updating {}%", percent),
        };
        let content = match operation {
            Operation::FactoryDataReset => "• Resetting user data\n\
            • Upon completion, the\n   \
              device will automatically\n   \
              restart"
                .to_string(),
            Operation::Reinstall => "• This may take several minutes\n\
            • Upon completion, the\n   \
              device will automatically\n   \
              restart"
                .to_string(),
        };
        let view_assistant_ptr = Box::new(
            GenericSplitViewAssistant::new(
                self.app_sender.clone(),
                self.view_key,
                ScreenSplit::Even,
                Some(title),
                Some(content),
                None,
                None,
                None,
                None,
                Some(IMAGE_DEVICE_UPDATING),
                Some(IMAGE_DEVICE_UPDATING_SIZE),
            )
            .unwrap(),
        );
        view_assistant_ptr
    }

    fn home(&self) -> ViewAssistantPtr {
        let view_assistant_ptr = Box::new(
            GenericSplitViewAssistant::new(
                self.app_sender.clone(),
                self.view_key,
                ScreenSplit::Wide,
                Some("Factory reset".to_string()),
                Some("• A factory reset can fix a device problem\n• This will clear your data from the device and can't be undone".to_string()),
                Some("Still having issues after factory reset?".to_string()),
                Some(vec![ButtonInfo::new("Start factory reset", Some(ICON_FACTORY_RESET), false, false, Event::StartFactoryReset)]),
                Some(vec![ButtonInfo::new("Try another option", None, true, true, Event::TryAnotherWay)]),
                None,
                None,
                None,
            )
                .unwrap(),
        );
        view_assistant_ptr
    }

    fn failed(&self, operation: &Operation, error: &Option<String>) -> ViewAssistantPtr {
        let text1 = match operation {
            Operation::FactoryDataReset => "Reset failed",
            Operation::Reinstall => "Recovery failed",
        };
        let error = if let Some(error) = error { error.clone() } else { "".to_string() };
        let text2 = match operation {
            Operation::FactoryDataReset => "An error occurred while\nrecovering".to_string(),
            Operation::Reinstall => error + "\n\nPlease reconnect and try again.",
        };
        let view_assistant_ptr = Box::new(
            GenericSplitViewAssistant::new(
                self.app_sender.clone(),
                self.view_key,
                ScreenSplit::Even,
                Some(text1.to_string()),
                Some(text2),
                None,
                Some(vec![
                    ButtonInfo::new("Cancel", None, false, true, Event::Cancel),
                    ButtonInfo::new("Try again", None, false, false, Event::TryAgain),
                ]),
                None,
                None,
                Some(IMAGE_RESET_FAILED),
                None,
            )
            .unwrap(),
        );
        view_assistant_ptr
    }

    fn rebooting(&self) -> ViewAssistantPtr {
        let view_assistant_ptr = Box::new(
            GenericSplitViewAssistant::new(
                self.app_sender.clone(),
                self.view_key,
                ScreenSplit::Even,
                Some("Rebooting...".to_string()),
                Some("Your device will now reboot.".to_string()),
                None,
                None,
                None,
                None,
                Some(IMAGE_DEVICE_UPDATING),
                None,
            )
            .unwrap(),
        );
        view_assistant_ptr
    }

    fn reinstall(&self) -> ViewAssistantPtr {
        let view_assistant_ptr = Box::new(
            GenericSplitViewAssistant::new(
                self.app_sender.clone(),
                self.view_key,
                ScreenSplit::Wide,
                Some("Reinstall Software".to_string()),
                Some(
                    "• Install the latest software to fix bugs and update the system\n\
                     • This will clear your data from the device and can't be undone\n\
                     • You'll need a Wi-Fi connection to proceed"
                        .to_string(),
                ),
                None,
                Some(vec![
                    ButtonInfo::new("Cancel", None, false, true, Event::Cancel),
                    ButtonInfo::new(
                        "Reinstall Software",
                        Some(ICON_REINSTALL_SOFTWARE),
                        false,
                        false,
                        Event::Reinstall,
                    ),
                ]),
                Some(vec![ButtonInfo::new("Reboot", None, true, true, Event::Reboot)]),
                None,
                None,
                None,
            )
            .unwrap(),
        );
        view_assistant_ptr
    }

    fn select_wifi(&self, networks: &Vec<NetworkInfo>) -> ViewAssistantPtr {
        let network_view = Box::new(
            NetworkViewAssistant::new(self.app_sender.clone(), self.view_key, networks.to_vec())
                .unwrap(),
        );
        network_view
    }

    fn user_entry(&self, text: String, privacy: TextVisibility) -> ViewAssistantPtr {
        let mut keyboard = Box::new(
            KeyboardViewAssistant::new(
                self.app_sender.clone(),
                self.view_key,
                font::get_default_font_face().clone(),
            )
            .unwrap(),
        );
        keyboard.set_field_name(text);
        keyboard.set_text_field(String::new());
        keyboard.set_privacy(privacy);
        keyboard
    }

    fn connecting(&self, _network: &String, _password: &String) -> ViewAssistantPtr {
        let view_assistant_ptr = Box::new(
            GenericSplitViewAssistant::new(
                self.app_sender.clone(),
                self.view_key,
                ScreenSplit::Even,
                Some("Connecting Wi-Fi".to_string()),
                Some("Sit tight. This may take a few\nseconds.".to_string()),
                None,
                Some(vec![ButtonInfo::new("Cancel", None, false, true, Event::Cancel)]),
                None,
                None,
                Some(IMAGE_DEVICE_CONNECT),
                Some(IMAGE_DEFAULT_SIZE),
            )
            .unwrap(),
        );
        view_assistant_ptr
    }

    fn connection_failed(&self, network: &String, password: &String) -> ViewAssistantPtr {
        let title_text = "Check Wi-Fi Network".to_string();
        let body_text = format!(
            "Incorrect password for Wi-Fi “{}”\n\
            Try again or choose a different Wi-Fi network.",
            network
        );
        let view_assistant_ptr = Box::new(
            CheckNetworkViewAssistant::new(
                self.app_sender.clone(),
                self.view_key,
                title_text,
                body_text,
                network.into(),
                password.into(),
                vec![
                    ButtonInfo::new("Cancel", None, false, true, Event::Cancel),
                    ButtonInfo::new("Choose network", None, false, true, Event::ChooseNetwork),
                    ButtonInfo::new("Try Again", None, false, false, Event::TryAgain),
                ],
            )
            .unwrap(),
        );
        view_assistant_ptr
    }

    fn reinstall_confirm(
        &self,
        _desired: DataSharingConsent,
        reported: DataSharingConsent,
    ) -> ViewAssistantPtr {
        let next_privacy_state = match reported {
            DataSharingConsent::Unknown => Allow,
            _ => reported.toggle(),
        };
        let view_assistant_ptr = Box::new(
            GenericSplitViewAssistant::new(
                self.app_sender.clone(),
                self.view_key,
                ScreenSplit::Even,
                Some("Reinstall Software".to_string()),
                // TODO(b/260539609 Reinstall timing hint text needs an update
                Some(
                    "• This will clear your data from the\n  \
                       device and can't be undone\n\
                     • It may take several minutes\n\
                     • Upon completion, the device will\n  \
                       automatically restart"
                        .to_string(),
                ),
                None,
                Some(vec![
                    ButtonInfo::new("Cancel", None, false, true, Event::Cancel),
                    ButtonInfo::new("Reinstall Now", None, false, false, Event::Reinstall),
                ]),
                None,
                Some(vec![ButtonInfo::new(
                    "Permission",
                    None,
                    false,
                    reported == Allow, // Unknown and DontAllow show metrics as disabled
                    Event::SendReports(next_privacy_state),
                )]),
                Some(IMAGE_DEVICE_INSTALL),
                Some(IMAGE_DEFAULT_SIZE),
            )
            .unwrap(),
        );
        view_assistant_ptr
    }
}

impl StateHandler for Screens {
    fn handle_state(&mut self, state: State) {
        self.state(&state);
    }
}
