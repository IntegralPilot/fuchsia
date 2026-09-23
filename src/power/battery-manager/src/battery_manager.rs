// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::BatteryInfoSource;
use crate::battery_info_recorders::{BatteryInfoRecorders, FaultRecoveryEvent, RecorderConfig};
use crate::polisher::Polisher;
use anyhow::Error;
use fidl::endpoints::Proxy;
use fidl_fuchsia_hardware_power_battery as fbattery;
use fidl_fuchsia_power_battery as fpower;
use fidl_fuchsia_power_system as fsystem;
use fuchsia_async as fasync;
use futures::channel::mpsc;
use futures::{StreamExt, TryStreamExt, stream};
use log::{debug, error, info, warn};
use std::cell::RefCell;
use std::rc::Rc;
use zx;

fn fbattery_to_fpower(
    info: fbattery::Status,
    spec: Option<&fbattery::Spec>,
) -> fpower::BatteryInfo {
    let mut result = fpower::BatteryInfo {
        timestamp: Some(zx::BootInstant::get().into_nanos()),
        ..Default::default()
    };

    let is_present = info.present.unwrap_or_else(|| {
        info.voltage_uv.is_some()
            || info.level_percent.is_some()
            || info.remaining_capacity_uah.is_some()
            || info.health.is_some()
            || info.charge_status.is_some()
    });
    result.status = Some(match (is_present, info.health) {
        (false, _) => fpower::BatteryStatus::NotPresent,
        (true, Some(fbattery::HealthStatus::Dead)) => fpower::BatteryStatus::NotAvailable,
        (true, _) => fpower::BatteryStatus::Ok,
    });

    result.present_voltage_mv = info.voltage_uv.map(|v| v / 1000);
    result.present_charging_current_ua = info.current_ua;

    // `fuchsia.hardware.power.battery` deliberately describes only the pack, not the external
    // supply, so the legacy `charge_source` is inferred: any state other than discharging implies
    // that an external source is attached and powering the system.
    // TODO(https://fxbug.dev/559183508): Report the real source type once battery-manager consumes
    // the charger/source protocols directly, instead of assuming an AC adapter.
    result.charge_source = info.charge_status.map(|status| match status {
        fbattery::ChargeStatus::Discharging => fpower::ChargeSource::None,
        fbattery::ChargeStatus::Charging
        | fbattery::ChargeStatus::Full
        | fbattery::ChargeStatus::NotCharging => fpower::ChargeSource::AcAdapter,
        _ => fpower::ChargeSource::Unknown,
    });

    result.charge_status = info.charge_status.map(|charge_status| match charge_status {
        fbattery::ChargeStatus::NotCharging => fpower::ChargeStatus::NotCharging,
        fbattery::ChargeStatus::Charging => fpower::ChargeStatus::Charging,
        fbattery::ChargeStatus::Discharging => fpower::ChargeStatus::Discharging,
        fbattery::ChargeStatus::Full => fpower::ChargeStatus::Full,
        _ => fpower::ChargeStatus::Unknown,
    });

    result.level_percent = info.level_percent;
    result.remaining_charge_uah = info.remaining_capacity_uah;
    result.full_capacity_uah =
        info.full_charge_capacity_uah.map(|v| v.try_into().unwrap_or(i32::MAX));

    result.health = info.health.map(|health| match health {
        fbattery::HealthStatus::Good => fpower::HealthStatus::Good,
        fbattery::HealthStatus::Cold => fpower::HealthStatus::Cold,
        fbattery::HealthStatus::Cool => fpower::HealthStatus::Cool,
        fbattery::HealthStatus::Warm => fpower::HealthStatus::Warm,
        fbattery::HealthStatus::Hot => fpower::HealthStatus::Hot,
        fbattery::HealthStatus::Dead => fpower::HealthStatus::Dead,
        fbattery::HealthStatus::OverVoltage => fpower::HealthStatus::OverVoltage,
        fbattery::HealthStatus::UnspecifiedFailure => fpower::HealthStatus::UnspecifiedFailure,
        _ => fpower::HealthStatus::Unknown,
    });

    const MILLIDEGREES_PER_DEGREE: f32 = 1000.0;
    result.temperature_mc = info
        .temp_celsius
        .filter(|t| t.is_finite())
        .map(|t| (t * MILLIDEGREES_PER_DEGREE).round() as i32);

    result.time_remaining = info.time_remaining.map(|time| match info.charge_status {
        Some(fbattery::ChargeStatus::Charging) => fpower::TimeRemaining::FullCharge(time),
        Some(fbattery::ChargeStatus::Discharging) => fpower::TimeRemaining::BatteryLife(time),
        _ => match info.current_ua {
            Some(c) if c > 0 => fpower::TimeRemaining::FullCharge(time),
            Some(c) if c < 0 => fpower::TimeRemaining::BatteryLife(time),
            _ => fpower::TimeRemaining::Indeterminate(time),
        },
    });

    result.battery_spec = spec.map(|s| fpower::BatterySpec {
        design_capacity_uah: s.design_capacity_uah.map(|v| v.try_into().unwrap_or(i32::MAX)),
        ..Default::default()
    });

    result
}

pub(crate) trait BatterySimulationStateObserver {
    fn update_simulation(&self, new_state: bool);
    fn update_simulated_battery_info(&self, battery_info: fpower::BatteryInfo);
}

impl BatterySimulationStateObserver for BatteryManager {
    fn update_simulation(&self, is_simulating: bool) {
        let mut sim_state = self.simulation_state.borrow_mut();
        *sim_state = is_simulating;
        drop(sim_state);
        if !is_simulating {
            self.common_update_watchers(self.get_battery_info_copy(), None);
        }
    }
    fn update_simulated_battery_info(&self, battery_info: fpower::BatteryInfo) {
        self.update_watchers_conditionally(true, battery_info, None);
    }
}

/// Core component for the battery manager system.
///
/// BatteryManager maintains the current state info for the battery system
/// as well as the watchers that share this information with subscribed clients.
///
/// simulation_state: true when the simulator is running
pub struct BatteryManager {
    /// Cached battery info representing the last committed state.
    ///
    /// Used to:
    /// - Send an initial snapshot to newly registered `Watch()` clients.
    /// - Check previous state transitions (e.g. plug/unplug events) when processing new updates.
    /// - Maintain state across simulation mode toggles.
    cached_battery_info: RefCell<fpower::BatteryInfo>,
    watchers: Rc<RefCell<Vec<fpower::BatteryInfoWatcherProxy>>>,
    simulation_state: RefCell<bool>,
    simulated_battery_info: RefCell<fpower::BatteryInfo>,
    data_polisher: RefCell<Polisher>,
    info_recorders: BatteryInfoRecorders,
    /// Blocking suspension if charging
    charge_wake_lease: RefCell<Option<fsystem::LeaseToken>>,

    update_sender: mpsc::Sender<(fpower::BatteryInfo, Option<zx::EventPair>)>,
    _worker_task: fasync::Task<()>,
}

#[inline]
fn get_current_time() -> i64 {
    zx::BootInstant::get().into_nanos()
}

impl BatteryManager {
    #[cfg(test)]
    pub fn new(recorder_config: RecorderConfig) -> BatteryManager {
        Self::new_with_battery_manager_config(
            recorder_config,
            crate::BatteryManagerConfig::default(),
        )
    }

    pub fn new_with_battery_manager_config(
        recorder_config: RecorderConfig,
        battery_manager_config: crate::BatteryManagerConfig,
    ) -> BatteryManager {
        let watchers_rc = Rc::new(RefCell::new(Vec::new()));
        // For now the size is arbitrary chosen. Will log error and catch in CQ.
        let (sender, receiver) = futures::channel::mpsc::channel(10);
        let worker_task = Self::start_watcher_worker(watchers_rc.clone(), receiver);

        BatteryManager {
            cached_battery_info: RefCell::new(fpower::BatteryInfo {
                status: Some(fpower::BatteryStatus::NotAvailable),
                charge_status: Some(fpower::ChargeStatus::Unknown),
                charge_source: Some(fpower::ChargeSource::Unknown),
                level_percent: None,
                level_status: Some(fpower::LevelStatus::Unknown),
                health: Some(fpower::HealthStatus::Unknown),
                time_remaining: Some(fpower::TimeRemaining::Indeterminate(0)),
                timestamp: Some(get_current_time()),
                ..Default::default()
            }),
            watchers: watchers_rc,
            simulation_state: RefCell::new(false),
            simulated_battery_info: RefCell::new(fpower::BatteryInfo {
                status: Some(fpower::BatteryStatus::NotAvailable),
                charge_status: Some(fpower::ChargeStatus::Unknown),
                charge_source: Some(fpower::ChargeSource::Unknown),
                level_percent: None,
                level_status: Some(fpower::LevelStatus::Unknown),
                health: Some(fpower::HealthStatus::Unknown),
                time_remaining: Some(fpower::TimeRemaining::Indeterminate(0)),
                timestamp: Some(get_current_time()),
                ..Default::default()
            }),
            data_polisher: RefCell::new(Polisher::new_with_battery_manager_config(
                battery_manager_config,
            )),
            info_recorders: BatteryInfoRecorders::new(recorder_config),
            charge_wake_lease: RefCell::new(None),
            update_sender: sender,
            _worker_task: worker_task,
        }
    }

    // Global Worker Task (This runs only once)
    fn start_watcher_worker(
        watchers_rc: Rc<RefCell<Vec<fpower::BatteryInfoWatcherProxy>>>,
        mut receiver: mpsc::Receiver<(fpower::BatteryInfo, Option<zx::EventPair>)>,
    ) -> fasync::Task<()> {
        fasync::Task::local(async move {
            // Processes updates sequentially, guaranteeing order.
            while let Some((info, wake_lease)) = receiver.next().await {
                let watchers_to_send = {
                    let mut watchers_guard = watchers_rc.borrow_mut();
                    watchers_guard.retain(|w| !w.is_closed()); // Cleanup of closed channels
                    watchers_guard.clone() // Clone the cleaned list for concurrent sending
                };

                stream::iter(watchers_to_send)
                    .for_each_concurrent(None, |w| {
                        let info_clone = info.clone();
                        let wake_lease_dup = BatteryManager::duplicate_wake_lease(&wake_lease);

                        async move {
                            if let Err(e) =
                                w.on_change_battery_info(&info_clone.into(), wake_lease_dup).await
                            {
                                warn!("failed to send battery info to watcher {:?}", e);
                            }
                        }
                    })
                    .await;
            }
        })
    }

    // Adds watcher
    pub fn add_watcher(&self, watcher: fpower::BatteryInfoWatcherProxy) {
        let mut watchers = self.watchers.borrow_mut();
        debug!("::manager:: adding watcher: {:?} [{:?}]", watcher, watchers.len());
        watchers.push(watcher)
    }

    // Call update_watchers if expecting_simulating == simulation_state.
    // This behavior avoids unnecessary updates.
    fn update_watchers_conditionally(
        &self,
        expect_simulating: bool,
        info: fpower::BatteryInfo,
        wake_lease: Option<zx::EventPair>,
    ) {
        if expect_simulating {
            *self.simulated_battery_info.borrow_mut() = info.clone();
        } else {
            *self.cached_battery_info.borrow_mut() = info.clone();
        }
        if self.is_simulating() == expect_simulating {
            self.common_update_watchers(info, wake_lease);
        }
    }

    pub fn common_update_watchers(
        &self,
        info: fpower::BatteryInfo,
        wake_lease: Option<zx::EventPair>,
    ) {
        debug!("::manager:: update watchers...");
        if let Err(e) = self.update_sender.clone().try_send((info, wake_lease)) {
            log::error!("Failed to send watcher update: {:?}", e);
        }
    }

    async fn determine_suspend_status(
        &self,
        source: Option<fpower::ChargeSource>,
        sag: Option<fsystem::ActivityGovernorProxy>,
    ) {
        let Some(sag) = sag else {
            return;
        };

        let Some(charge_source) = source else {
            return;
        };

        let is_charging = match charge_source {
            fpower::ChargeSource::Unknown | fpower::ChargeSource::None => false,
            _ => true,
        };

        if is_charging && self.charge_wake_lease.borrow().is_none() {
            let res = sag.acquire_unmonitored_wake_lease("charging_block_suspension").await;

            match res {
                Ok(Ok(token)) => {
                    info!("Acquired wake lock to block suspension while charging.");
                    *self.charge_wake_lease.borrow_mut() = Some(token);
                }
                Ok(Err(e)) => {
                    error!("Can't block suspension due to error: {:?}", e);
                }
                Err(e) => {
                    error!("Can't block suspension due to FIDL error {:?}", e);
                }
            }
        }

        if !is_charging && self.charge_wake_lease.borrow().is_some() {
            *self.charge_wake_lease.borrow_mut() = None;
            info!("Dropped wake lease token, allowing suspension.");
        }
    }

    async fn process_battery_info(
        &self,
        info: fpower::BatteryInfo,
        sag: Option<fsystem::ActivityGovernorProxy>,
    ) -> fpower::BatteryInfo {
        let raw_level = info.level_percent;
        let new_charge_status = info.charge_status;
        let recovery_event = self.info_recorders.update(raw_level, new_charge_status);
        self.info_recorders.record_raw_level_on_change(raw_level);

        let old_is_plugged_in = Polisher::is_plugged_in(&self.cached_battery_info.borrow());
        let new_is_plugged_in = Polisher::is_plugged_in(&info);

        let mut info = {
            let mut data_polisher = self.data_polisher.borrow_mut();
            if !old_is_plugged_in && new_is_plugged_in {
                data_polisher.reset_average_current();
            }
            if recovery_event == FaultRecoveryEvent::Recovered {
                data_polisher.reset_rate_limiter();
            }
            data_polisher.polish_info(info)
        };

        self.determine_suspend_status(info.charge_source, sag).await;

        if info.timestamp.is_none() {
            info.timestamp = Some(get_current_time());
        }

        self.publish_to_inspect(&info);
        info
    }

    fn publish_to_inspect(&self, info: &fpower::BatteryInfo) {
        self.info_recorders.record_level_on_change(info);
        self.info_recorders.record_present_voltage(info.present_voltage_mv);
        self.info_recorders.record_remaining_capacity(info.remaining_charge_uah);
        self.info_recorders.record_present_current(info.present_charging_current_ua);
        self.info_recorders.record_average_current(info.average_charging_current_ua);
        self.info_recorders.record_health_on_change(info.health);
        self.info_recorders.record_charge_status_on_change(info.charge_status);
    }

    pub fn get_battery_info_copy(&self) -> fpower::BatteryInfo {
        if *self.simulation_state.borrow() {
            let info_lock = self.simulated_battery_info.borrow();
            (*info_lock).clone()
        } else {
            let info_lock = self.cached_battery_info.borrow();
            (*info_lock).clone()
        }
    }

    pub fn is_simulating(&self) -> bool {
        *self.simulation_state.borrow()
    }

    pub(crate) async fn serve(
        &self,
        stream: fpower::BatteryManagerRequestStream,
    ) -> Result<(), Error> {
        stream
            .try_for_each_concurrent(None, move |request| {
                async move {
                    match request {
                        fpower::BatteryManagerRequest::GetBatteryInfo { responder, .. } => {
                            let info = self.get_battery_info_copy();
                            debug!(
                                info:?;
                                "::battery_manager_request:: handle GetBatteryInfo request"
                            );
                            responder.send(&info)?;
                        }
                        fpower::BatteryManagerRequest::Watch { watcher, .. } => {
                            let watcher = watcher.into_proxy();
                            debug!("::battery_manager_request:: handle Watch request");
                            self.add_watcher(watcher.clone());

                            // Make sure watcher has current battery info.
                            // But there is no copy of the wake lease.
                            let info = self.get_battery_info_copy();
                            debug!(info:?; "::battery_manager_request:: callback on new watcher");
                            watcher.on_change_battery_info(&info, None).await?;
                        }
                    }
                    Ok(())
                }
            })
            .await?;

        Ok(())
    }

    // Called by start_watching_battery_info to process the OnChangeBatteryInfo Call
    async fn wait_on_updates(
        &self,
        watcher: fidl::endpoints::ServerEnd<fpower::BatteryInfoWatcherMarker>,
        sag: Option<fsystem::ActivityGovernorProxy>,
    ) -> Result<(), Error> {
        let mut stream = watcher.into_stream();
        while let Some(event) = stream.try_next().await? {
            match event {
                fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
                    info,
                    wake_lease,
                    responder,
                } => {
                    let info = self.process_battery_info(info, sag.clone()).await;
                    self.update_watchers_conditionally(false, info, wake_lease);
                    responder.send()?;
                }
            }
        }
        Ok(())
    }

    // Main should explicitly call this, so Battery Manager starts to watch the battery info from
    // battery driver, and conditionally dispatches to clients according to simulating state.
    pub(crate) async fn start_watching_battery_info(
        &self,
        source: BatteryInfoSource,
        sag: Option<fsystem::ActivityGovernorProxy>,
    ) -> Result<(), Error> {
        match source {
            BatteryInfoSource::New(proxy) => self.wait_on_new_driver_updates(proxy, sag).await,
            BatteryInfoSource::ModernService(proxy) => {
                self.wait_on_modern_service_updates(proxy, sag).await
            }
        }
    }

    async fn wait_on_new_driver_updates(
        &self,
        proxy: fbattery::BatteryProxy,
        sag: Option<fsystem::ActivityGovernorProxy>,
    ) -> Result<(), Error> {
        info!("Waiting on updates from new fuchsia.hardware.power.battery driver");

        let battery_spec = proxy.get_spec().await.ok().and_then(Result::ok);

        let options = fbattery::WatchOptions {
            // Omitting `interest` defaults to receiving change notifications on any field change.
            wake_on: Some(fbattery::Status {
                present: Some(true),
                charge_status: Some(fbattery::ChargeStatus::Charging),
                level_percent: Some(0.0),
                remaining_capacity_uah: Some(0),
                full_charge_capacity_uah: Some(0),
                health: Some(fbattery::HealthStatus::Good),
                ..Default::default()
            }),
            ..Default::default()
        };

        match proxy.configure_watch(&options).await {
            Ok(Ok(_effective_options)) => {}
            Ok(Err(status)) => {
                warn!("Battery driver returned error configuring watch triggers: {:?}", status);
            }
            Err(e) => {
                warn!("Failed to configure watch triggers on battery driver: {:?}", e);
            }
        }

        let mut current_lease: Option<fsystem::LeaseToken> = if let Some(sag) = &sag {
            match sag.acquire_wake_lease("battery_manager").await {
                Ok(Ok(token)) => {
                    info!("Acquired wake lock for battery manager.");
                    Some(token)
                }
                Ok(Err(e)) => {
                    warn!("Can't acquire wake lock due to error: {:?}", e);
                    None
                }
                Err(e) => {
                    warn!("Can't acquire wake lock due to FIDL error {:?}", e);
                    None
                }
            }
        } else {
            warn!("No ActivityGovernor service available, can't acquire wake lock");
            None
        };
        loop {
            match proxy.watch(current_lease.take()).await {
                Ok(Ok((info, wake_lease))) => {
                    let converted_info = fbattery_to_fpower(info, battery_spec.as_ref());
                    let processed_info =
                        self.process_battery_info(converted_info, sag.clone()).await;

                    // Duplicate the wake lease to notify downstream watchers while retaining a copy
                    // to hand back to the driver on the next Watch call.
                    let watcher_wake_lease = Self::duplicate_wake_lease(&wake_lease);
                    current_lease = wake_lease;

                    self.update_watchers_conditionally(false, processed_info, watcher_wake_lease);
                }
                Ok(Err(e)) => {
                    return Err(anyhow::anyhow!("Battery driver error: {:?}", e));
                }
                Err(e) => {
                    return Err(anyhow::Error::from(e).context("Error in Watch"));
                }
            }
        }
    }
    async fn wait_on_modern_service_updates(
        &self,
        proxy: fpower::BatteryInfoProviderProxy,
        sag: Option<fsystem::ActivityGovernorProxy>,
    ) -> Result<(), Error> {
        info!("Waiting on updates from fuchsia.power.battery service");
        let (client_end, server_end) =
            fidl::endpoints::create_endpoints::<fpower::BatteryInfoWatcherMarker>();
        proxy.watch(client_end)?;

        info!("Waiting on updates from driver");
        let res = self.wait_on_updates(server_end, sag).await;
        warn!("Driver disconnected");

        self.info_recorders.record_disconnected();

        res
    }

    // This function takes a reference to an Option<zx::EventPair>
    // and returns a new Option containing a duplicated handle, or None.
    fn duplicate_wake_lease(wake_lease_ref: &Option<zx::EventPair>) -> Option<zx::EventPair> {
        if let Some(handle_ref) = wake_lease_ref.as_ref() {
            handle_ref.duplicate_handle(zx::Rights::SAME_RIGHTS).ok()
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::battery_info_recorders::PersistenceDirs;
    use async_utils::hanging_get::server::HangingGet;
    use fidl::endpoints::create_request_stream;
    use fuchsia_inspect::{self as inspect};
    use futures::channel::oneshot;
    use futures::future::{join, join3};
    use log::info;
    use std::collections::VecDeque;
    use std::fs;
    use std::sync::Arc;
    use tempfile::{TempDir, tempdir};

    pub fn create_manager() -> (TempDir, BatteryManager) {
        let dir = tempdir().unwrap();
        let storage_path = dir.path().join("data");
        let volatile_path = dir.path().join("tmp");
        fs::create_dir(&storage_path).unwrap();
        fs::create_dir(&volatile_path).unwrap();

        let storage_dir = storage_path.to_str().unwrap().to_string();
        let volatile_dir = volatile_path.to_str().unwrap().to_string();

        let recorder_config = RecorderConfig {
            persistence_dirs: Some(PersistenceDirs { storage_dir, volatile_dir }),
        };
        let battery_manager = BatteryManager::new_with_battery_manager_config(
            recorder_config,
            crate::BatteryManagerConfig { shutdown_offset_percent: 3.0, ..Default::default() },
        );
        (dir, battery_manager)
    }

    #[fuchsia::test]
    async fn test_run_watcher() {
        info!("Starting");
        // To guarantee the code in the fake_watcher gets executed to the end.
        let (tx_signal, rx_signal) = oneshot::channel();

        let (_dir, mut battery_manager) = create_manager();
        let mut battery_info: fpower::BatteryInfo = battery_manager.get_battery_info_copy();
        battery_info.level_percent = Some(50.0);

        let (watcher_client_end, mut stream) =
            create_request_stream::<fpower::BatteryInfoWatcherMarker>();
        let watcher = watcher_client_end.into_proxy();

        battery_manager.add_watcher(watcher.clone());

        // Create a zx::EventPair for the test
        let (tx, rx) = zx::EventPair::create();
        let token_info = tx.basic_info().unwrap();
        let tx_id = token_info.koid;
        let wake_lease = Some(rx); // The rx handle is what we'll pass to the server

        let serve_fut = async move {
            info!("Try_nest");
            let request = stream.try_next().await.unwrap();
            if let Some(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
                info,
                wake_lease: Some(received_lease),
                responder,
            }) = request
            {
                let level = info.level_percent.unwrap().round() as u8;
                assert_eq!(level, 50);

                let token_info = received_lease.basic_info().unwrap();
                let related_id = token_info.related_koid;
                assert_eq!(related_id, tx_id);
                info!("fake watcher ends checking lease");

                responder.send().unwrap();
                let _ = tx_signal.send(());
            } else {
                panic!("Unexpected message received: {:?}", request);
            };
        };
        let request_fut = async {
            info!("Updating watchers");
            battery_manager.common_update_watchers(battery_info, wake_lease);
        };

        join(serve_fut, request_fut).await;
        rx_signal.await.unwrap();
        battery_manager.update_sender.close_channel();
        battery_manager._worker_task.await;
    }

    #[fuchsia::test]
    async fn test_run_watchers_channel_closed() {
        let (_dir, battery_manager) = create_manager();
        let mut battery_info: fpower::BatteryInfo = battery_manager.get_battery_info_copy();
        battery_info.level_percent = Some(50.0);

        let (watcher1_client_end, mut stream1) =
            create_request_stream::<fpower::BatteryInfoWatcherMarker>();
        let watcher1 = watcher1_client_end.into_proxy();

        let (watcher2_client_end, mut stream2) =
            create_request_stream::<fpower::BatteryInfoWatcherMarker>();
        let watcher2 = watcher2_client_end.into_proxy();

        battery_manager.add_watcher(watcher1);
        battery_manager.add_watcher(watcher2);

        let serve1_fut = async move {
            // first request should match first change notification sent
            // at 50%
            let request = stream1.try_next().await.unwrap();
            if let Some(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
                info,
                wake_lease: None,
                responder,
            }) = request
            {
                let level = info.level_percent.unwrap().round() as u8;
                assert_eq!(level, 50);
                responder.send().unwrap();
            } else {
                panic!("Unexpected message received: {:?}", request);
            };
            // second should match subsequent notification at 60%
            let request = stream1.try_next().await.unwrap();
            if let Some(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
                info,
                wake_lease: None,
                responder,
            }) = request
            {
                let level = info.level_percent.unwrap().round() as u8;
                assert_eq!(level, 60);
                responder.send().unwrap();
            } else {
                panic!("Unexpected message received: {:?}", request);
            };
        };

        let serve2_fut = async move {
            // first request should match first change notification sent
            // at 50%
            let request = stream2.try_next().await.unwrap();
            if let Some(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
                info,
                wake_lease: None,
                responder,
            }) = request
            {
                let level = info.level_percent.unwrap().round() as u8;
                assert_eq!(level, 50);
                // but then we drop the channel...
                std::mem::drop(responder);
                std::mem::drop(stream2);
            } else {
                panic!("Unexpected message received: {:?}", request);
            };
        };

        let request_fut = async {
            battery_manager.common_update_watchers(battery_info.clone(), None);
            battery_info.level_percent = Some(60.0);
            battery_manager.common_update_watchers(battery_info, None);
        };

        join3(serve1_fut, serve2_fut, request_fut).await;
    }

    // This function acts as a fake watcher, processing FIDL messages
    fn fake_watcher(
        info_checker: impl Fn(fpower::BatteryInfo) + 'static,
        lease_checker: impl FnOnce(Option<zx::EventPair>) + 'static,
    ) -> fpower::BatteryInfoWatcherProxy {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::BatteryInfoWatcherMarker>();
        fasync::Task::local(async move {
            if let Ok(req) = stream.try_next().await {
                match req {
                    Some(fpower::BatteryInfoWatcherRequest::OnChangeBatteryInfo {
                        info,
                        wake_lease,
                        responder,
                    }) => {
                        info_checker(info);
                        lease_checker(wake_lease);
                        let _ = responder.send();
                    }
                    e => panic!("Unexpected request: {:?}", e),
                }
            }
        })
        .detach();

        proxy
    }

    #[fuchsia::test]
    async fn test_wait_on_updates() {
        // To guarantee the code in the fake_watcher gets executed to the end.
        let (tx_signal, rx_signal) = oneshot::channel();

        // Prepare the wake_lease. For the tx, we need to obtain its koid.
        let (tx, rx) = zx::EventPair::create();
        let token_info = tx.basic_info().unwrap();
        let tx_id = token_info.koid;
        let wake_lease = Some(rx);

        let (_dir, battery_manager) = create_manager();
        let battery_manager = Rc::new(battery_manager);

        // Create a client and server pair for the FIDL call to be used by the pair of
        // wait_on_updates(business logic) and on_change_battery_info(test)
        let (proxy, server_end) =
            fidl::endpoints::create_proxy::<fpower::BatteryInfoWatcherMarker>();

        // Set some battery info, and add a fake watcher.
        let mut updated_info = battery_manager.get_battery_info_copy();
        updated_info.level_percent = Some(100.0);
        updated_info.status = Some(fpower::BatteryStatus::Ok);
        battery_manager.add_watcher(fake_watcher(
            move |info| {
                assert_eq!(info.level_percent, Some(100.0));
                assert_eq!(info.status, Some(fpower::BatteryStatus::Ok));
            },
            move |lease| {
                let lease = lease.expect("Should not be None");
                let token_info = lease.basic_info().unwrap();
                let related_id = token_info.related_koid;
                assert_eq!(related_id, tx_id);
                info!("fake watcher ends checking lease");
                let _ = tx_signal.send(());
            },
        ));

        // The 'server' task: run wait_on_updates in the background
        let battery_clone = battery_manager.clone();
        let server_task = fasync::Task::local(async move {
            battery_clone.clone().wait_on_updates(server_end, None).await
        });

        let client_fut = async move {
            proxy.on_change_battery_info(&updated_info, wake_lease).await.unwrap();
        };

        // Run both the server task and client future concurrently
        let _ = join(server_task, client_fut).await;

        // After the futures complete, check the state of the BatteryManager
        let final_info = battery_manager.get_battery_info_copy();

        // Assert that the state was updated
        assert_eq!(final_info.level_percent, Some(100.0));
        assert_eq!(final_info.status, Some(fpower::BatteryStatus::Ok));

        rx_signal.await.unwrap();
    }

    // This function acts as a fake driver, provide battery info and lease.
    fn fake_driver(
        info: fpower::BatteryInfo,
        lease: Option<zx::EventPair>,
    ) -> fpower::BatteryInfoProviderProxy {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::BatteryInfoProviderMarker>();
        fasync::Task::local(async move {
            while let Ok(req) = stream.try_next().await {
                match req {
                    Some(fpower::BatteryInfoProviderRequest::Watch { watcher, .. }) => {
                        let watcher = watcher.into_proxy();
                        let duplicated_lease = BatteryManager::duplicate_wake_lease(&lease);
                        assert!(
                            watcher.on_change_battery_info(&info, duplicated_lease).await.is_ok()
                        );
                    }
                    e => panic!("Unexpected request: {:?}", e),
                }
            }
        })
        .detach();

        proxy
    }

    #[fuchsia::test]
    async fn test_start_watching_battery_info() -> Result<(), Error> {
        // To guarantee the code in the fake_watcher gets executed to the end.
        let (tx_signal, rx_signal) = oneshot::channel();

        // Prepare the wake_lease. For the tx, we need to obtain its koid.
        let (tx, rx) = zx::EventPair::create();
        let token_info = tx.basic_info()?;
        let tx_id = token_info.koid;
        let wake_lease = Some(rx);

        // Set some battery info, and add a fake watcher.
        let (_dir, battery_manager) = create_manager();
        let mut updated_info = battery_manager.get_battery_info_copy();
        updated_info.level_percent = Some(100.0);
        updated_info.status = Some(fpower::BatteryStatus::Ok);
        updated_info.charge_source = Some(fpower::ChargeSource::Usb);
        updated_info.timestamp = Some(20);

        battery_manager.add_watcher(fake_watcher(
            move |info| {
                assert_eq!(info.level_percent, Some(100.0));
                assert_eq!(info.status, Some(fpower::BatteryStatus::Ok));
                let timestamp = info.timestamp.unwrap();
                assert_eq!(timestamp, 20);
            },
            move |lease| {
                let lease = lease.expect("Should not be None");
                let token_info = lease.basic_info().unwrap();
                let related_id = token_info.related_koid;
                assert_eq!(related_id, tx_id);
                info!("fake watcher ends checking lease");
                let _ = tx_signal.send(());
            },
        ));

        // test start_watching_battery_info
        let _ = battery_manager
            .start_watching_battery_info(
                BatteryInfoSource::ModernService(fake_driver(updated_info, wake_lease)),
                None,
            )
            .await;

        // After the futures complete, check the state of the BatteryManager
        let final_info = battery_manager.get_battery_info_copy();

        // Assert that the state was updated
        assert_eq!(final_info.level_percent, Some(100.0));
        assert_eq!(final_info.status, Some(fpower::BatteryStatus::Ok));

        rx_signal.await.unwrap();
        Ok(())
    }

    // This function acts as a fake sag server and respond with leases from the queue.
    fn fake_sag_vec(
        lease_sequence: Rc<RefCell<VecDeque<fsystem::LeaseToken>>>,
    ) -> fsystem::ActivityGovernorProxy {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fsystem::ActivityGovernorMarker>();
        fasync::Task::local(async move {
            while let Ok(req) = stream.try_next().await {
                match req {
                    Some(fsystem::ActivityGovernorRequest::AcquireUnmonitoredWakeLease {
                        responder,
                        ..
                    }) => {
                        let mut queue = lease_sequence.borrow_mut();
                        let result = queue.pop_front().unwrap();
                        responder.send(Ok(result)).unwrap();
                    }
                    e => panic!("Unexpected request: {:?}", e),
                }
            }
        })
        .detach();

        proxy
    }

    #[fuchsia::test]
    async fn test_block_suspend() {
        info!("Starting");
        let (_dir, battery_manager) = create_manager();
        {
            let charge_wake_lease = battery_manager.charge_wake_lease.borrow();
            assert!(charge_wake_lease.is_none());
        }

        let (tx, rx1) = zx::EventPair::create();
        let token_info = tx.basic_info().unwrap();
        let tx_id1 = token_info.koid;

        let (tx, rx2) = zx::EventPair::create();
        let token_info = tx.basic_info().unwrap();
        let tx_id2 = token_info.koid;

        let vector = vec![rx1, rx2];
        let sag = Some(fake_sag_vec(Rc::new(RefCell::new(VecDeque::from(vector)))));

        battery_manager
            .determine_suspend_status(Some(fpower::ChargeSource::Usb), sag.clone())
            .await;
        {
            let charge_wake_lease = battery_manager.charge_wake_lease.borrow();
            assert!(!charge_wake_lease.is_none());
            let lease_token =
                charge_wake_lease.as_ref().expect("LeaseToken be present inside the RefCell");
            let token_info = lease_token.basic_info().unwrap();
            let related_id = token_info.related_koid;
            assert_eq!(related_id, tx_id1);
        }

        // Call again, and expect the same lease.
        battery_manager
            .determine_suspend_status(Some(fpower::ChargeSource::Usb), sag.clone())
            .await;
        {
            let charge_wake_lease = battery_manager.charge_wake_lease.borrow();
            assert!(!charge_wake_lease.is_none());
            let lease_token =
                charge_wake_lease.as_ref().expect("LeaseToken be present inside the RefCell");
            let token_info = lease_token.basic_info().unwrap();
            let related_id = token_info.related_koid;
            assert_eq!(related_id, tx_id1);
        }

        // Call with no power, and expect the lease dropped.
        battery_manager
            .determine_suspend_status(Some(fpower::ChargeSource::None), sag.clone())
            .await;
        {
            let charge_wake_lease = battery_manager.charge_wake_lease.borrow();
            assert!(charge_wake_lease.is_none());
        }

        // Call again, and expect a new lease.
        battery_manager
            .determine_suspend_status(Some(fpower::ChargeSource::Usb), sag.clone())
            .await;
        {
            let charge_wake_lease = battery_manager.charge_wake_lease.borrow();
            assert!(!charge_wake_lease.is_none());
            let lease_token =
                charge_wake_lease.as_ref().expect("LeaseToken be present inside the RefCell");
            let token_info = lease_token.basic_info().unwrap();
            let related_id = token_info.related_koid;
            assert_eq!(related_id, tx_id2);
        }
    }

    // This function acts as a fake driver for the new fuchsia.hardware.power.battery protocol.
    // It uses the HangingGet server crate to correctly mimic hanging get behavior and avoid
    // tight busy loops in the test. It also takes an optional oneshot sender to signal
    // when the second watch request has been received, allowing the test to synchronize
    // reliably without using flaky timers.
    fn fake_battery_driver_new(
        info: fbattery::Status,
        spec: fbattery::Spec,
        wake_lease: Option<zx::EventPair>,
        on_second_watch: Option<oneshot::Sender<()>>,
    ) -> fbattery::BatteryProxy {
        struct State {
            status: fbattery::Status,
            wake_lease: std::sync::Mutex<Option<zx::EventPair>>,
        }

        let mut hanging_get = HangingGet::new(
            State { status: info.clone(), wake_lease: std::sync::Mutex::new(wake_lease) },
            |state: &State, responder: fbattery::BatteryWatchResponder| {
                let status = &state.status;
                let lease = state.wake_lease.lock().unwrap().take();
                responder.send(Ok((status, lease))).is_ok()
            },
        );

        let publisher = hanging_get.new_publisher();
        let subscriber = hanging_get.new_subscriber();

        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fbattery::BatteryMarker>();
        fasync::Task::local(async move {
            let _publisher = publisher; // Keep alive
            let mut watch_count = 0;
            let mut on_second_watch = on_second_watch;
            while let Ok(Some(req)) = stream.try_next().await {
                match req {
                    fbattery::BatteryRequest::GetSpec { responder } => {
                        let _ = responder.send(Ok(&spec));
                    }
                    fbattery::BatteryRequest::ConfigureWatch { options, responder } => {
                        let _ = responder.send(Ok(&options));
                    }
                    fbattery::BatteryRequest::Watch { lease, responder, .. } => {
                        let _ = lease;
                        if let Err(e) = subscriber.register(responder) {
                            error!("Failed to register watcher: {:?}", e);
                        }
                        watch_count += 1;
                        if watch_count == 2 {
                            if let Some(sender) = on_second_watch.take() {
                                let _ = sender.send(());
                            }
                        }
                    }
                    _ => panic!("Unexpected request"),
                }
            }
        })
        .detach();
        proxy
    }

    #[fuchsia::test]
    async fn test_wait_on_new_driver_updates() {
        let (_dir, battery_manager) = create_manager();

        let (_tx, rx) = zx::EventPair::create();
        let info = fbattery::Status {
            level_percent: Some(100.0),
            charge_status: Some(fbattery::ChargeStatus::Charging),
            current_ua: Some(1000000),
            voltage_uv: Some(4200000),
            ..Default::default()
        };

        let spec = fbattery::Spec { design_capacity_uah: Some(5000000), ..Default::default() };

        let (sender, receiver) = oneshot::channel();
        let proxy = fake_battery_driver_new(info, spec, Some(rx), Some(sender));

        let battery_manager = Arc::new(battery_manager);
        let bm_clone = battery_manager.clone();

        // We need to run wait_on_new_driver_updates and then check if the info was updated.
        // Since it's a loop, we'll run it in a task.
        let _server_task = fasync::Task::local(async move {
            let _ = bm_clone.wait_on_new_driver_updates(proxy, None).await;
        });

        // Wait for the second Watch call to be received, ensuring the first update
        // has been completely processed. This is completely reliable and non-flaky.
        receiver.await.unwrap();

        let battery_info = battery_manager.get_battery_info_copy();
        assert_eq!(battery_info.level_percent, Some(100.0));
        assert_eq!(battery_info.charge_status, Some(fpower::ChargeStatus::Charging));
        assert_eq!(battery_info.status, Some(fpower::BatteryStatus::Ok));
        assert_eq!(battery_info.present_voltage_mv, Some(4200));
        assert_eq!(battery_info.present_charging_current_ua, Some(1000000));

        // Check if battery spec was applied
        assert_eq!(battery_info.battery_spec.unwrap().design_capacity_uah, Some(5000000));
    }

    #[fuchsia::test]
    async fn test_process_battery_info_records_polished_vs_raw() {
        use diagnostics_assertions::assert_data_tree;

        let (_dir, battery_manager) = create_manager();

        // Raw level 3.0 should be polished to 0.0 (by InitialScaler)
        let raw_level = 3.0;
        let info = fpower::BatteryInfo {
            level_percent: Some(raw_level),
            charge_status: Some(fpower::ChargeStatus::Discharging),
            ..Default::default()
        };

        battery_manager.process_battery_info(info.clone(), None).await;

        // Verify recordings in Inspect
        let global_inspector = inspect::component::inspector();
        assert_data_tree!(global_inspector, root: {
            power_observability_state_recorders: contains {
                raw_level_percent: contains {
                    history: contains {
                        shards: contains {
                            "0": contains {
                                values: vec![3u64],
                            }
                        }
                    }
                },
                level_percent: contains {
                    history: contains {
                        shards: contains {
                            "0": contains {
                                values: vec![0u64],
                            }
                        }
                    }
                },
                charge_status: contains {
                    metadata: contains {
                        name: "charge_status",
                        type: "enum",
                    },
                    history: contains {
                        shards: contains {
                            "0": contains {
                                values: vec![2u64],
                            }
                        }
                    }
                },
            }
        });
    }

    #[fuchsia::test]
    async fn test_start_watching_battery_info_driver_disconnected() -> Result<(), Error> {
        use diagnostics_assertions::assert_data_tree;

        let (_dir, battery_manager) = create_manager();

        let (proxy, stream) =
            fidl::endpoints::create_proxy_and_stream::<fpower::BatteryInfoProviderMarker>();

        // Drop the stream immediately to simulate driver disconnect
        drop(stream);

        let _ = battery_manager
            .start_watching_battery_info(BatteryInfoSource::ModernService(proxy), None)
            .await;

        let global_inspector = inspect::component::inspector();
        assert_data_tree!(global_inspector, root: contains {
            power_observability_state_recorders: contains {
                battery_level_fault: contains {
                    history: contains {
                        shards: contains {
                            "0": contains {
                                values: vec![0u64, 2u64],
                            }
                        }
                    }
                }
            }
        });
        Ok(())
    }

    // Note: Must be `async fn` because `create_manager()` spawns an `fasync::Task`,
    // which requires `#[fuchsia::test]` to initialize a Fuchsia async executor.
    #[fuchsia::test]
    async fn test_update_watchers_conditionally_updates_simulated_cache() {
        let (_dir, battery_manager) = create_manager();

        // Ensure we are initially in real mode (not simulating)
        assert_eq!(battery_manager.is_simulating(), false);

        // Call update_watchers_conditionally with expect_simulating = true (simulated update)
        // Even though is_simulating() is false, simulated_battery_info should still be updated.
        let mut simulated_info = fpower::BatteryInfo::default();
        simulated_info.level_percent = Some(42.0);
        battery_manager.update_watchers_conditionally(true, simulated_info, None);
        assert_eq!(battery_manager.simulated_battery_info.borrow().level_percent, Some(42.0));
    }

    // Note: Must be `async fn` because `create_manager()` spawns an `fasync::Task`,
    // which requires `#[fuchsia::test]` to initialize a Fuchsia async executor.
    #[fuchsia::test]
    async fn test_update_watchers_conditionally_updates_real_cache() {
        let (_dir, battery_manager) = create_manager();

        // Switch to simulating mode
        battery_manager.update_simulation(true);
        assert_eq!(battery_manager.is_simulating(), true);

        // Call update_watchers_conditionally with expect_simulating = false (real update)
        // Even though is_simulating() is true, cached_battery_info should still be updated.
        let mut real_info = fpower::BatteryInfo::default();
        real_info.level_percent = Some(84.0);
        battery_manager.update_watchers_conditionally(false, real_info, None);
        assert_eq!(battery_manager.cached_battery_info.borrow().level_percent, Some(84.0));
    }
}
