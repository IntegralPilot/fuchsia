// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use argh::FromArgs;
use fidl_fuchsia_usb_policy as usb_policy;

mod config;
mod health;
mod inspect;

#[derive(FromArgs, PartialEq, Debug)]
/// USB diagnostics and configuration CLI tool.
struct UsbCliArgs {
    #[argh(subcommand)]
    subcommand: SubCommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
enum SubCommand {
    Health(HealthArgs),
    Inspect(InspectArgs),
    Diagnostics(DiagnosticsArgs),
    Diag(DiagArgs),
    GetConfig(GetConfigArgs),
    SetConfig(SetConfigArgs),
}

#[derive(FromArgs, PartialEq, Debug)]
/// Prints the USB policy health report.
#[argh(subcommand, name = "health")]
struct HealthArgs {
    /// prints verbose health details
    #[argh(switch, short = 'v')]
    verbose: bool,
}

#[derive(FromArgs, PartialEq, Debug)]
/// Prints the device-side USB Inspect diagnostics.
#[argh(subcommand, name = "inspect")]
struct InspectArgs {}

#[derive(FromArgs, PartialEq, Debug)]
/// Prints both USB health report and Inspect diagnostics.
#[argh(subcommand, name = "diagnostics")]
struct DiagnosticsArgs {
    /// prints verbose health details
    #[argh(switch, short = 'v')]
    verbose: bool,
}

#[derive(FromArgs, PartialEq, Debug)]
/// Prints both USB health report and Inspect diagnostics (alias for 'diagnostics').
#[argh(subcommand, name = "diag")]
struct DiagArgs {
    /// prints verbose health details
    #[argh(switch, short = 'v')]
    verbose: bool,
}

#[derive(FromArgs, PartialEq, Debug)]
/// Prints the current USB peripheral configuration in JSON format.
#[argh(subcommand, name = "get-config")]
struct GetConfigArgs {}

#[derive(FromArgs, PartialEq, Debug)]
/// Sets the USB peripheral configuration.
///
/// Supported input formats:
///   - Single configuration (comma-separated functions):
///       usb-cli set-config "cdc,adb"
///       usb-cli set-config "sourcesink"
///       usb-cli set-config "loopback"
///   - Multi-configuration (semicolon-separated configurations):
///       usb-cli set-config "sourcesink;loopback"
///       usb-cli set-config "cdc;vsock"
///       usb-cli set-config "cdc,adb;vsock"
///   - JSON configuration string:
///       usb-cli set-config '{"configurations": [["sourcesink"]]}'
///       usb-cli set-config '{"configurations": [["cdc", "sourcesink"], ["loopback"]]}'
///   - JSON configuration file path:
///       usb-cli set-config /path/to/usb_config.json
#[argh(subcommand, name = "set-config")]
struct SetConfigArgs {
    /// configuration string (e.g. "cdc,adb", "cdc;vsock", "sourcesink;loopback"), inline JSON, or JSON file path
    #[argh(positional)]
    config: String,
}

#[fuchsia::main(logging_tags = ["usb-cli"])]
async fn main() {
    if let Err(e) = run_cli().await {
        eprintln!("usb-cli error: {:?}", e);
        std::process::exit(1);
    }
    println!("[usb-cli:DONE]");
}

async fn get_configuration_client() -> Result<usb_policy::ConfigurationProxy, Error> {
    if std::path::Path::new("/exposed/fuchsia.usb.policy.Configuration").exists() {
        return fuchsia_component::client::connect_to_protocol_at_path::<
            usb_policy::ConfigurationMarker,
        >("/exposed/fuchsia.usb.policy.Configuration")
        .context("Failed to connect to /exposed/fuchsia.usb.policy.Configuration");
    }

    fuchsia_component::client::connect_to_protocol::<usb_policy::ConfigurationMarker>()
        .context("Failed to connect to fuchsia.usb.policy.Configuration protocol")
}

async fn run_get_config(_args: GetConfigArgs) -> Result<(), Error> {
    let config_client = get_configuration_client().await?;
    let (device_desc, config_descriptors) = config_client
        .get_configuration()
        .await
        .context("Failed FIDL call get_configuration")?
        .map_err(zx::Status::err_from_raw)
        .context("GetConfiguration returned an error status")?;

    let configurations = config::config_descriptors_to_names(&config_descriptors);

    let json_output = config::UsbConfigJson {
        configurations,
        id_vendor: (device_desc.id_vendor != 0).then_some(device_desc.id_vendor),
        id_product: Some(device_desc.id_product),
        product: (!device_desc.product.is_empty()).then_some(device_desc.product),
    };

    let serialized = serde_json::to_string_pretty(&json_output).context("Failed to format JSON")?;
    println!("{}", serialized);
    Ok(())
}

async fn run_set_config(args: SetConfigArgs) -> Result<(), Error> {
    let parsed_config = config::load_config_input(&args.config)?;
    let config_descriptors = config::resolve_config_descriptors(&parsed_config)?;
    let config_client = get_configuration_client().await?;

    let (device_desc, _) = config_client
        .get_configuration()
        .await
        .context("Failed FIDL call get_configuration")?
        .map_err(zx::Status::err_from_raw)
        .context("GetConfiguration returned an error status")?;

    let num_configurations =
        u8::try_from(config_descriptors.len()).context("Too many configurations")?;
    let (device_desc, standard_derived) =
        config::update_device_descriptor(&parsed_config, device_desc, num_configurations);

    if standard_derived {
        println!(
            "Using standard USB identifiers: VID 0x{:04x}, PID 0x{:04x} ('{}')",
            device_desc.id_vendor, device_desc.id_product, device_desc.product
        );
    } else {
        match (&parsed_config.id_product, &parsed_config.product) {
            (None, None) => {
                println!(
                    "Note: No standard USB PID found for this configuration; retaining current PID (0x{:04x}) and product string.",
                    device_desc.id_product
                );
            }
            (None, Some(_)) => {
                println!(
                    "Note: No standard USB PID found for this configuration; retaining current PID (0x{:04x}).",
                    device_desc.id_product
                );
            }
            _ => {}
        }
    }

    println!(
        "Applying new configuration via Policy ({} configuration(s): {:?})...",
        config_descriptors.len(),
        config::config_descriptors_to_names(&config_descriptors)
    );
    config_client
        .set_configuration(&device_desc, &config_descriptors)
        .await
        .context("Failed set_configuration FIDL call")?
        .map_err(zx::Status::err_from_raw)
        .context("SetConfiguration returned an error status")?;

    println!("Successfully applied USB peripheral configuration.");

    let (active_device_desc, active_config_descriptors) = config_client
        .get_configuration()
        .await
        .context("Failed FIDL call get_configuration to query active configuration")?
        .map_err(zx::Status::err_from_raw)
        .context("GetConfiguration returned an error status")?;

    let active_configurations = config::config_descriptors_to_names(&active_config_descriptors);

    println!(
        "Active configuration (VID: 0x{:04x}, PID: 0x{:04x}, {} configuration(s)): {:?}",
        active_device_desc.id_vendor,
        active_device_desc.id_product,
        active_configurations.len(),
        active_configurations
    );
    Ok(())
}

async fn get_health_report() -> Result<usb_policy::HealthReport, Error> {
    let health =
        fuchsia_component::client::connect_to_protocol_at_path::<usb_policy::HealthMarker>(
            "/exposed/fuchsia.usb.policy.Health",
        )
        .map_err(|e| {
            anyhow::format_err!("Failed to connect to Health protocol at /exposed: {e:?}")
        })?;

    health
        .get_report()
        .await
        .map_err(|e| anyhow::format_err!("Failed to communicate (get_report): {e:?}"))?
        .map_err(|e| anyhow::format_err!("Failed to get report (zx status): {e:?}"))
}

async fn run_health(args: HealthArgs) -> Result<(), Error> {
    match get_health_report().await {
        Ok(report) => {
            if args.verbose {
                println!("{}", health::format_verbose(&report));
            } else {
                println!("{}", health::format_dashboard(&report));
            }
            Ok(())
        }
        Err(e) => {
            println!(
                "USB Policy Health service not available: fuchsia.usb.policy.Health not found."
            );
            if args.verbose {
                println!("Error details: {e:?}");
            }
            Err(e)
        }
    }
}

async fn run_diagnostics(verbose: bool) -> Result<(), Error> {
    match get_health_report().await {
        Ok(report) => {
            if verbose {
                println!("{}", health::format_verbose(&report));
            } else {
                println!("{}", health::format_dashboard(&report));
            }
        }
        Err(_) => {
            println!("USB Policy Health service not available (skipping).");
        }
    }

    if let Err(e) = inspect::print_usb_inspect_diagnostics().await {
        println!("Failed to print USB inspect diagnostics: {:?}", e);
    }

    Ok(())
}

async fn run_cli() -> Result<(), Error> {
    let args: UsbCliArgs = argh::from_env();
    match args.subcommand {
        SubCommand::Health(sc_args) => run_health(sc_args).await,
        SubCommand::Inspect(_) => inspect::print_usb_inspect_diagnostics().await,
        SubCommand::Diagnostics(sc_args) => run_diagnostics(sc_args.verbose).await,
        SubCommand::Diag(sc_args) => run_diagnostics(sc_args.verbose).await,
        SubCommand::GetConfig(sc_args) => run_get_config(sc_args).await,
        SubCommand::SetConfig(sc_args) => run_set_config(sc_args).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_missing_subcommand() {
        assert!(UsbCliArgs::from_args(&["usb-cli"], &[]).is_err());
    }

    #[test]
    fn test_parse_health() {
        let args = UsbCliArgs::from_args(&["usb-cli"], &["health"]).unwrap();
        assert_eq!(
            args,
            UsbCliArgs { subcommand: SubCommand::Health(HealthArgs { verbose: false }) }
        );

        let args_v = UsbCliArgs::from_args(&["usb-cli"], &["health", "-v"]).unwrap();
        assert_eq!(
            args_v,
            UsbCliArgs { subcommand: SubCommand::Health(HealthArgs { verbose: true }) }
        );
    }

    #[test]
    fn test_parse_inspect() {
        let args = UsbCliArgs::from_args(&["usb-cli"], &["inspect"]).unwrap();
        assert_eq!(args, UsbCliArgs { subcommand: SubCommand::Inspect(InspectArgs {}) });
    }

    #[test]
    fn test_parse_diagnostics() {
        let args = UsbCliArgs::from_args(&["usb-cli"], &["diagnostics"]).unwrap();
        assert_eq!(
            args,
            UsbCliArgs { subcommand: SubCommand::Diagnostics(DiagnosticsArgs { verbose: false }) }
        );

        let args_v = UsbCliArgs::from_args(&["usb-cli"], &["diagnostics", "--verbose"]).unwrap();
        assert_eq!(
            args_v,
            UsbCliArgs { subcommand: SubCommand::Diagnostics(DiagnosticsArgs { verbose: true }) }
        );
    }

    #[test]
    fn test_parse_diag_alias() {
        let args = UsbCliArgs::from_args(&["usb-cli"], &["diag"]).unwrap();
        assert_eq!(args, UsbCliArgs { subcommand: SubCommand::Diag(DiagArgs { verbose: false }) });

        let args_v = UsbCliArgs::from_args(&["usb-cli"], &["diag", "-v"]).unwrap();
        assert_eq!(args_v, UsbCliArgs { subcommand: SubCommand::Diag(DiagArgs { verbose: true }) });

        let args_verbose = UsbCliArgs::from_args(&["usb-cli"], &["diag", "--verbose"]).unwrap();
        assert_eq!(
            args_verbose,
            UsbCliArgs { subcommand: SubCommand::Diag(DiagArgs { verbose: true }) }
        );
    }

    #[test]
    fn test_parse_get_config() {
        let args = UsbCliArgs::from_args(&["usb-cli"], &["get-config"]).unwrap();
        assert_eq!(args, UsbCliArgs { subcommand: SubCommand::GetConfig(GetConfigArgs {}) });
    }

    #[test]
    fn test_parse_set_config() {
        let args = UsbCliArgs::from_args(&["usb-cli"], &["set-config", "cdc,sourcesink"]).unwrap();
        assert_eq!(
            args,
            UsbCliArgs {
                subcommand: SubCommand::SetConfig(SetConfigArgs {
                    config: "cdc,sourcesink".to_string(),
                }),
            }
        );

        let multi_args =
            UsbCliArgs::from_args(&["usb-cli"], &["set-config", "cdc,sourcesink;loopback"])
                .unwrap();
        assert_eq!(
            multi_args,
            UsbCliArgs {
                subcommand: SubCommand::SetConfig(SetConfigArgs {
                    config: "cdc,sourcesink;loopback".to_string(),
                }),
            }
        );

        let test_args =
            UsbCliArgs::from_args(&["usb-cli"], &["set-config", "sourcesink;loopback"]).unwrap();
        assert_eq!(
            test_args,
            UsbCliArgs {
                subcommand: SubCommand::SetConfig(SetConfigArgs {
                    config: "sourcesink;loopback".to_string(),
                }),
            }
        );

        let json_input = r#"{"configurations": [["sourcesink"]]}"#;
        let json_args = UsbCliArgs::from_args(&["usb-cli"], &["set-config", json_input]).unwrap();
        assert_eq!(
            json_args,
            UsbCliArgs {
                subcommand: SubCommand::SetConfig(SetConfigArgs { config: json_input.to_string() }),
            }
        );
    }
}
