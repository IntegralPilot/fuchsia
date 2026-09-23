// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

//! ACPI initialization and global parser for PC platform.

use handoff::PhysHandoff;
use lazy_init::LazyInit;

#[cfg(console_enabled)]
use crate::console_rust::console::{CMD_AVAIL_ALWAYS, CmdArgs, static_command};

// System-wide ACPI parser.
static GLOBAL_ACPI_PARSER: LazyInit<acpi_lite::AcpiParser<'static>> = LazyInit::uninit();

/// Initializes the system-wide ACPI parser.
fn platform_init_acpi(_level: init::LkInitLevel) {
    let rsdp_pa: u64 = Option::from(PhysHandoff::get().acpi_rsdp).unwrap_or(0);
    // SAFETY: Early boot, single-threaded execution at LK_INIT_LEVEL_VM.
    let parser = match unsafe {
        crate::platform_pc::acpi_lite_zircon::acpi_parser_init(crate::kernel::types::PAddr(
            rsdp_pa as usize,
        ))
    } {
        Ok(p) => p,
        Err(status) => {
            panic!("Could not initialize ACPI. Error code: {:?}.", status);
        }
    };

    // SAFETY: Initialization is serialized with respect to any other access.
    unsafe {
        GLOBAL_ACPI_PARSER.init(parser);
    }
}

init::lk_init_hook!(acpi, platform_init_acpi, init::LK_INIT_LEVEL_VM);

/// Returns a reference to the global `AcpiParser` instance.
///
/// # Panics
///
/// Panics if ACPI initialization has not occurred yet.
pub fn global_acpi_lite_parser() -> &'static acpi_lite::AcpiParser<'static> {
    GLOBAL_ACPI_PARSER.get()
}

#[cfg(console_enabled)]
unsafe extern "C" fn cmd_acpidump(_argc: i32, _argv: *const CmdArgs, _flags: u32) -> i32 {
    let parser = global_acpi_lite_parser();
    parser.dump_tables();
    0
}

#[cfg(console_enabled)]
static_command!(
    CMD_ACPIDUMP,
    c"acpidump".as_ptr(),
    c"dump ACPI tables to console".as_ptr(),
    cmd_acpidump,
    CMD_AVAIL_ALWAYS
);
