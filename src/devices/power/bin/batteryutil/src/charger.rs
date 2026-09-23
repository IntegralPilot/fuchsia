// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Charger Client and Controller (supporting both
//! fuchsia.hardware.power.charger and fuchsia.power.battery.Charger).

use crate::common::{MEMBER_CHARGER, MEMBER_DEVICE, append_member_suffix, select_instance};
use anyhow::{Context, Result, anyhow};
use fidl::endpoints::ServiceMarker;
use fidl_fuchsia_hardware_power_charger as fcharger;
use fidl_fuchsia_power_battery as fpowerbattery;
use fuchsia_component::client::connect_to_protocol_at_path;

pub async fn enable_charger(path: Option<&str>, enable: bool) -> Result<()> {
    let target = if let Some(p) = path {
        if p.contains(fpowerbattery::ChargerServiceMarker::SERVICE_NAME) {
            set_legacy_charger_enable(p, enable).await?
        } else if p.contains(fcharger::ServiceMarker::SERVICE_NAME) {
            set_modern_charger_enable(p, enable).await?
        } else {
            match set_modern_charger_enable(p, enable).await {
                Ok(target) => target,
                Err(_) => set_legacy_charger_enable(p, enable).await?,
            }
        }
    } else if let Ok(resolved) = select_instance(fcharger::ServiceMarker::SERVICE_NAME) {
        match set_modern_charger_enable(&resolved, enable).await {
            Ok(target) => target,
            Err(_) => {
                let legacy_resolved =
                    select_instance(fpowerbattery::ChargerServiceMarker::SERVICE_NAME)?;
                set_legacy_charger_enable(&legacy_resolved, enable).await?
            }
        }
    } else {
        let legacy_resolved = select_instance(fpowerbattery::ChargerServiceMarker::SERVICE_NAME)?;
        set_legacy_charger_enable(&legacy_resolved, enable).await?
    };

    println!("Successfully {} charging via {target}", if enable { "enabled" } else { "disabled" });
    Ok(())
}

async fn set_modern_charger_enable(path: &str, enable: bool) -> Result<String> {
    let charger_path = append_member_suffix(path, MEMBER_CHARGER);
    let charger_path_str = charger_path.to_str().context("invalid UTF-8 path")?;
    let proxy = connect_to_protocol_at_path::<fcharger::ChargerMarker>(charger_path_str)
        .with_context(|| format!("Failed to connect to Charger at {charger_path_str}"))?;

    proxy
        .set_charging_enabled(enable)
        .await
        .context("SetChargingEnabled call failed")?
        .map_err(|status| anyhow!("SetChargingEnabled rejected with status: {:?}", status))?;

    Ok(format!("fuchsia.hardware.power.charger.Charger ({charger_path_str})"))
}

async fn set_legacy_charger_enable(path: &str, enable: bool) -> Result<String> {
    let legacy_path = append_member_suffix(path, MEMBER_DEVICE);
    let legacy_path_str = legacy_path.to_str().context("invalid UTF-8 path")?;
    let proxy = connect_to_protocol_at_path::<fpowerbattery::ChargerMarker>(legacy_path_str)
        .with_context(|| {
            format!("Failed to connect to legacy Charger protocol at {legacy_path_str}")
        })?;

    proxy
        .enable(enable)
        .await
        .context("Enable call failed")?
        .map_err(|e| anyhow!("Enable rejected with domain error: {:?}", e))?;
    Ok(format!("fuchsia.power.battery.Charger ({legacy_path_str})"))
}
