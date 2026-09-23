// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::{InterruptPolarity, InterruptTriggerMode};
use crate::console_rust::console::{CMD_AVAIL_NORMAL, CmdArgs, static_command};
use core::ffi::{CStr, c_int};
use debug::dprintf;
use zx_status::Status;

#[cfg(target_arch = "aarch64")]
const FIRST_VALID_IRQ: u32 = 32;
#[cfg(not(target_arch = "aarch64"))]
const FIRST_VALID_IRQ: u32 = 0;
const LAST_VALID_IRQ: u32 = super::MAX_INTERRUPTS as u32;

struct IrqInfo {
    registered: bool,
    trigger_mode: Option<InterruptTriggerMode>,
    polarity: Option<InterruptPolarity>,
    pending: Option<bool>,
    enabled: Option<bool>,
}

impl IrqInfo {
    #[allow(clippy::absurd_extreme_comparisons)]
    fn valid_irq_num(irq_num: u32) -> bool {
        (FIRST_VALID_IRQ..LAST_VALID_IRQ).contains(&irq_num)
    }

    fn get(irq_num: u32) -> Self {
        let (tm, pol) = super::query_interrupt_config(irq_num);
        let (pending, enabled) = super::query_interrupt_status(irq_num);
        let registered = super::is_interrupt_registered(irq_num);

        Self { registered, trigger_mode: tm, polarity: pol, pending, enabled }
    }

    fn is_valid(&self) -> bool {
        self.trigger_mode.is_some() && self.polarity.is_some()
    }

    fn trigger_mode_str(&self) -> &'static str {
        match self.trigger_mode {
            Some(InterruptTriggerMode::Edge) => "Edge",
            Some(InterruptTriggerMode::Level) => "Level",
            None => "Unknown",
        }
    }

    fn polarity_str(&self) -> &'static str {
        match (self.trigger_mode, self.polarity) {
            (Some(InterruptTriggerMode::Level), Some(InterruptPolarity::High)) => "High",
            (Some(InterruptTriggerMode::Level), Some(InterruptPolarity::Low)) => "Low",
            (Some(InterruptTriggerMode::Edge), Some(InterruptPolarity::High)) => "Rising",
            (Some(InterruptTriggerMode::Edge), Some(InterruptPolarity::Low)) => "Falling",
            _ => "Unknown",
        }
    }

    fn pending_str(&self) -> &'static str {
        match self.pending {
            Some(true) => "True",
            Some(false) => "False",
            None => "Unknown",
        }
    }

    fn enabled_str(&self) -> &'static str {
        match self.enabled {
            Some(true) => "True",
            Some(false) => "False",
            None => "Unknown",
        }
    }
}

fn usage(ret: Result<(), Status>) -> c_int {
    dprintf!(ALWAYS, "Usage: irq <cmd>\n");
    dprintf!(ALWAYS, "Valid commands are:\n");
    dprintf!(ALWAYS, "  help : show this message\n");
    dprintf!(ALWAYS, "  show (all | reg | pending | enabled | <N>)\n");
    dprintf!(
        ALWAYS,
        "    Shows the status of one or more IRQs, depending on the mandatory argument.\n"
    );
    dprintf!(ALWAYS, "      all     : Show the status of all IRQs in the system.\n");
    dprintf!(
        ALWAYS,
        "      reg     : Show the status of all IRQs which have a registered handler.\n"
    );
    dprintf!(ALWAYS, "      pending : Show the status of all IRQs which are currently pending.\n");
    dprintf!(ALWAYS, "      enabled : Show the status of all IRQs which are currently enabled.\n");
    dprintf!(
        ALWAYS,
        "      pendenb : Show the status of all IRQs which are currently pending OR enabled.\n"
    );
    dprintf!(ALWAYS, "      <N>     : Show the status IRQ #<N>.\n");
    Status::result_into_raw(ret)
}

unsafe fn arg_cstr(arg: &CmdArgs) -> Option<&CStr> {
    if arg.arg_str.is_null() { None } else { Some(unsafe { CStr::from_ptr(arg.arg_str) }) }
}

/// Console command handler for `irq`.
///
/// # Safety
///
/// If `argc > 0`, `argv` must point to a valid array of `CmdArgs` structures of length `argc`.
pub unsafe extern "C" fn irq_console_cmd(argc: c_int, argv: *const CmdArgs, _flags: u32) -> c_int {
    if argc < 2 || argv.is_null() {
        dprintf!(ALWAYS, "No command specified!\n");
        return usage(Err(Status::INVALID_ARGS));
    }

    let slice = unsafe { core::slice::from_raw_parts(argv, argc as usize) };
    let subcmd = match unsafe { arg_cstr(&slice[1]) } {
        Some(cstr) => cstr,
        None => {
            dprintf!(ALWAYS, "No command specified!\n");
            return usage(Err(Status::INVALID_ARGS));
        }
    };

    if subcmd == c"help" {
        return usage(Ok(()));
    }

    if subcmd != c"show" {
        let subcmd_str = subcmd.to_str().unwrap_or("?");
        dprintf!(ALWAYS, "Unrecognized command \"{}\"\n", subcmd_str);
        return usage(Err(Status::INVALID_ARGS));
    }

    if argc < 3 {
        dprintf!(ALWAYS, "No target specified for \"show\"\n");
        return usage(Err(Status::INVALID_ARGS));
    }

    let target_arg = match unsafe { arg_cstr(&slice[2]) } {
        Some(cstr) => cstr,
        None => {
            dprintf!(ALWAYS, "No target specified for \"show\"\n");
            return usage(Err(Status::INVALID_ARGS));
        }
    };

    if target_arg == c"all"
        || target_arg == c"reg"
        || target_arg == c"pending"
        || target_arg == c"enabled"
        || target_arg == c"pendenb"
    {
        dprintf!(ALWAYS, "  ID  | Registered | Trigger Mode | Polarity | Pending | Enabled |\n");
        dprintf!(ALWAYS, "------+------------+--------------+----------+---------+---------+\n");
        for i in FIRST_VALID_IRQ..LAST_VALID_IRQ {
            let info = IrqInfo::get(i);
            if !info.is_valid() {
                continue;
            }
            let matches = if target_arg == c"all" {
                true
            } else if target_arg == c"reg" {
                info.registered
            } else if target_arg == c"pending" {
                info.pending.unwrap_or(false)
            } else if target_arg == c"enabled" {
                info.enabled.unwrap_or(false)
            } else if target_arg == c"pendenb" {
                info.pending.unwrap_or(false) || info.enabled.unwrap_or(false)
            } else {
                false
            };

            if matches {
                dprintf!(
                    ALWAYS,
                    " {:4} |        {:3} |      {:7} |  {:7} | {:7} | {:7} |\n",
                    i,
                    if info.registered { "Yes" } else { "No" },
                    info.trigger_mode_str(),
                    info.polarity_str(),
                    info.pending_str(),
                    info.enabled_str()
                );
            }
        }
        zx_status::sys::ZX_OK
    } else {
        let target_num = slice[2].arg_uint as u32;
        if !IrqInfo::valid_irq_num(target_num) {
            let target_str = target_arg.to_str().unwrap_or("?");
            dprintf!(ALWAYS, "Invalid IRQ target (\"{}\") for \"show\".\n", target_str);
            return usage(Err(Status::INVALID_ARGS));
        }
        let info = IrqInfo::get(target_num);
        dprintf!(ALWAYS, "IRQ          : {}\n", target_num);
        dprintf!(
            ALWAYS,
            "State        : {}\n",
            if info.registered { "Registered" } else { "Unregistered" }
        );
        dprintf!(ALWAYS, "Trigger Mode : {}\n", info.trigger_mode_str());
        dprintf!(ALWAYS, "Polarity     : {}\n", info.polarity_str());
        dprintf!(ALWAYS, "Pending      : {}\n", info.pending_str());
        dprintf!(ALWAYS, "Enabled      : {}\n", info.enabled_str());
        zx_status::sys::ZX_OK
    }
}

static_command!(
    IRQ_CMD,
    c"irq".as_ptr(),
    c"IRQ related commands".as_ptr(),
    irq_console_cmd,
    CMD_AVAIL_NORMAL
);
