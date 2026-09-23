// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use fdomain_fuchsia_settings::{PrivacyProxy, PrivacySettings};
use ffx_setui_privacy_args::Privacy;
use ffx_writer::SimpleWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use target_holders::moniker;
use utils::{Either, WatchOrSetResult, handle_mixed_result};

#[derive(FfxTool)]
#[check(AvailabilityFlag("setui"))]
pub struct PrivacyTool {
    #[command]
    cmd: Privacy,
    #[with(moniker("/core/setui_service"))]
    privacy_proxy: PrivacyProxy,
}

fho::embedded_plugin!(PrivacyTool);

#[async_trait(?Send)]
impl FfxMain for PrivacyTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        run_command(self.privacy_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

async fn run_command<W: std::io::Write>(
    privacy_proxy: PrivacyProxy,
    privacy: Privacy,
    writer: &mut W,
) -> Result<()> {
    handle_mixed_result(
        "Privacy",
        command(privacy_proxy, privacy.user_data_sharing_consent).await,
        writer,
    )
    .await
}

async fn command(proxy: PrivacyProxy, user_data_sharing_consent: Option<bool>) -> WatchOrSetResult {
    let mut settings = PrivacySettings::default();
    settings.user_data_sharing_consent = user_data_sharing_consent;

    if settings == PrivacySettings::default() {
        Ok(Either::Watch(utils::watch_to_stream(proxy, |p| p.watch())))
    } else {
        Ok(Either::Set(if let Err(err) = proxy.set(&settings).await? {
            format!("{:?}", err)
        } else {
            format!("Successfully set user_data_sharing_consent to {:?}", user_data_sharing_consent)
        }))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_settings::PrivacyRequest;
    use target_holders::fake_proxy;
    use test_case::test_case;

    #[fuchsia::test]
    async fn test_run_command() {
        const CONSENT: bool = true;

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            PrivacyRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            PrivacyRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let privacy = Privacy { user_data_sharing_consent: Some(CONSENT) };
        let response = run_command(proxy, privacy, &mut vec![]).await;
        assert!(response.is_ok());
    }

    #[test_case(
        true;
        "Test privacy set() output with user_data_sharing_consent as true."
    )]
    #[test_case(
        false;
        "Test privacy set() output with user_data_sharing_consent as false."
    )]
    #[fuchsia::test]
    async fn validate_privacy_set_output(expected_user_data_sharing_consent: bool) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            PrivacyRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            PrivacyRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let output = utils::assert_set!(command(proxy, Some(expected_user_data_sharing_consent)));
        assert_eq!(
            output,
            format!(
                "Successfully set user_data_sharing_consent to {:?}",
                Some(expected_user_data_sharing_consent)
            )
        );
        Ok(())
    }

    #[test_case(
        None;
        "Test privacy watch() output with user_data_sharing_consent as None."
    )]
    #[test_case(
        Some(false);
        "Test privacy watch() output with user_data_sharing_consent as false."
    )]
    #[test_case(
        Some(true);
        "Test privacy watch() output with user_data_sharing_consent as true."
    )]
    #[fuchsia::test]
    async fn validate_privacy_watch_output(
        expected_user_data_sharing_consent: Option<bool>,
    ) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            PrivacyRequest::Set { .. } => {
                panic!("Unexpected call to set");
            }
            PrivacyRequest::Watch { responder } => {
                let _ = responder.send(&PrivacySettings {
                    user_data_sharing_consent: expected_user_data_sharing_consent,
                    ..Default::default()
                });
            }
        });

        let output = utils::assert_watch!(command(proxy, None));
        assert_eq!(
            output,
            format!(
                "{:#?}",
                PrivacySettings {
                    user_data_sharing_consent: expected_user_data_sharing_consent,
                    ..Default::default()
                }
            )
        );
        Ok(())
    }
}
