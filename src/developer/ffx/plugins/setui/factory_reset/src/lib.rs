// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use fdomain_fuchsia_settings::{FactoryResetProxy, FactoryResetSettings};
use ffx_setui_factory_reset_args::FactoryReset;
use ffx_writer::SimpleWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use target_holders::moniker;
use utils::{Either, WatchOrSetResult, handle_mixed_result};

#[derive(FfxTool)]
#[check(AvailabilityFlag("setui"))]
pub struct FactoryResetTool {
    #[command]
    cmd: FactoryReset,
    #[with(moniker("/core/setui_service"))]
    factory_reset_proxy: FactoryResetProxy,
}

fho::embedded_plugin!(FactoryResetTool);

#[async_trait(?Send)]
impl FfxMain for FactoryResetTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        run_command(self.factory_reset_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

async fn run_command<W: std::io::Write>(
    factory_reset_proxy: FactoryResetProxy,
    factory_reset: FactoryReset,
    w: &mut W,
) -> Result<()> {
    handle_mixed_result(
        "FactoryReset",
        command(factory_reset_proxy, factory_reset.is_local_reset_allowed).await,
        w,
    )
    .await
}

async fn command(
    proxy: FactoryResetProxy,
    is_local_reset_allowed: Option<bool>,
) -> WatchOrSetResult {
    let mut settings = FactoryResetSettings::default();
    settings.is_local_reset_allowed = is_local_reset_allowed;

    if settings == FactoryResetSettings::default() {
        Ok(Either::Watch(utils::watch_to_stream(proxy, |p| p.watch())))
    } else {
        Ok(Either::Set(if let Err(err) = proxy.set(&settings).await? {
            format!("{:?}", err)
        } else {
            format!("Successfully set factory_reset to {:?}", settings)
        }))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_settings::FactoryResetRequest;
    use target_holders::fake_proxy;
    use test_case::test_case;

    #[fuchsia::test]
    async fn test_run_command() {
        const ALLOWED: bool = true;

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            FactoryResetRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            FactoryResetRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let factory_reset = FactoryReset { is_local_reset_allowed: Some(ALLOWED) };
        let response = run_command(proxy, factory_reset, &mut vec![]).await;
        assert!(response.is_ok());
    }

    #[test_case(
        true;
        "Test factory reset set() output with is_local_reset_allowed as true."
    )]
    #[test_case(
        false;
        "Test factory reset set() output with is_local_reset_allowed as false."
    )]
    #[fuchsia::test]
    async fn validate_factory_reset_set_output(
        expected_is_local_reset_allowed: bool,
    ) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            FactoryResetRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            FactoryResetRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let output = utils::assert_set!(command(proxy, Some(expected_is_local_reset_allowed)));
        assert_eq!(
            output,
            format!(
                "Successfully set factory_reset to {:?}",
                FactoryResetSettings {
                    is_local_reset_allowed: Some(expected_is_local_reset_allowed),
                    ..Default::default()
                }
            )
        );
        Ok(())
    }

    #[test_case(
        None;
        "Test factory reset watch() output with is_local_reset_allowed as None."
    )]
    #[test_case(
        Some(false);
        "Test factory reset watch() output with is_local_reset_allowed as false."
    )]
    #[test_case(
        Some(true);
        "Test factory reset watch() output with is_local_reset_allowed as true."
    )]
    #[fuchsia::test]
    async fn validate_factory_reset_watch_output(
        expected_is_local_reset_allowed: Option<bool>,
    ) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            FactoryResetRequest::Set { .. } => {
                panic!("Unexpected call to set");
            }
            FactoryResetRequest::Watch { responder } => {
                let _ = responder.send(&FactoryResetSettings {
                    is_local_reset_allowed: expected_is_local_reset_allowed,
                    ..Default::default()
                });
            }
        });

        let output = utils::assert_watch!(command(proxy, None));
        assert_eq!(
            output,
            format!(
                "{:#?}",
                FactoryResetSettings {
                    is_local_reset_allowed: expected_is_local_reset_allowed,
                    ..Default::default()
                }
            )
        );
        Ok(())
    }
}
