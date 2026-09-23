// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#[path = "../../lib/acpi_lite/rust/zircon/src/mod.rs"]
pub mod acpi_lite_zircon;

pub mod acpi;
pub mod cmos_bootbyte;
pub mod hpet;
pub mod interrupt_manager;
pub mod interrupts;
pub mod keyboard;
pub mod pic;
pub mod smbios;
