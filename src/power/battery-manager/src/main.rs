// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod battery_info_recorders;
mod battery_manager;
mod battery_simulator;
mod config;
mod polisher;

use crate::battery_info_recorders::RecorderConfig;
use crate::battery_manager::{BatteryManager, BatterySimulationStateObserver};
use crate::battery_simulator::SimulatedBatteryInfoSource;
pub(crate) use crate::config::BatteryManagerConfig;
use crate::config::read_battery_manager_config;
use anyhow::Error;
use battery_manager_config::Config;
use fidl_fuchsia_hardware_power_battery as fbattery;
use fidl_fuchsia_power_battery as fpower;
use fidl_fuchsia_power_battery_test as spower;
use fidl_fuchsia_power_system as fsystem;
use fuchsia_async as fasync;
use fuchsia_component::client as fclient;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::{self as inspect};
use futures::prelude::*;
use inspect_runtime::PublishOptions;
use log::{error, info, warn};
use std::sync::{Arc, Weak};

pub(crate) enum BatteryInfoSource {
    New(fbattery::BatteryProxy),
    ModernService(fpower::BatteryInfoProviderProxy),
}

enum IncomingService {
    BatteryManager(fpower::BatteryManagerRequestStream),
    BatterySimulator(spower::BatterySimulatorRequestStream),
}

const CURR_BOOT_BATTERY_HISTORY_FILE: &str = "/data/history.txt";

async fn get_battery_info_source() -> Result<BatteryInfoSource, Error> {
    info!("Looking for battery info service (new or old)...");

    let new_stream = match fclient::Service::open(fbattery::ServiceMarker) {
        Ok(s) => match s.watch().await {
            Ok(w) => Some(w),
            Err(e) => {
                warn!("Failed to watch new battery service: {:?}", e);
                None
            }
        },
        Err(e) => {
            warn!("Failed to open new battery service: {:?}", e);
            None
        }
    };

    let old_stream = match fclient::Service::open(fpower::InfoServiceMarker) {
        Ok(s) => match s.watch().await {
            Ok(w) => Some(w),
            Err(e) => {
                warn!("Failed to watch old battery service: {:?}", e);
                None
            }
        },
        Err(e) => {
            warn!("Failed to open old battery service: {:?}", e);
            None
        }
    };

    if new_stream.is_none() && old_stream.is_none() {
        return Err(anyhow::anyhow!(
            "Failed to initialize both battery service watchers. Check component manifest."
        ));
    }

    // Use futures::stream::iter to turn Option<impl Stream> into a Stream,
    // and flatten it to get a single stream of instances.
    // If the original stream was None, it becomes an empty stream that never yields.
    let mut new_stream = futures::stream::iter(new_stream).flatten().fuse();
    let mut old_stream = futures::stream::iter(old_stream).flatten().fuse();

    loop {
        futures::select! {
            instance_res = new_stream.select_next_some() => {
                match instance_res {
                    Ok(instance) => {
                        if let Ok(proxy) = instance.connect_to_battery() {
                            info!("Connected to new fuchsia.hardware.power.battery service");
                            return Ok(BatteryInfoSource::New(proxy));
                        }
                        warn!("Failed to connect to an instance of the new service, looking for next...");
                    }
                    Err(e) => warn!("New service stream error: {:?}", e),
                }
            }
            instance_res = old_stream.select_next_some() => {
                match instance_res {
                    Ok(instance) => {
                        if let Ok(proxy) = instance.connect_to_device() {
                            info!("Connected to fuchsia.power.battery service");
                            return Ok(BatteryInfoSource::ModernService(proxy));
                        }
                        warn!("Failed to connect to an instance of the old service, looking for next...");
                    }
                    Err(e) => warn!("Old service stream error: {:?}", e),
                }
            }
            complete => return Err(anyhow::anyhow!("All battery service streams closed")),
        }
    }
}

// TODO(b/523292405): Remove this function and the CURR_BOOT_BATTERY_HISTORY_FILE constant
// once we are confident that the legacy history file has been cleaned up from all user devices.
fn remove_battery_history() {
    // Remove legacy battery history file if it exists.
    let _ = std::fs::remove_file(CURR_BOOT_BATTERY_HISTORY_FILE);
}

// Paths to search for configuration: /config/config.json from BootFS (in production),
// falling back to /pkg/config/test_config.json (in integration tests).
const BOOTFS_CONFIG_PATH: &str = "/config/config.json";
const PKG_CONFIG_PATH: &str = "/pkg/config/test_config.json";

fn load_battery_manager_config_from_path(path: &str) -> Result<BatteryManagerConfig, Error> {
    info!("Loading battery manager config from {path}");
    read_battery_manager_config(path)
}

fn load_battery_manager_config() -> Result<BatteryManagerConfig, Error> {
    if std::path::Path::new(BOOTFS_CONFIG_PATH).exists() {
        load_battery_manager_config_from_path(BOOTFS_CONFIG_PATH)
    } else if std::path::Path::new(PKG_CONFIG_PATH).exists() {
        load_battery_manager_config_from_path(PKG_CONFIG_PATH)
    } else {
        info!("No config file found in bootfs or package, using default config");
        Ok(BatteryManagerConfig::default())
    }
}

#[fuchsia::main(logging_tags = ["battery_manager"])]
async fn main() -> Result<(), Error> {
    info!("starting up");

    let inspector = inspect::component::inspector();
    let _inspect_server_task = inspect_runtime::publish(inspector, PublishOptions::default());
    inspect::component::serve_inspect_stats();

    let config = Config::take_from_startup_handle();
    inspector.root().record_child("config", |config_node| config.record_inspect(config_node));
    log::info!(config:?; "config");

    // Remove the legacy battery history file before the service starts.
    remove_battery_history();

    let battery_manager_config = load_battery_manager_config()?;
    info!("Loaded battery manager config: {:?}", battery_manager_config);
    inspector
        .root()
        .record_child("battery_manager_config", |node| battery_manager_config.record_inspect(node));

    let recorder_config = RecorderConfig::default();
    let battery_manager = Arc::new(BatteryManager::new_with_battery_manager_config(
        recorder_config,
        battery_manager_config,
    ));
    let battery_manager_clone = battery_manager.clone();

    fasync::Task::local(async move {
        let source = match get_battery_info_source().await {
            Ok(s) => s,
            Err(e) => {
                error!("Error getting battery info source: {e:?}");
                return; // Exit the task on error
            }
        };

        let sag = if config.suspend_enabled {
            Some(
                fuchsia_component::client::connect_to_protocol::<fsystem::ActivityGovernorMarker>()
                    .expect("should connect to system activity governor"),
            )
        } else {
            None
        };
        if let Err(e) = battery_manager_clone.start_watching_battery_info(source, sag).await {
            error!("Error when watching battery info: {e:?}");
        }
    })
    .detach();

    let battery_simulator = Arc::new(SimulatedBatteryInfoSource::new(
        battery_manager.get_battery_info_copy(),
        Arc::downgrade(&battery_manager) as Weak<dyn BatterySimulationStateObserver>,
    ));

    let mut fs = ServiceFs::new();
    fs.dir("svc")
        .add_fidl_service(IncomingService::BatteryManager)
        .add_fidl_service(IncomingService::BatterySimulator);

    fs.take_and_serve_directory_handle()?;

    fs.for_each_concurrent(None, |request| {
        let battery_manager = battery_manager.clone();
        let battery_simulator = battery_simulator.clone();

        async move {
            match request {
                IncomingService::BatteryManager(stream) => {
                    let res = battery_manager.serve(stream).await;
                    if let Err(e) = res {
                        error!("BatteryManager failed {}", e);
                    }
                }
                IncomingService::BatterySimulator(stream) => {
                    let res = stream
                        .err_into()
                        .try_for_each_concurrent(None, |request| {
                            let battery_simulator = battery_simulator.clone();
                            let battery_manager = battery_manager.clone();
                            async move {
                                match request {
                                    spower::BatterySimulatorRequest::DisconnectRealBattery {
                                        ..
                                    } => {
                                        battery_simulator
                                            .update_simulation(
                                                true,
                                                battery_manager.get_battery_info_copy(),
                                            )
                                            .await?;
                                    }
                                    spower::BatterySimulatorRequest::ReconnectRealBattery {
                                        ..
                                    } => {
                                        battery_simulator
                                            .update_simulation(
                                                false,
                                                battery_manager.get_battery_info_copy(),
                                            )
                                            .await?;
                                    }
                                    spower::BatterySimulatorRequest::IsSimulating {
                                        responder,
                                        ..
                                    } => {
                                        let info = battery_manager.is_simulating();
                                        responder.send(info)?;
                                    }
                                    _ => {
                                        battery_simulator.handle_request(request).await?;
                                    }
                                }
                                Ok::<(), Error>(())
                            }
                        })
                        .await;

                    if let Err(e) = res {
                        error!("BatterySimulator failed {}", e);
                    }
                }
            }
        }
    })
    .await;

    info!("stopping battery_manager");
    Ok(())
}
