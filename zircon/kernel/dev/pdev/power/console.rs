// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT
//
// Power and Runtime Processor Power Management (RPPM) Console Commands.
//
// Exposes interactive kernel console commands (`power` and `rppm`) for inspecting
// and controlling system power lifecycle states, CPU performance limits, and
// energy model operating performance points.

use super::{
    PowerCpuState, PowerRebootFlags, rust_power_cpu_off, rust_power_cpu_on,
    rust_power_get_cpu_state, rust_power_opp_get, rust_power_opp_get_domain_count,
    rust_power_opp_set, rust_power_reboot, rust_power_shutdown,
};
use crate::console_rust::console::{CMD_AVAIL_ALWAYS, CMD_AVAIL_NORMAL, CmdArgs, static_command};
use core::ffi::{CStr, c_int};
use debug::dprintf;
use zx_status::Status;

unsafe extern "C" {
    fn cpp_rppm_dump();
    fn cpp_rppm_update_active_power_level(cpu: u32, power_level: u8) -> Result<(), Status>;
    fn cpp_rppm_request_power_level_for_testing(cpu: u32, power_level: u8) -> bool;
    fn cpp_rppm_get_active_power_level(cpu: u32, out_power_level: *mut u8) -> Result<(), Status>;
    fn cpp_rppm_update_processing_limits(
        cpu_mask: u64,
        min_rate: u64,
        max_rate: u64,
    ) -> Result<(), Status>;
    fn cpp_rppm_get_processor_count() -> usize;
}

/// Helper function to safely read a null-terminated C string argument from `CmdArgs`.
///
/// # Safety
///
/// If `arg.arg_str` is non-null, it must point to a valid null-terminated C string.
unsafe fn arg_cstr(arg: &CmdArgs) -> Option<&CStr> {
    if arg.arg_str.is_null() {
        None
    } else {
        // SAFETY: Caller ensures `arg.arg_str` points to a valid null-terminated C string.
        Some(unsafe { CStr::from_ptr(arg.arg_str) })
    }
}

fn rppm_usage(cmd_name: &str) -> c_int {
    dprintf!(ALWAYS, "{cmd_name} dump\n");
    dprintf!(ALWAYS, "{cmd_name} set-level <cpu id> <power level>\n");
    dprintf!(ALWAYS, "{cmd_name} req-level <cpu id> <power level>\n");
    dprintf!(ALWAYS, "{cmd_name} get-level <cpu id>\n");
    dprintf!(ALWAYS, "{cmd_name} set-limit <cpu mask> <min rate> <max rate>\n");
    -1
}

/// Console command handler for `rppm` (Runtime Processor Power Management).
///
/// # Safety
///
/// If `argc > 0`, `argv` must point to a valid array of `CmdArgs` structures of length `argc`.
pub unsafe extern "C" fn rppm_console_cmd(argc: c_int, argv: *const CmdArgs, _flags: u32) -> c_int {
    if argc < 2 || argv.is_null() {
        dprintf!(ALWAYS, "not enough arguments\n");
        let cmd_name = if argc > 0 && !argv.is_null() {
            // SAFETY: Caller guarantees `argv` points to `argc` valid `CmdArgs`.
            let slice = unsafe { core::slice::from_raw_parts(argv, argc as usize) };
            // SAFETY: Kernel console framework guarantees valid null-terminated strings for args.
            unsafe { arg_cstr(&slice[0]) }.and_then(|c| c.to_str().ok()).unwrap_or("rppm")
        } else {
            "rppm"
        };
        return rppm_usage(cmd_name);
    }

    // SAFETY: Verified `argv` is non-null and `argc >= 2`.
    let slice = unsafe { core::slice::from_raw_parts(argv, argc as usize) };
    // SAFETY: Kernel console subsystem provides valid C string arguments.
    let cmd_name = unsafe { arg_cstr(&slice[0]) }.and_then(|c| c.to_str().ok()).unwrap_or("rppm");
    // SAFETY: Kernel console subsystem provides valid C string arguments.
    let subcmd = match unsafe { arg_cstr(&slice[1]) } {
        Some(cstr) => cstr,
        None => {
            dprintf!(ALWAYS, "not enough arguments\n");
            return rppm_usage(cmd_name);
        }
    };

    if subcmd == c"dump" {
        // SAFETY: Dumps registered power domain state to console.
        unsafe {
            cpp_rppm_dump();
        }
        return 0;
    }

    let is_set_level = subcmd == c"set-level";
    let is_req_level = subcmd == c"req-level";
    let is_get_level = subcmd == c"get-level";
    let is_set_limit = subcmd == c"set-limit";

    if !(is_set_level || is_req_level || is_get_level || is_set_limit) {
        dprintf!(ALWAYS, "Unrecognized command\n");
        return rppm_usage(cmd_name);
    }

    if ((is_set_level || is_req_level) && argc < 4)
        || (is_get_level && argc < 3)
        || (is_set_limit && argc < 5)
    {
        dprintf!(ALWAYS, "not enough arguments\n");
        return rppm_usage(cmd_name);
    }

    // SAFETY: Querying CPU topology count from scheduler.
    let processor_count = unsafe { cpp_rppm_get_processor_count() };

    if is_set_limit {
        let cpu_mask_arg = slice[2].arg_uint as u64;
        let max_cpu_mask =
            if processor_count >= 64 { u64::MAX } else { (1u64 << processor_count) - 1 };
        if cpu_mask_arg > max_cpu_mask {
            dprintf!(
                ALWAYS,
                "Invalid cpu mask {:#x}. Valid values are in the range [0, {:#x}].\n",
                cpu_mask_arg,
                max_cpu_mask
            );
            return rppm_usage(cmd_name);
        }
        let min_rate = slice[3].arg_uint as u64;
        let max_rate = slice[4].arg_uint as u64;
        // SAFETY: Invokes processing limit update on scheduler with validated CPU mask.
        let status = unsafe { cpp_rppm_update_processing_limits(cpu_mask_arg, min_rate, max_rate) };
        return Status::result_into_raw(status);
    }

    let cpu_id = slice[2].arg_uint as usize;
    if cpu_id >= processor_count {
        dprintf!(
            ALWAYS,
            "Invalid cpu id {}. Valid values are in the range [0, {}].\n",
            cpu_id,
            processor_count.saturating_sub(1)
        );
        return rppm_usage(cmd_name);
    }

    if is_set_level || is_req_level {
        let level_arg = slice[3].arg_uint;
        if level_arg > (u8::MAX as core::ffi::c_ulong) {
            dprintf!(ALWAYS, "Invalid power level\n");
            return rppm_usage(cmd_name);
        }
        let power_level = level_arg as u8;

        if is_set_level {
            // SAFETY: Updating active power level for validated CPU number.
            let status = unsafe { cpp_rppm_update_active_power_level(cpu_id as u32, power_level) };
            if let Err(status) = status {
                dprintf!(ALWAYS, "Failed to set power level: {status}\n");
            } else {
                dprintf!(ALWAYS, "Set CPU {cpu_id} to power level {power_level}\n");
            }
        } else if is_req_level {
            // SAFETY: Requesting power level for validated CPU number.
            let posted =
                unsafe { cpp_rppm_request_power_level_for_testing(cpu_id as u32, power_level) };
            if posted {
                dprintf!(ALWAYS, "CPU {cpu_id} request for power level {power_level} posted.\n");
            } else {
                dprintf!(ALWAYS, "CPU {cpu_id} request for power level {power_level} ignored.\n");
                let mut current_level = 0u8;
                // SAFETY: Querying active power level with valid stack pointer.
                let status =
                    unsafe { cpp_rppm_get_active_power_level(cpu_id as u32, &mut current_level) };
                if status.is_ok() {
                    dprintf!(ALWAYS, "CPU {cpu_id} at power level {current_level}\n");
                } else {
                    dprintf!(ALWAYS, "CPU {cpu_id} power level not set\n");
                }
            }
        }
    } else if is_get_level {
        let mut power_level = 0u8;
        // SAFETY: Querying active power level with valid stack pointer.
        let status = unsafe { cpp_rppm_get_active_power_level(cpu_id as u32, &mut power_level) };
        if status.is_ok() {
            dprintf!(ALWAYS, "CPU {cpu_id} at power level {power_level}\n");
        } else {
            dprintf!(ALWAYS, "CPU {cpu_id} power level not set\n");
        }
    }

    0
}

fn power_usage(cmd_name: &str) -> c_int {
    dprintf!(ALWAYS, "Usage: {cmd_name} <subcommand>\n");
    dprintf!(ALWAYS, "  reboot [normal|bootloader|recovery|panic] : reboot the system\n");
    dprintf!(ALWAYS, "  shutdown                                 : shut down the system\n");
    dprintf!(ALWAYS, "  cpu-off                                  : turn off calling cpu\n");
    dprintf!(ALWAYS, "  cpu-on <hw_cpu_id>                       : turn on cpu\n");
    dprintf!(ALWAYS, "  cpu-state <hw_cpu_id>                    : query cpu power state\n");
    dprintf!(ALWAYS, "  opp-domains                              : get OPP domain count\n");
    dprintf!(ALWAYS, "  opp-get <domain_id>                      : get OPP level for domain\n");
    dprintf!(ALWAYS, "  opp-set <domain_id> <opp>                : set OPP level for domain\n");
    -1
}

/// Console command handler for low-level PDEV power operations (`power`).
///
/// # Safety
///
/// If `argc > 0`, `argv` must point to a valid array of `CmdArgs` structures of length `argc`.
pub unsafe extern "C" fn power_console_cmd(
    argc: c_int,
    argv: *const CmdArgs,
    _flags: u32,
) -> c_int {
    if argc < 2 || argv.is_null() {
        let cmd_name = if argc > 0 && !argv.is_null() {
            // SAFETY: Caller guarantees `argv` points to `argc` valid `CmdArgs`.
            let slice = unsafe { core::slice::from_raw_parts(argv, argc as usize) };
            // SAFETY: Valid C string argument.
            unsafe { arg_cstr(&slice[0]) }.and_then(|c| c.to_str().ok()).unwrap_or("power")
        } else {
            "power"
        };
        return power_usage(cmd_name);
    }

    // SAFETY: Verified `argv` is non-null and `argc >= 2`.
    let slice = unsafe { core::slice::from_raw_parts(argv, argc as usize) };
    // SAFETY: Valid C string argument.
    let cmd_name = unsafe { arg_cstr(&slice[0]) }.and_then(|c| c.to_str().ok()).unwrap_or("power");
    // SAFETY: Valid C string argument.
    let subcmd = match unsafe { arg_cstr(&slice[1]) } {
        Some(cstr) => cstr,
        None => return power_usage(cmd_name),
    };

    if subcmd == c"reboot" {
        let flags = if argc >= 3 {
            // SAFETY: Valid C string argument for reboot target.
            match unsafe { arg_cstr(&slice[2]) } {
                Some(c) if c == c"bootloader" => PowerRebootFlags::Bootloader,
                Some(c) if c == c"recovery" => PowerRebootFlags::Recovery,
                Some(c) if c == c"panic" => PowerRebootFlags::Panic,
                _ => PowerRebootFlags::Normal,
            }
        } else {
            PowerRebootFlags::Normal
        };
        rust_power_reboot(flags);
        return 0;
    }

    if subcmd == c"shutdown" {
        rust_power_shutdown();
        return 0;
    }

    if subcmd == c"cpu-off" {
        let status = rust_power_cpu_off();
        dprintf!(ALWAYS, "cpu_off returned: {status:?}\n");
        return Status::result_into_raw(status);
    }

    if subcmd == c"cpu-on" {
        if argc < 3 {
            dprintf!(ALWAYS, "Missing hw_cpu_id argument\n");
            return power_usage(cmd_name);
        }
        let hw_cpu_id = slice[2].arg_uint as u64;
        let status = rust_power_cpu_on(hw_cpu_id, 0, 0);
        dprintf!(ALWAYS, "cpu_on({hw_cpu_id}) returned: {status:?}\n");
        return Status::result_into_raw(status);
    }

    if subcmd == c"cpu-state" {
        if argc < 3 {
            dprintf!(ALWAYS, "Missing hw_cpu_id argument\n");
            return power_usage(cmd_name);
        }
        let hw_cpu_id = slice[2].arg_uint as u64;
        let mut state = PowerCpuState::Off;
        // SAFETY: Stack-allocated `state` pointer is valid and aligned.
        let status = unsafe { rust_power_get_cpu_state(hw_cpu_id, &mut state) };
        if status.is_ok() {
            dprintf!(ALWAYS, "CPU {hw_cpu_id} state: {state:?}\n");
        } else if let Err(err) = status {
            dprintf!(ALWAYS, "Failed to get CPU {hw_cpu_id} state: {err}\n");
        }
        return Status::result_into_raw(status);
    }

    if subcmd == c"opp-domains" {
        let mut count = 0usize;
        // SAFETY: Stack-allocated `count` pointer is valid and aligned.
        let status = unsafe { rust_power_opp_get_domain_count(&mut count) };
        if status.is_ok() {
            dprintf!(ALWAYS, "OPP domain count: {count}\n");
        } else if let Err(err) = status {
            dprintf!(ALWAYS, "Failed to get OPP domain count: {err}\n");
        }
        return Status::result_into_raw(status);
    }

    if subcmd == c"opp-get" {
        if argc < 3 {
            dprintf!(ALWAYS, "Missing domain_id argument\n");
            return power_usage(cmd_name);
        }
        let domain_id = slice[2].arg_uint as u32;
        let mut opp = 0u64;
        // SAFETY: Stack-allocated `opp` pointer is valid and aligned.
        let status = unsafe { rust_power_opp_get(domain_id, &mut opp) };
        if status.is_ok() {
            dprintf!(ALWAYS, "Domain {domain_id} active OPP: {opp}\n");
        } else if let Err(err) = status {
            dprintf!(ALWAYS, "Failed to get domain {domain_id} OPP: {err}\n");
        }
        return Status::result_into_raw(status);
    }

    if subcmd == c"opp-set" {
        if argc < 4 {
            dprintf!(ALWAYS, "Missing domain_id or opp arguments\n");
            return power_usage(cmd_name);
        }
        let domain_id = slice[2].arg_uint as u32;
        let opp = slice[3].arg_uint as u64;
        let status = rust_power_opp_set(domain_id, opp);
        if status.is_ok() {
            dprintf!(ALWAYS, "Set domain {domain_id} OPP to {opp}\n");
        } else if let Err(err) = status {
            dprintf!(ALWAYS, "Failed to set domain {domain_id} OPP to {opp}: {err}\n");
        }
        return Status::result_into_raw(status);
    }

    dprintf!(ALWAYS, "Unrecognized command\n");
    power_usage(cmd_name)
}

static_command!(
    RPPM_CMD,
    c"rppm".as_ptr(),
    c"runtime processor power management commands".as_ptr(),
    rppm_console_cmd,
    CMD_AVAIL_ALWAYS
);

static_command!(
    POWER_CMD,
    c"power".as_ptr(),
    c"low-level platform power commands".as_ptr(),
    power_console_cmd,
    CMD_AVAIL_NORMAL
);
