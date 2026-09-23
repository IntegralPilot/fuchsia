// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ota::controller::SendEvent;
use crate::ota::state_machine::DataSharingConsent::{Allow, DontAllow, Unknown};
use crate::ota::state_machine::{DataSharingConsent, Event};
use fidl_fuchsia_settings::{PrivacyMarker, PrivacyProxy, PrivacySettings};
use fuchsia_async::Task;
use fuchsia_component::client::connect_to_protocol;

pub struct SetSharingConsentAction {}

impl SetSharingConsentAction {
    pub fn run(
        event_sender: Box<dyn SendEvent>,
        desired: DataSharingConsent,
        reported: DataSharingConsent,
    ) {
        let proxy = connect_to_protocol::<PrivacyMarker>().unwrap();
        Self::run_with_proxy(event_sender, desired, reported, proxy)
    }

    fn run_with_proxy(
        mut event_sender: Box<dyn SendEvent>,
        desired: DataSharingConsent,
        reported: DataSharingConsent,
        proxy: PrivacyProxy,
    ) {
        // We only need to update if the desired state is known and different
        if desired == reported || desired == Unknown {
            return;
        }
        let task = async move {
            let mut privacy_settings = PrivacySettings::default();
            privacy_settings.user_data_sharing_consent = Some(desired == Allow);
            #[cfg(feature = "debug_logging")]
            println!("Setting privacy to {:?}", privacy_settings);
            let res = proxy.set(&privacy_settings).await;
            #[cfg(feature = "debug_logging")]
            println!("Privacy response is {:?}", res);
            match res {
                Ok(Err(error)) => {
                    // Errors come back inside an Ok!
                    eprintln!("Set privacy returned an internal error: {:?}", error);
                    event_sender.send(Event::SystemPrivacySetting(DontAllow));
                }
                Ok(Ok(())) => {
                    //Ok's come back inside an Ok!
                    event_sender.send(Event::SystemPrivacySetting(desired));
                }
                Err(error) => {
                    eprintln!("Set privacy returned a FIDL error: {:?}", error);
                    event_sender.send(Event::SystemPrivacySetting(DontAllow));
                }
            };
        };
        Task::local(task).detach();
    }
}

#[cfg(test)]
mod test {
    use super::SetSharingConsentAction;
    use crate::ota::controller::{MockSendEvent, SendEvent};
    use crate::ota::state_machine::DataSharingConsent::{Allow, DontAllow, Unknown};
    use crate::ota::state_machine::Event;
    use anyhow::Error;
    use fidl::endpoints::{ControlHandle, Responder};
    use fidl_fuchsia_settings::{
        Error as PrivacyError, PrivacyMarker, PrivacyProxy, PrivacyRequest,
    };
    use fuchsia_async as fasync;
    use futures::{future, TryStreamExt};
    use mockall::predicate::eq;

    // For future reference this test structure comes from
    // fxr/753732/4/src/recovery/lib/recovery-util/src/reboot.rs#49
    fn create_mock_privacy_server(will_succeed: Option<bool>) -> Result<PrivacyProxy, Error> {
        let (proxy, mut request_stream) =
            fidl::endpoints::create_proxy_and_stream::<PrivacyMarker>();
        fasync::Task::local(async move {
            while let Some(request) =
                request_stream.try_next().await.expect("failed to read mock request")
            {
                match request {
                    PrivacyRequest::Set { responder, settings: _ } => match will_succeed {
                        Some(will_succeed) => {
                            let response =
                                if will_succeed { Ok(()) } else { Err(PrivacyError::Failed) };
                            responder.send(response).expect("Should not fail");
                        }
                        // We haven't been told to succeed or fail so cause a FIDL error
                        None => responder.control_handle().shutdown(),
                    },
                    _ => {}
                }
            }
        })
        .detach();
        Ok(proxy)
    }

    #[fuchsia::test]
    fn test_privacy_set_true() {
        let mut exec = fasync::TestExecutor::new();
        let mut event_sender = MockSendEvent::new();
        event_sender
            .expect_send()
            .withf(move |event| {
                if let Event::SystemPrivacySetting(desired) = event {
                    *desired == Allow
                } else {
                    false
                }
            })
            .times(1)
            .return_const(());
        let event_sender: Box<dyn SendEvent> = Box::new(event_sender);
        let proxy = create_mock_privacy_server(Some(true)).unwrap();
        let desired = Allow;
        let reported = Unknown;
        SetSharingConsentAction::run_with_proxy(event_sender, desired, reported, proxy);
        let _ = exec.run_until_stalled(&mut future::pending::<()>());
    }

    #[fuchsia::test]
    fn test_privacy_set_false() {
        let mut exec = fasync::TestExecutor::new();
        let mut event_sender = MockSendEvent::new();
        event_sender
            .expect_send()
            .withf(move |event| {
                if let Event::SystemPrivacySetting(desired) = event {
                    *desired == DontAllow
                } else {
                    false
                }
            })
            .times(1)
            .return_const(());
        let event_sender: Box<dyn SendEvent> = Box::new(event_sender);
        let proxy = create_mock_privacy_server(Some(true)).unwrap();
        let desired = DontAllow;
        let reported = Unknown;
        SetSharingConsentAction::run_with_proxy(event_sender, desired, reported, proxy);
        let _ = exec.run_until_stalled(&mut future::pending::<()>());
    }

    #[fuchsia::test]
    fn test_privacy_set_error() {
        let mut exec = fasync::TestExecutor::new();
        let mut event_sender = MockSendEvent::new();
        event_sender
            .expect_send()
            .with(eq(Event::SystemPrivacySetting(DontAllow)))
            .times(1)
            .return_const(());
        let event_sender: Box<dyn SendEvent> = Box::new(event_sender);
        let proxy = create_mock_privacy_server(Some(false)).unwrap();
        let desired = Allow;
        let reported = Unknown;
        SetSharingConsentAction::run_with_proxy(event_sender, desired, reported, proxy);
        let _ = exec.run_until_stalled(&mut future::pending::<()>());
    }

    #[fuchsia::test]
    fn test_privacy_fidl_error() {
        let mut exec = fasync::TestExecutor::new();
        let mut event_sender = MockSendEvent::new();
        event_sender
            .expect_send()
            .with(eq(Event::SystemPrivacySetting(DontAllow)))
            .times(1)
            .return_const(());
        let event_sender: Box<dyn SendEvent> = Box::new(event_sender);
        let proxy = create_mock_privacy_server(None).unwrap();
        let desired = Allow;
        let reported = Unknown;
        SetSharingConsentAction::run_with_proxy(event_sender, desired, reported, proxy);
        let _ = exec.run_until_stalled(&mut future::pending::<()>());
    }
}
