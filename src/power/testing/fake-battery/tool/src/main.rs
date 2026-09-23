// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, anyhow};
use argh::FromArgs;
use fidl_fuchsia_hardware_power_battery as fbattery;
use fidl_fuchsia_io as fio;
use fidl_test_hardwarepowercontrol as fcontrol;
use fuchsia_component::{SVC_DIR, client as fclient};

/// Command line tool to interact with the fake-battery driver.
/// This tool allows developers to inspect and inject fake battery metrics
/// directly into the driver under `ffx component explore`.
#[derive(FromArgs, PartialEq, Debug)]
struct TopLevel {
    #[argh(subcommand)]
    command: Command,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum Command {
    Set(SetOptions),
    Get(GetOptions),
}

/// Set fake battery telemetry state.
#[derive(FromArgs, PartialEq, Debug, Default)]
#[argh(subcommand, name = "set")]
struct SetOptions {
    /// whether battery is present (true/false)
    #[argh(option, short = 'p')]
    present: Option<bool>,

    /// battery charge level in percent (0.0 to 100.0)
    #[argh(option, short = 'l')]
    level: Option<f32>,

    /// battery charging status: "charging", "discharging", "not_charging", "full"
    #[argh(option, short = 'c')]
    status: Option<String>,

    /// battery/source voltage in millivolts (e.g. 4200 for 4.2V)
    #[argh(option, short = 'v')]
    voltage_mv: Option<u32>,

    /// current in microamps (positive = charging, negative = discharging)
    #[argh(option, short = 'i')]
    current_ua: Option<i32>,

    /// temperature in degrees Celsius (e.g. 25.0 for 25.0°C)
    #[argh(option, short = 't')]
    temp_celsius: Option<f32>,

    /// battery health: "good", "cold", "cool", "warm", "hot", "dead", "over_voltage", "unspecified_failure"
    #[argh(option)]
    health: Option<String>,

    /// remaining capacity in microamp-hours (µAh)
    #[argh(option)]
    remaining_uah: Option<u32>,

    /// full charge capacity in microamp-hours (µAh)
    #[argh(option)]
    full_capacity_uah: Option<u32>,

    /// estimated time remaining in seconds until empty/full
    #[argh(option)]
    time_remaining_sec: Option<i64>,

    /// battery charge cycle count
    #[argh(option)]
    cycle_count: Option<u32>,
}

/// Query and display current fake battery telemetry from the driver.
#[derive(FromArgs, PartialEq, Debug, Default)]
#[argh(subcommand, name = "get")]
struct GetOptions {}

fn parse_charge_status(s: &str) -> Result<fbattery::ChargeStatus, Error> {
    match s.to_ascii_lowercase().as_str() {
        "charging" => Ok(fbattery::ChargeStatus::Charging),
        "discharging" => Ok(fbattery::ChargeStatus::Discharging),
        "not_charging" | "not-charging" | "notcharging" => Ok(fbattery::ChargeStatus::NotCharging),
        "full" => Ok(fbattery::ChargeStatus::Full),
        _ => Err(anyhow!(
            "Invalid charge status '{}'. Expected: charging, discharging, not_charging, full",
            s
        )),
    }
}

fn parse_health_status(s: &str) -> Result<fbattery::HealthStatus, Error> {
    match s.to_ascii_lowercase().as_str() {
        "good" => Ok(fbattery::HealthStatus::Good),
        "cold" => Ok(fbattery::HealthStatus::Cold),
        "cool" => Ok(fbattery::HealthStatus::Cool),
        "warm" => Ok(fbattery::HealthStatus::Warm),
        "hot" => Ok(fbattery::HealthStatus::Hot),
        "dead" => Ok(fbattery::HealthStatus::Dead),
        "over_voltage" | "over-voltage" => Ok(fbattery::HealthStatus::OverVoltage),
        "unspecified_failure" | "unspecified-failure" => {
            Ok(fbattery::HealthStatus::UnspecifiedFailure)
        }
        _ => Err(anyhow!(
            "Invalid health status '{}'. Expected: good, cold, cool, warm, hot, dead, over_voltage, unspecified_failure",
            s
        )),
    }
}

fn build_battery_status(opts: &SetOptions) -> Result<fbattery::Status, Error> {
    let mut status = fbattery::Status::default();

    if let Some(present) = opts.present {
        status.present = Some(present);
    }

    if let Some(level) = opts.level {
        if !(0.0..=100.0).contains(&level) {
            return Err(anyhow!("Level percent must be between 0.0 and 100.0, got {}", level));
        }
        status.level_percent = Some(level);
    }

    if let Some(ref st) = opts.status {
        status.charge_status = Some(parse_charge_status(st)?);
    }

    if let Some(ref h) = opts.health {
        status.health = Some(parse_health_status(h)?);
    }

    if let Some(temp) = opts.temp_celsius {
        status.temp_celsius = Some(temp);
    }

    if let Some(v_mv) = opts.voltage_mv {
        status.voltage_uv = Some(v_mv * 1000);
    }

    if let Some(curr) = opts.current_ua {
        status.current_ua = Some(curr);
    }

    if let Some(rem_uah) = opts.remaining_uah {
        status.remaining_capacity_uah = Some(rem_uah);
    }

    if let Some(full_uah) = opts.full_capacity_uah {
        status.full_charge_capacity_uah = Some(full_uah);
    }

    if let Some(time_sec) = opts.time_remaining_sec {
        status.time_remaining = Some(time_sec * 1_000_000_000);
    }

    if let Some(cycles) = opts.cycle_count {
        status.cycle_count = Some(cycles);
    }

    Ok(status)
}

fn print_battery_status(status: &fbattery::Status) {
    println!("=== Fake Battery Status ===");
    if let Some(present) = status.present {
        println!("  Present:              {}", present);
    }

    if let Some(level) = status.level_percent {
        println!("  Level:                {:.1}%", level);
    }

    if let Some(ref cs) = status.charge_status {
        println!("  Charge Status:        {:?}", cs);
    }
    if let Some(ref h) = status.health {
        println!("  Health:               {:?}", h);
    }
    if let Some(temp) = status.temp_celsius {
        println!("  Temperature:          {:.1}°C", temp);
    }
    if let Some(uv) = status.voltage_uv {
        println!("  Voltage:              {} µV ({:.3} V)", uv, uv as f32 / 1_000_000.0);
    }
    if let Some(ua) = status.current_ua {
        println!("  Current:              {} µA ({:.3} mA)", ua, ua as f32 / 1_000.0);
    }
    if let Some(rem_uah) = status.remaining_capacity_uah {
        println!("  Remaining Capacity:   {} µAh", rem_uah);
    }
    if let Some(full_uah) = status.full_charge_capacity_uah {
        println!("  Full Charge Capacity: {} µAh", full_uah);
    }
    if let Some(time_rem) = status.time_remaining {
        println!("  Time Remaining:       {} s", time_rem / 1_000_000_000);
    }
    if let Some(cycles) = status.cycle_count {
        println!("  Cycle Count:          {}", cycles);
    }
}

async fn handle_set(dir: &fio::DirectoryProxy, opts: SetOptions) -> Result<(), Error> {
    let status = build_battery_status(&opts)?;

    let control = fclient::Service::open_from_dir_prefix(dir, SVC_DIR, fcontrol::ServiceMarker)
        .context("Failed to open test.hardwarepowercontrol.Service in /out")?
        .watch_for_any()
        .await
        .context("Failed to find Control service instance in /out")?
        .connect_to_control()
        .context("Failed to connect to Control protocol")?;

    control.set_battery_status(&status).await.context("Failed to call SetBatteryStatus")?;

    println!("Successfully updated fake battery state.");
    print_battery_status(&status);
    Ok(())
}

async fn handle_get(dir: &fio::DirectoryProxy) -> Result<(), Error> {
    let battery_service =
        fclient::Service::open_from_dir_prefix(dir, SVC_DIR, fbattery::ServiceMarker)
            .context("Failed to open fuchsia.hardware.power.battery.Service in /out")?
            .watch_for_any()
            .await
            .context("Failed to find Battery service instance in /out")?
            .connect_to_battery()
            .context("Failed to connect to Battery protocol")?;

    let status = battery_service
        .get_status()
        .await
        .context("FIDL error calling Battery.GetStatus")?
        .map_err(|e| anyhow!("Battery.GetStatus returned error: {:?}", e))?;

    print_battery_status(&status);
    Ok(())
}

#[fuchsia::main(logging_tags = ["fake_battery_cli"])]
async fn main() -> Result<(), Error> {
    let top_level: TopLevel = argh::from_env();

    let dir = fuchsia_fs::directory::open_in_namespace("/out", fio::PERM_READABLE)
        .context("Failed to open '/out' in namespace. Run this tool inside 'ffx component explore <fake_battery_driver>'")?;

    match top_level.command {
        Command::Set(opts) => handle_set(&dir, opts).await,
        Command::Get(_) => handle_get(&dir).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async as fasync;
    use futures::StreamExt;

    fn fake_control_server() -> (fcontrol::ControlProxy, fasync::Task<()>) {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fcontrol::ControlMarker>();
        let task = fasync::Task::local(async move {
            while let Some(Ok(req)) = stream.next().await {
                match req {
                    fcontrol::ControlRequest::SetBatteryStatus { responder, .. } => {
                        let _ = responder.send();
                    }
                }
            }
        });
        (proxy, task)
    }

    #[fuchsia::test]
    async fn test_set_battery_status() {
        let (proxy, _task) = fake_control_server();
        let options = SetOptions {
            present: Some(true),
            level: Some(75.5),
            status: Some("charging".to_string()),
            voltage_mv: Some(4200),
            current_ua: Some(250000),
            temp_celsius: Some(28.0),
            health: Some("good".to_string()),
            remaining_uah: Some(300000),
            full_capacity_uah: Some(400000),
            time_remaining_sec: Some(3600),
            cycle_count: Some(15),
        };
        let status = build_battery_status(&options).expect("build status failed");
        assert_eq!(status.present, Some(true));
        assert_eq!(status.level_percent, Some(75.5));
        assert_eq!(status.charge_status, Some(fbattery::ChargeStatus::Charging));
        assert_eq!(status.health, Some(fbattery::HealthStatus::Good));
        assert_eq!(status.temp_celsius, Some(28.0));
        assert_eq!(status.voltage_uv, Some(4200 * 1000));
        assert_eq!(status.current_ua, Some(250000));
        assert_eq!(status.remaining_capacity_uah, Some(300000));
        assert_eq!(status.full_charge_capacity_uah, Some(400000));
        assert_eq!(status.time_remaining, Some(3600 * 1_000_000_000));
        assert_eq!(status.cycle_count, Some(15));

        let res = proxy.set_battery_status(&status).await;
        assert!(res.is_ok());
    }

    #[test]
    fn test_parse_charge_status() {
        assert_eq!(parse_charge_status("charging").unwrap(), fbattery::ChargeStatus::Charging);
        assert_eq!(
            parse_charge_status("discharging").unwrap(),
            fbattery::ChargeStatus::Discharging
        );
        assert_eq!(
            parse_charge_status("not_charging").unwrap(),
            fbattery::ChargeStatus::NotCharging
        );
        assert_eq!(
            parse_charge_status("not-charging").unwrap(),
            fbattery::ChargeStatus::NotCharging
        );
        assert_eq!(parse_charge_status("full").unwrap(), fbattery::ChargeStatus::Full);
        assert!(parse_charge_status("invalid").is_err());
    }

    #[test]
    fn test_parse_health_status() {
        assert_eq!(parse_health_status("good").unwrap(), fbattery::HealthStatus::Good);
        assert_eq!(parse_health_status("cold").unwrap(), fbattery::HealthStatus::Cold);
        assert_eq!(
            parse_health_status("over_voltage").unwrap(),
            fbattery::HealthStatus::OverVoltage
        );
        assert_eq!(
            parse_health_status("over-voltage").unwrap(),
            fbattery::HealthStatus::OverVoltage
        );
        assert!(parse_health_status("invalid").is_err());
    }

    #[test]
    fn test_invalid_level() {
        let opts = SetOptions { level: Some(105.0), ..Default::default() };
        assert!(build_battery_status(&opts).is_err());
        let opts = SetOptions { level: Some(-1.0), ..Default::default() };
        assert!(build_battery_status(&opts).is_err());
    }
}
