// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Battery CLI Diagnostic and Control Tool.

mod battery;
mod charger;
mod common;
mod spmi;

use anyhow::Result;
use argh::FromArgs;
use spmi::PowerSource;

#[derive(FromArgs, Debug, PartialEq)]
/// Inspect battery telemetry and control power/charging.
pub struct Args {
    #[argh(option, short = 'p')]
    /// optional specific service instance or device path (e.g.
    /// '/svc/fuchsia.hardware.power.battery.Service/default')
    pub path: Option<String>,

    #[argh(subcommand)]
    pub command: Option<Subcommand>,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
pub enum Subcommand {
    Get(GetCommand),
    Watch(WatchCommand),
    Enable(EnableCommand),
    Power(PowerCommand),
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "get")]
/// inspect battery telemetry
pub struct GetCommand {}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "watch")]
/// stream real-time battery status updates via hanging-get
pub struct WatchCommand {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnableArg(pub bool);

impl std::str::FromStr for EnableArg {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "enable" => Ok(Self(true)),
            "0" | "false" | "off" | "disable" => Ok(Self(false)),
            _ => Err(format!(
                "Invalid enable argument '{s}'. Use 1/true/on/enable or 0/false/off/disable."
            )),
        }
    }
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "enable")]
/// enable or disable battery charging (e.g. 'batteryutil enable 1' or 'batteryutil enable on')
pub struct EnableCommand {
    #[argh(positional)]
    /// set to 1/true/on to enable charging, 0/false/off to disable charging
    pub enable: EnableArg,
}

#[derive(FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "power")]
/// low-level SPMI test override for power source (Sorrel only: battery or usb)
pub struct PowerCommand {
    #[argh(positional)]
    /// source: battery or usb
    pub source: PowerSource,
}

#[fuchsia::main]
async fn main() -> Result<()> {
    let args: Args = argh::from_env();
    let path = args.path.as_deref();

    match args.command {
        None | Some(Subcommand::Get(_)) => battery::get_battery_info(path).await,
        Some(Subcommand::Watch(_)) => battery::watch_battery(path).await,
        Some(Subcommand::Enable(EnableCommand { enable })) => {
            charger::enable_charger(path, enable.0).await
        }
        Some(Subcommand::Power(PowerCommand { source })) => {
            spmi::set_spmi_power_source(source).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_argh_args_parsing() {
        let args = Args::from_args(&["batteryutil"], &["get"]).unwrap();
        assert_eq!(args.command, Some(Subcommand::Get(GetCommand {})));

        let args = Args::from_args(&["batteryutil"], &["-p", "/svc/test", "enable", "1"]).unwrap();
        assert_eq!(args.path, Some("/svc/test".to_string()));
        assert_eq!(
            args.command,
            Some(Subcommand::Enable(EnableCommand { enable: EnableArg(true) }))
        );

        let args = Args::from_args(&["batteryutil"], &["watch"]).unwrap();
        assert_eq!(args.command, Some(Subcommand::Watch(WatchCommand {})));

        let args = Args::from_args(&["batteryutil"], &["power", "battery"]).unwrap();
        assert_eq!(
            args.command,
            Some(Subcommand::Power(PowerCommand { source: PowerSource::Battery }))
        );
    }

    #[test]
    fn test_parse_enable_arg() {
        assert_eq!("1".parse::<EnableArg>(), Ok(EnableArg(true)));
        assert_eq!("true".parse::<EnableArg>(), Ok(EnableArg(true)));
        assert_eq!("on".parse::<EnableArg>(), Ok(EnableArg(true)));
        assert_eq!("enable".parse::<EnableArg>(), Ok(EnableArg(true)));
        assert_eq!("0".parse::<EnableArg>(), Ok(EnableArg(false)));
        assert_eq!("false".parse::<EnableArg>(), Ok(EnableArg(false)));
        assert_eq!("off".parse::<EnableArg>(), Ok(EnableArg(false)));
        assert_eq!("disable".parse::<EnableArg>(), Ok(EnableArg(false)));
        assert!("invalid".parse::<EnableArg>().is_err());
    }
}
