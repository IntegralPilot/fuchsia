// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use fdomain_fuchsia_settings::{ConfigurationInterfaces, SetupProxy, SetupSettings};
use ffx_setui_setup_args::Setup;
use ffx_writer::SimpleWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use target_holders::moniker;
use utils::{Either, WatchOrSetResult, handle_mixed_result};

#[derive(FfxTool)]
#[check(AvailabilityFlag("setui"))]
pub struct SetupTool {
    #[command]
    cmd: Setup,
    #[with(moniker("/core/setui_service"))]
    setup_proxy: SetupProxy,
}

fho::embedded_plugin!(SetupTool);

#[async_trait(?Send)]
impl FfxMain for SetupTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        run_command(self.setup_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

async fn run_command<W: std::io::Write>(
    setup_proxy: SetupProxy,
    setup: Setup,
    writer: &mut W,
) -> Result<()> {
    handle_mixed_result("Setup", command(setup_proxy, setup.configuration_interfaces).await, writer)
        .await
}

async fn command(
    proxy: SetupProxy,
    configuration_interfaces: Option<ConfigurationInterfaces>,
) -> WatchOrSetResult {
    let mut settings = SetupSettings::default();
    settings.enabled_configuration_interfaces = configuration_interfaces;

    if settings == SetupSettings::default() {
        Ok(Either::Watch(utils::watch_to_stream(proxy, |p| p.watch())))
    } else {
        // Default to reboot the device in order for the change to take effect.
        Ok(Either::Set(if let Err(err) = proxy.set(&settings, true).await? {
            format!("{:?}", err)
        } else {
            format!("Successfully set configuration interfaces to {:?}", configuration_interfaces)
        }))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_settings::SetupRequest;
    use target_holders::fake_proxy;
    use test_case::test_case;

    #[fuchsia::test]
    async fn test_run_command() {
        const INTERFACE: ConfigurationInterfaces = ConfigurationInterfaces::ETHERNET;

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            SetupRequest::Set { settings, responder, .. } => {
                if let Some(val) = settings.enabled_configuration_interfaces {
                    assert_eq!(val, INTERFACE);
                    let _ = responder.send(Ok(()));
                } else {
                    panic!("Unexpected call to set");
                }
            }
            SetupRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let setup = Setup { configuration_interfaces: Some(INTERFACE) };
        let response = run_command(proxy, setup, &mut vec![]).await;
        assert!(response.is_ok());
    }

    #[test_case(
        ConfigurationInterfaces::ETHERNET;
        "Test setup set() output with ethernet config."
    )]
    #[test_case(
        ConfigurationInterfaces::WIFI;
        "Test setup set() output with wifi config."
    )]
    #[fuchsia::test]
    async fn validate_setup_output(expected_interface: ConfigurationInterfaces) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            SetupRequest::Set { settings, responder, .. } => {
                if let Some(val) = settings.enabled_configuration_interfaces {
                    assert_eq!(val, expected_interface);
                    let _ = responder.send(Ok(()));
                } else {
                    panic!("Unexpected call to set");
                }
            }
            SetupRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let output = utils::assert_set!(command(proxy, Some(expected_interface)));
        assert_eq!(
            output,
            format!("Successfully set configuration interfaces to {:?}", Some(expected_interface))
        );
        Ok(())
    }

    #[test_case(
        None;
        "Test setup watch() output with empty config."
    )]
    #[test_case(
        Some(ConfigurationInterfaces::ETHERNET);
        "Test setup watch() output with non-empty config."
    )]
    #[fuchsia::test]
    async fn validate_setup_watch_output(
        expected_interface: Option<ConfigurationInterfaces>,
    ) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            SetupRequest::Set { .. } => {
                panic!("Unexpected call to set");
            }
            SetupRequest::Watch { responder } => {
                let _ = responder.send(&SetupSettings {
                    enabled_configuration_interfaces: expected_interface,
                    ..Default::default()
                });
            }
        });

        let output = utils::assert_watch!(command(proxy, None));
        assert_eq!(
            output,
            format!(
                "{:#?}",
                SetupSettings {
                    enabled_configuration_interfaces: expected_interface,
                    ..Default::default()
                }
            )
        );
        Ok(())
    }
}
