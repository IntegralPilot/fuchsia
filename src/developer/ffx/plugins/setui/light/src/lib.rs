// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use async_trait::async_trait;
use fdomain_fuchsia_settings::{LightProxy, LightState};
use ffx_setui_light_args::LightGroup;
use ffx_writer::SimpleWriter;
use fho::{AvailabilityFlag, FfxMain, FfxTool};
use target_holders::moniker;
use utils::{Either, WatchOrSetResult, handle_mixed_result};

#[derive(FfxTool)]
#[check(AvailabilityFlag("setui"))]
pub struct LightTool {
    #[command]
    cmd: LightGroup,
    #[with(moniker("/core/setui_service"))]
    light_proxy: LightProxy,
}

fho::embedded_plugin!(LightTool);

#[async_trait(?Send)]
impl FfxMain for LightTool {
    type Writer = SimpleWriter;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        run_command(self.light_proxy, self.cmd, &mut writer).await?;
        Ok(())
    }
}

pub async fn run_command<W: std::io::Write>(
    light_proxy: LightProxy,
    light_group: LightGroup,
    writer: &mut W,
) -> Result<()> {
    handle_mixed_result("Light", command(light_proxy, light_group).await, writer).await
}

async fn command(proxy: LightProxy, light_group: LightGroup) -> WatchOrSetResult {
    let has_name = light_group.name.is_some();
    let has_values =
        light_group.simple.len() + light_group.brightness.len() + light_group.rgb.len() > 0;

    if !has_name && !has_values {
        // No values set, perform a watch instead.
        return Ok(Either::Watch(utils::watch_to_stream(proxy, |p| p.watch_light_groups())));
    }

    if !has_values {
        // Only name specified, perform watch on individual light group.
        return Ok(Either::Watch(utils::watch_to_stream(proxy, move |p: &LightProxy| {
            p.watch_light_group(light_group.name.clone().unwrap().as_str())
        })));
    }

    if !has_name {
        return Err(format_err!("light group name required"));
    }

    let light_states: Vec<LightState> = light_group.clone().into();
    let result = proxy
        .set_light_group_values(light_group.name.clone().unwrap().as_str(), &light_states)
        .await?;

    Ok(Either::Set(match result {
        Ok(_) => format!(
            "Successfully set light group {} with values {:?}",
            light_group.name.unwrap(),
            light_states
        ),
        Err(err) => format!("{:#?}", err),
    }))
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_settings::{
        LightGroup as LightGroupSettings, LightRequest, LightType, LightValue,
    };
    use target_holders::fake_proxy;
    use test_case::test_case;

    const TEST_NAME: &str = "test_name";
    const LIGHT_VAL_1: f64 = 0.2;
    const LIGHT_VAL_2: f64 = 0.42;

    #[fuchsia::test]
    async fn test_run_command() {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            LightRequest::SetLightGroupValues { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            LightRequest::WatchLightGroups { .. } => {
                panic!("Unexpected call to watch light groups");
            }
            LightRequest::WatchLightGroup { .. } => {
                panic!("Unexpected call to watch a light group");
            }
        });

        let light = LightGroup {
            name: Some(TEST_NAME.to_string()),
            simple: vec![],
            brightness: vec![LIGHT_VAL_1, LIGHT_VAL_2],
            rgb: vec![],
        };
        let response = run_command(proxy, light, &mut vec![]).await;
        assert!(response.is_ok());
    }

    #[test_case(
        LightGroup {
            name: Some(TEST_NAME.to_string()),
            simple: vec![],
            brightness: vec![0.2, 0.42],
            rgb: vec![],
        };
        "Test light set() output with non-empty input."
    )]
    #[test_case(
        LightGroup {
            name: Some(TEST_NAME.to_string()),
            simple: vec![],
            brightness: vec![0.2, 0.42, 0.5],
            rgb: vec![],
        };
        "Test light set() output with a different non-empty input."
    )]
    #[fuchsia::test]
    async fn validate_light_set_output(expected_light: LightGroup) -> Result<()> {
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            LightRequest::SetLightGroupValues { responder, .. } => {
                let _ = responder.send(Ok(()));
            }
            LightRequest::WatchLightGroups { .. } => {
                panic!("Unexpected call to watch light groups");
            }
            LightRequest::WatchLightGroup { .. } => {
                panic!("Unexpected call to watch a light group");
            }
        });

        let light_states: Vec<LightState> = expected_light.clone().into();
        let output = utils::assert_set!(command(proxy, expected_light.clone()));
        assert_eq!(
            output,
            format!(
                "Successfully set light group {} with values {:?}",
                expected_light.name.unwrap(),
                light_states
            )
        );
        Ok(())
    }

    #[test_case(
        LightGroupSettings {
            name: Some(TEST_NAME.to_string()),
            enabled: Some(false),
            type_: Some(LightType::Simple),
            lights: Some(vec![
                LightState { value: Some(LightValue::Brightness(0.2)), ..Default::default() },
                LightState { value: Some(LightValue::Brightness(0.42)), ..Default::default() }
            ]),
            ..Default::default()
        };
        "Test light watch() output."
    )]
    #[test_case(
        LightGroupSettings {
            name: Some(TEST_NAME.to_string()),
            enabled: Some(true),
            type_: Some(LightType::Simple),
            lights: Some(vec![
                LightState { value: Some(LightValue::Brightness(0.2)), ..Default::default() },
                LightState { value: Some(LightValue::Brightness(0.42)), ..Default::default() },
                LightState { value: Some(LightValue::Brightness(0.66)), ..Default::default() }
            ]),
            ..Default::default()
        };
        "Test light watch() output with different values."
    )]
    #[fuchsia::test]
    async fn validate_light_watch_output(
        expected_light_settings: LightGroupSettings,
    ) -> Result<()> {
        let groups = [expected_light_settings];
        let groups_clone = groups.clone();
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            LightRequest::SetLightGroupValues { .. } => {
                panic!("Unexpected call to set");
            }
            LightRequest::WatchLightGroups { responder } => {
                let _ = responder.send(&groups);
            }
            LightRequest::WatchLightGroup { .. } => {
                panic!("Unexpected call to watch a light group");
            }
        });

        let output = utils::assert_watch!(command(
            proxy,
            LightGroup { name: None, simple: vec![], brightness: vec![], rgb: vec![] }
        ));
        assert_eq!(output, format!("{:#?}", groups_clone));
        Ok(())
    }

    #[test_case(
        LightGroupSettings {
            name: Some(TEST_NAME.to_string()),
            enabled: Some(false),
            type_: Some(LightType::Simple),
            lights: Some(vec![
                LightState { value: Some(LightValue::Brightness(0.2)), ..Default::default() },
                LightState { value: Some(LightValue::Brightness(0.42)), ..Default::default() }
            ]),
            ..Default::default()
        };
        "Test individual light watch() output."
    )]
    #[test_case(
        LightGroupSettings {
            name: Some(TEST_NAME.to_string()),
            enabled: Some(true),
            type_: Some(LightType::Simple),
            lights: Some(vec![
                LightState { value: Some(LightValue::Brightness(0.2)), ..Default::default() }
            ]),
            ..Default::default()
        };
        "Test individual light watch() output with different values."
    )]
    #[fuchsia::test]
    async fn validate_light_watch_individual_output(
        expected_light_settings: LightGroupSettings,
    ) -> Result<()> {
        let val_clone = expected_light_settings.clone();
        let client = fdomain_local::local_client_empty();
        let proxy = fake_proxy(client, move |req| match req {
            LightRequest::SetLightGroupValues { .. } => {
                panic!("Unexpected call to set");
            }
            LightRequest::WatchLightGroups { .. } => {
                panic!("Unexpected call to watch light groups");
            }
            LightRequest::WatchLightGroup { responder, .. } => {
                let _ = responder.send(&expected_light_settings);
            }
        });

        let output = utils::assert_watch!(command(
            proxy,
            LightGroup {
                name: Some(TEST_NAME.to_string()),
                simple: vec![],
                brightness: vec![],
                rgb: vec![]
            }
        ));
        assert_eq!(output, format!("{:#?}", val_clone));
        Ok(())
    }
}
