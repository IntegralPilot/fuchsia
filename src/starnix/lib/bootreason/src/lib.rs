// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use async_lock::OnceCell;
use fidl_fuchsia_feedback::{LastRebootInfoProviderMarker, RebootReason};
use fidl_fuchsia_io as fio;
use fuchsia_component::client::connect_to_protocol_sync;
use fuchsia_fs::node::OpenError;
use log::{debug, info};
use zx_status::Status;

/// Temp file for the Starnix lifecycle detection
const STARTED_ONCE: &str = "component-started-once";
/// Starnix session restart indicator.
/// True if the current Starnix session was restarted without a full reboot.
static HAS_STARNIX_SESSION_RESTARTED: OnceCell<bool> = OnceCell::new();
/// The Android boot reason of the current session.
static ANDROID_BOOTREASON: OnceCell<Result<String, Error>> = OnceCell::new();

/// Timeout for FIDL calls to LastRebootInfoProvider
const LRIP_FIDL_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::INFINITE;

/// Determines whether the current session was restarted without a reboot.
/// Returns true if the session was restarted, false if it is the initial session.
async fn has_session_restarted(dir: Option<fio::DirectoryProxy>) -> bool {
    match dir {
        Some(dir) => {
            match fuchsia_fs::directory::open_file(&dir, STARTED_ONCE, fio::Flags::FLAG_MUST_CREATE)
                .await
            {
                Ok(_file) => false,
                Err(OpenError::OpenError(Status::ALREADY_EXISTS)) => true,
                Err(err) => {
                    info!("Failed to generate the file with err {err:#?}.");
                    false
                }
            }
        }
        None => false,
    }
}

/// Get an Android-compatible boot reason suitable to add to the cmdline or bootconfig.
pub async fn get_or_init_android_bootreason(
    dir: Option<fio::DirectoryProxy>,
    android_provided_bootreason: Option<String>,
) -> &'static Result<String, Error> {
    ANDROID_BOOTREASON
        .get_or_init(async || update_android_bootreason(dir, android_provided_bootreason).await)
        .await
}

/// Update the Android bootreason.
/// Use get_or_init_android_bootreason to get the cached Android boot reason instead of this.
pub async fn update_android_bootreason(
    dir: Option<fio::DirectoryProxy>,
    android_provided_bootreason: Option<String>,
) -> Result<String, Error> {
    // Set the Android bootreason to kernel_panic if the current session was restarted.
    if *HAS_STARNIX_SESSION_RESTARTED.get_or_init(async || has_session_restarted(dir).await).await {
        info!("Session restart observed, set android bootreason to kernel_panic.");
        return Ok("kernel_panic".to_string());
    }

    // There are certain values from the Android bootloader that are more specific than
    // what the Fuchsia platform knows so use that when relevant.
    if let Some(reason) = &android_provided_bootreason {
        if reason.starts_with("reboot,uvlo") || reason.starts_with("reboot,longkey") {
            return Ok(reason.clone());
        }
    }

    info!("Converting LastRebootInfo to an android-friendly bootreason.");
    let reboot_info_proxy = connect_to_protocol_sync::<LastRebootInfoProviderMarker>()?;
    let deadline = zx::MonotonicInstant::after(LRIP_FIDL_TIMEOUT);
    let reboot_info = reboot_info_proxy.get(deadline)?;

    let bootreason = match reboot_info.reason {
        Some(RebootReason::Unknown) => "reboot,unknown",
        Some(RebootReason::Cold) => "reboot,cold",
        Some(RebootReason::BriefPowerLoss) => "reboot,hard_reset",
        Some(RebootReason::Brownout) => "reboot,undervoltage",
        Some(RebootReason::KernelPanic) => "kernel_panic",
        Some(RebootReason::SystemOutOfMemory) => "kernel_panic,oom",
        Some(RebootReason::HardwareWatchdogTimeout) => "watchdog",
        Some(RebootReason::SoftwareWatchdogTimeout) => "watchdog,sw",
        Some(RebootReason::SuspensionFailure) => "kernel_panic",
        Some(RebootReason::RootJobTermination) => "kernel_panic",
        Some(RebootReason::UserRequest) => "reboot,userrequested",
        Some(RebootReason::UserRequestDeviceStuck) => "reboot,userrequested",
        Some(RebootReason::UserHardReset) => "reboot,longkey,s2",
        Some(RebootReason::DeveloperRequest) => "reboot,shell",
        Some(RebootReason::RetrySystemUpdate) => "reboot,ota",
        Some(RebootReason::HighTemperature) => "shutdown,thermal",
        Some(RebootReason::SessionFailure) => "kernel_panic",
        Some(RebootReason::SysmgrFailure) => "kernel_panic",
        Some(RebootReason::FactoryDataReset) => "reboot,factory_reset",
        Some(RebootReason::CriticalComponentFailure) => "kernel_panic",
        Some(RebootReason::CriticalDriverFailure) => "kernel_panic",
        Some(RebootReason::ZbiSwap) => "reboot,normal",
        Some(RebootReason::SystemUpdate) => "reboot,ota",
        Some(RebootReason::NetstackMigration) => "reboot,normal",
        Some(RebootReason::AndroidUnexpectedReason) => "reboot,normal",
        Some(RebootReason::AndroidNoReason) => "reboot",
        Some(RebootReason::AndroidRescueParty) => "reboot,rescueparty",
        Some(RebootReason::AndroidCriticalProcessFailure) => "reboot,userspace_failed",
        Some(RebootReason::BatteryDrained) => "shutdown,battery",
        Some(RebootReason::__SourceBreaking { .. }) => "reboot,normal",
        None => "reboot,unknown",
    };
    Ok(bootreason.to_string())
}

/// Get the last reboot reason code.
fn get_reboot_reason() -> Option<RebootReason> {
    let reboot_info_proxy = connect_to_protocol_sync::<LastRebootInfoProviderMarker>().ok();
    let deadline = zx::MonotonicInstant::after(LRIP_FIDL_TIMEOUT);
    let reboot_info = reboot_info_proxy?.get(deadline);
    match reboot_info {
        Ok(info) => match info.reason {
            Some(r) => Some(r),
            None => {
                info!("Failed to get the reboot reason.");
                Some(RebootReason::unknown())
            }
        },
        Err(e) => {
            info!("Failed to get the reboot info: {:?}", e);
            Some(RebootReason::unknown())
        }
    }
}

/// Get contents for the pstore/console-ramoops* file.
///
/// In Linux it contains a limited amount of some of the previous boot's kernel logs.
/// The ramoops won't be created after a normal reboot.
pub fn get_console_ramoops() -> Option<Vec<u8>> {
    debug!("Getting console-ramoops contents");
    if HAS_STARNIX_SESSION_RESTARTED.get().copied().unwrap_or(false) {
        return Some(format!("Last Reboot Reason: Starnix Crash\n").as_bytes().to_vec());
    }
    match ANDROID_BOOTREASON.get() {
        Some(Ok(reason)) => match reason.as_str() {
            "kernel_panic" | "watchdog" | "watchdog,sw" => Some(
                format!("Last Reboot Reason: {:?}\n", get_reboot_reason()?).as_bytes().to_vec(),
            ),
            _ => None,
        },
        Some(Err(e)) => {
            info!("Failed to get android bootreason for console_ramoops: {:?}", e);
            None
        }
        None => {
            info!("Android bootreason not initialized.");
            None
        }
    }
}
