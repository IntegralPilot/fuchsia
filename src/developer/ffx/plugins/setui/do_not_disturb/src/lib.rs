// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use fdomain_fuchsia_settings::{DoNotDisturbProxy, DoNotDisturbSettings};
use ffx_setui_do_not_disturb_args::DoNotDisturb;
use ffx_writer::SimpleWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use target_holders::moniker;
use utils::{Either, WatchOrSetResult, handle_mixed_result};

#[derive(FfxTool)]
#[check(AvailabilityFlag("setui"))]
pub struct DoNotDisturbTool {
    #[command]
    cmd: DoNotDisturb,
    #[with(moniker("/core/setui_service"))]
    do_not_disturb_proxy: DoNotDisturbProxy,
}

fho::embedded_plugin!(DoNotDisturbTool);

#[async_trait(?Send)]
impl FfxMain for DoNotDisturbTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        run_command(self.do_not_disturb_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

async fn run_command<W: std::io::Write>(
    do_not_disturb_proxy: DoNotDisturbProxy,
    do_not_disturb: DoNotDisturb,
    w: &mut W,
) -> Result<()> {
    handle_mixed_result("DoNotDisturb", command(do_not_disturb_proxy, do_not_disturb).await, w)
        .await
}

async fn command(proxy: DoNotDisturbProxy, do_not_disturb: DoNotDisturb) -> WatchOrSetResult {
    let settings = DoNotDisturbSettings::from(do_not_disturb);

    if settings == DoNotDisturbSettings::default() {
        Ok(Either::Watch(utils::watch_to_stream(proxy, |p| p.watch())))
    } else {
        Ok(Either::Set(if let Err(err) = proxy.set(&settings).await? {
            format!("{:?}", err)
        } else {
            format!("Successfully set DoNotDisturb to {:?}", do_not_disturb)
        }))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_settings::DoNotDisturbRequest;
    use target_holders::fake_proxy;
    use test_case::test_case;

    #[fuchsia::test]
    async fn test_run_command() {
        const USER: bool = true;
        const NIGHT_MODE: bool = false;

        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            DoNotDisturbRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            DoNotDisturbRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let dnd = DoNotDisturb { user_dnd: Some(USER), night_mode_dnd: Some(NIGHT_MODE) };
        let response = run_command(proxy, dnd, &mut vec![]).await;
        assert!(response.is_ok());
    }

    #[test_case(
        DoNotDisturb {
            user_dnd: Some(false),
            night_mode_dnd: Some(false),
        };
        "Test do not disturb set() output with both user_dnd and night_mode_dnd as false."
    )]
    #[test_case(
        DoNotDisturb {
            user_dnd: Some(true),
            night_mode_dnd: Some(false),
        };
        "Test do not disturb set() output with user_dnd as true and night_mode_dnd as false."
    )]
    #[test_case(
        DoNotDisturb {
            user_dnd: Some(false),
            night_mode_dnd: Some(true),
        };
        "Test do not disturb set() output with user_dnd and night_mode_dnd as true."
    )]
    #[fuchsia::test]
    async fn validate_do_not_disturb_set_output(
        expected_do_not_disturb: DoNotDisturb,
    ) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            DoNotDisturbRequest::Set { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            DoNotDisturbRequest::Watch { .. } => {
                panic!("Unexpected call to watch");
            }
        });

        let output = utils::assert_set!(command(proxy, expected_do_not_disturb));
        assert_eq!(
            output,
            format!("Successfully set DoNotDisturb to {:?}", expected_do_not_disturb)
        );
        Ok(())
    }

    #[test_case(
        DoNotDisturb {
            user_dnd: None,
            night_mode_dnd: None,
        };
        "Test do not disturb watch() output with empty DoNotDisturb."
    )]
    #[test_case(
        DoNotDisturb {
            user_dnd: None,
            night_mode_dnd: Some(false),
        };
        "Test do not disturb watch() output with non-empty DoNotDisturb."
    )]
    #[fuchsia::test]
    async fn validate_do_not_disturb_watch_output(
        expected_do_not_disturb: DoNotDisturb,
    ) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            DoNotDisturbRequest::Set { .. } => {
                panic!("Unexpected call to set");
            }
            DoNotDisturbRequest::Watch { responder } => {
                let _ = responder.send(&DoNotDisturbSettings::from(expected_do_not_disturb));
            }
        });

        let output = utils::assert_watch!(command(
            proxy,
            DoNotDisturb { user_dnd: None, night_mode_dnd: None }
        ));
        assert_eq!(output, format!("{:#?}", DoNotDisturbSettings::from(expected_do_not_disturb)));
        Ok(())
    }
}
