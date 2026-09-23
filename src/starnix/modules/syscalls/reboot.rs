// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;

use bstr::ByteSlice;
use fidl_fuchsia_hardware_power_statecontrol as fpower;
use fuchsia_component::client::connect_to_protocol;
use linux_uapi::{
    LINUX_REBOOT_CMD_CAD_OFF, LINUX_REBOOT_CMD_CAD_ON, LINUX_REBOOT_CMD_HALT,
    LINUX_REBOOT_CMD_KEXEC, LINUX_REBOOT_CMD_POWER_OFF, LINUX_REBOOT_CMD_RESTART,
    LINUX_REBOOT_CMD_RESTART2, LINUX_REBOOT_CMD_SW_SUSPEND,
};
use starnix_core::mm::MemoryAccessorExt;
use starnix_core::security;
use starnix_core::task::{CurrentTask, ExitStatus};
use starnix_core::vfs::FsString;
use starnix_logging::{log_debug, log_error, log_info, log_warn, track_stub};
use starnix_sync::{InterruptibleEvent, Mutex};
use starnix_uapi::auth::CAP_SYS_BOOT;
use starnix_uapi::errors::{EINTR, Errno};
use starnix_uapi::signals::SigSet;
use starnix_uapi::user_address::{UserAddress, UserCString};
use starnix_uapi::{
    LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2, LINUX_REBOOT_MAGIC2A, LINUX_REBOOT_MAGIC2B,
    LINUX_REBOOT_MAGIC2C, errno, error,
};

pub fn sys_reboot(
    current_task: &mut CurrentTask,
    magic: u32,
    magic2: u32,
    cmd: u32,
    arg: UserAddress,
) -> Result<(), Errno> {
    if magic != LINUX_REBOOT_MAGIC1
        || (magic2 != LINUX_REBOOT_MAGIC2
            && magic2 != LINUX_REBOOT_MAGIC2A
            && magic2 != LINUX_REBOOT_MAGIC2B
            && magic2 != LINUX_REBOOT_MAGIC2C)
    {
        return error!(EINVAL);
    }
    security::check_task_capable(current_task, CAP_SYS_BOOT)?;

    let arg_bytes = if matches!(cmd, LINUX_REBOOT_CMD_RESTART2)
        || (matches!(cmd, LINUX_REBOOT_CMD_POWER_OFF) && !arg.is_null())
    {
        // This is an arbitrary limit that should be large enough.
        const MAX_REBOOT_ARG_LEN: usize = 256;
        current_task
            .read_c_string_to_vec(UserCString::new(current_task, arg), MAX_REBOOT_ARG_LEN)?
    } else {
        FsString::default()
    };

    if current_task.kernel().is_shutting_down() {
        log_debug!("Ignoring reboot() and parking caller, already shutting down.");
        let event = InterruptibleEvent::new();
        return current_task.block_until(event.begin_wait(), zx::MonotonicInstant::INFINITE);
    }

    match cmd {
        // CAD on/off commands turn Ctrl-Alt-Del keystroke on or off without halting the system.
        LINUX_REBOOT_CMD_CAD_ON | LINUX_REBOOT_CMD_CAD_OFF => Ok(()),

        // `kexec_load()` is not supported.
        LINUX_REBOOT_CMD_KEXEC => error!(EINVAL),

        // Suspend is not implemented.
        LINUX_REBOOT_CMD_SW_SUSPEND => error!(EINVAL),

        LINUX_REBOOT_CMD_HALT | LINUX_REBOOT_CMD_POWER_OFF => {
            log_info!("Powering off");
            let reboot_args: Vec<_> = arg_bytes.split_str(b",").collect();
            let shutdown_reason = parse_shutdown_reason(&reboot_args, &arg_bytes);
            let options = fpower::ShutdownOptions {
                action: Some(fpower::ShutdownAction::Poweroff),
                reasons: Some(vec![shutdown_reason]),
                ..Default::default()
            };
            shutdown_and_block(current_task, options, "sys_reboot_poweroff")
        }

        LINUX_REBOOT_CMD_RESTART | LINUX_REBOOT_CMD_RESTART2 => {
            let reboot_args: Vec<_> = arg_bytes.split_str(b",").collect();

            let options = if reboot_args.contains(&&b"bootloader"[..]) {
                log_info!("Rebooting to bootloader");
                fpower::ShutdownOptions {
                    action: Some(fpower::ShutdownAction::RebootToBootloader),
                    reasons: Some(vec![fpower::ShutdownReason::StarnixContainerNoReason]),
                    ..Default::default()
                }
            } else if reboot_args.contains(&&b"recovery"[..]) {
                log_info!("Rebooting to recovery...");
                fpower::ShutdownOptions {
                    action: Some(fpower::ShutdownAction::RebootToRecovery),
                    reasons: Some(vec![fpower::ShutdownReason::StarnixContainerNoReason]),
                    ..Default::default()
                }
            } else {
                let shutdown_reason = parse_shutdown_reason(&reboot_args, &arg_bytes);
                log_info!("Rebooting... reason: {:?}", shutdown_reason);
                fpower::ShutdownOptions {
                    action: Some(fpower::ShutdownAction::Reboot),
                    reasons: Some(vec![shutdown_reason]),
                    ..Default::default()
                }
            };
            shutdown_and_block(current_task, options, "sys_reboot_reboot")
        }

        _ => error!(EINVAL),
    }
}

fn shutdown_and_block(
    current_task: &mut CurrentTask,
    options: fpower::ShutdownOptions,
    debug_name: &'static str,
) -> Result<(), Errno> {
    let event = InterruptibleEvent::new();
    let event_clone = event.clone();
    let error_state = Arc::new(Mutex::new(None));
    let error_state_clone = error_state.clone();

    let kernel = current_task.kernel().clone();
    kernel.kthreads.spawn_future(
        move || async move {
            match connect_to_protocol::<fpower::AdminMarker>() {
                Ok(proxy) => match proxy.shutdown(&options).await {
                    Ok(Ok(())) => {}
                    Ok(Err(status)) => {
                        log_error!(
                            "sys_reboot: proxy.shutdown async ({debug_name}) returned error: {}",
                            zx::Status::err_from_raw(status)
                        );
                        error_state_clone.lock().replace(errno!(EIO));
                        event_clone.notify();
                    }
                    Err(e) => {
                        log_error!("sys_reboot: proxy.shutdown async ({debug_name}) failed: {e:?}");
                        error_state_clone.lock().replace(errno!(EIO));
                        event_clone.notify();
                    }
                },
                Err(e) => {
                    log_error!(
                        "sys_reboot: failed to connect to Admin async ({debug_name}): {e:?}"
                    );
                    error_state_clone.lock().replace(errno!(ENOTSUP));
                    event_clone.notify();
                }
            }
        },
        debug_name,
    );

    // Mask all maskable signals for the current task before blocking so that
    // normal signals do not wake up the task and return EINTR to userspace.
    // Unmaskable signals like SIGKILL (sent during container teardown) will
    // still interrupt the block and cause the task to exit without returning to userspace.
    let result = current_task.wait_with_temporary_mask(!SigSet::default(), |current_task| {
        current_task.block_until(event.begin_wait(), zx::MonotonicInstant::INFINITE)
    });

    if let Some(err) = error_state.lock().take() {
        return Err(err);
    }

    if let Err(err) = result {
        if err.code == EINTR {
            // Linux expects `sys_reboot` to never return. If we return `Err(EINTR)`
            // after receiving `SIGKILL` during container teardown, Android `init`
            // will abort and re-enter `reboot()`, causing a deadlock that blocks
            // component teardown from concluding inside Starnix.
            // Thus, we manually terminate the thread group.
            current_task.thread_group().kill(ExitStatus::Exit(0), None);
            return Ok(());
        }

        return Err(err);
    }

    Ok(())
}

fn parse_shutdown_reason(reboot_args: &[&[u8]], arg_bytes: &FsString) -> fpower::ShutdownReason {
    // TODO(https://fxbug.dev/391585107): Loop through all the arguments and
    // generate a list of shutdown reasons.
    if let Some(arg) = reboot_args.iter().find(|arg| arg.ends_with(b"-failed")) {
        let process_name = String::from_utf8_lossy(arg.strip_suffix(b"-failed").unwrap());
        // This log message is load-bearing server-side as it's used to
        // extract the critical process responsible for the reboot.
        // Please notify //src/developer/forensics/OWNERS upon changing.
        log_info!("Android critical process '{}' failed, rebooting", process_name);
        fpower::ShutdownReason::AndroidCriticalProcessFailure
    } else if reboot_args.contains(&&b"ota_update"[..])
        || reboot_args.contains(&&b"System update during setup"[..])
    {
        fpower::ShutdownReason::SystemUpdate
    } else if reboot_args.contains(&&b"shell"[..]) {
        fpower::ShutdownReason::DeveloperRequest
    } else if reboot_args.contains(&&b"RescueParty"[..])
        || reboot_args.contains(&&b"rescueparty"[..])
    {
        fpower::ShutdownReason::AndroidRescueParty
    } else if reboot_args.contains(&&b"userrequested"[..]) {
        fpower::ShutdownReason::UserRequest
    } else if reboot_args.contains(&&b"thermal"[..]) {
        fpower::ShutdownReason::HighTemperature
    } else if reboot_args.contains(&&b"battery"[..]) {
        fpower::ShutdownReason::BatteryDrained
    } else if reboot_args == [b""]
    // args empty? splitting "" returns [""], not []
    {
        fpower::ShutdownReason::StarnixContainerNoReason
    } else {
        log_warn!("Unknown reboot args: {arg_bytes:?}");
        track_stub!(
            TODO("https://fxbug.dev/322874610"),
            "unknown reboot args, see logs for strings"
        );
        fpower::ShutdownReason::AndroidUnexpectedReason
    }
}

#[cfg(target_arch = "aarch64")]
mod arch32 {
    pub use super::sys_reboot as sys_arch32_reboot;
}

#[cfg(target_arch = "aarch64")]
pub use arch32::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn check_parse_shutdown_reason(args: &str, expected: fpower::ShutdownReason) {
        let bytes = FsString::from(args.as_bytes().to_vec());
        let split_args: Vec<_> = bytes.split_str(b",").collect();
        let reason = super::parse_shutdown_reason(&split_args, &bytes);
        assert_eq!(reason, expected, "Failed for args: {}", args);
    }

    #[test]
    fn parse_shutdown_reason_thermal() {
        check_parse_shutdown_reason("shutdown,thermal", fpower::ShutdownReason::HighTemperature);
    }

    #[test]
    fn parse_shutdown_reason_battery() {
        check_parse_shutdown_reason("shutdown,battery", fpower::ShutdownReason::BatteryDrained);
    }

    #[test]
    fn parse_shutdown_reason_thermal_and_battery() {
        check_parse_shutdown_reason(
            "shutdown,thermal,battery",
            fpower::ShutdownReason::HighTemperature,
        );
    }
}
