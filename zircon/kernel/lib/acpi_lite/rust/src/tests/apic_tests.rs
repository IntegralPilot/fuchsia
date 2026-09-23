// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![cfg(test)]

extern crate std;

use crate::structures::{ACPI_MADT_FLAG_POLARITY_HIGH, ACPI_MADT_FLAG_TRIGGER_MASK};
use crate::test_util::{pixelbook_eve_acpi_parser, z840_acpi_parser};

#[test]
fn test_enumerate_eve_cpus() {
    let mut items = std::vec::Vec::new();
    let parser = pixelbook_eve_acpi_parser();
    crate::apic::enumerate_processor_local_apics(&parser, |item| {
        items.push(*item);
        Ok(())
    })
    .unwrap();

    assert_eq!(items.len(), 4);
    assert_eq!(items[0].apic_id, 0);
    assert_eq!(items[1].apic_id, 1);
    assert_eq!(items[2].apic_id, 2);
    assert_eq!(items[3].apic_id, 3);
}

#[test]
fn test_enumerate_z840_cpus() {
    let mut items = std::vec::Vec::new();
    let parser = z840_acpi_parser();
    crate::apic::enumerate_processor_local_apics(&parser, |item| {
        items.push(*item);
        Ok(())
    })
    .unwrap();

    assert_eq!(items.len(), 56);
}

#[test]
fn test_enumerate_eve_io() {
    let mut items = std::vec::Vec::new();
    let parser = pixelbook_eve_acpi_parser();
    crate::apic::enumerate_io_apics(&parser, |item| {
        items.push(*item);
        Ok(())
    })
    .unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].io_apic_id, 2);
    let global_system_interrupt_base = items[0].global_system_interrupt_base;
    assert_eq!(global_system_interrupt_base, 0);
    let io_apic_address = items[0].io_apic_address;
    assert_eq!(io_apic_address, 0xFEC00000);
}

#[test]
fn test_enumerate_interrupt_overrides() {
    let mut items = std::vec::Vec::new();
    let parser = pixelbook_eve_acpi_parser();
    crate::apic::enumerate_io_apic_isa_overrides(&parser, |item| {
        items.push(*item);
        Ok(())
    })
    .unwrap();

    assert_eq!(items.len(), 2);

    assert_eq!(items[0].source, 0);
    let flags = items[0].flags;
    assert_eq!(flags, 0);
    let global_sys_interrupt = items[0].global_sys_interrupt;
    assert_eq!(global_sys_interrupt, 2);

    assert_eq!(items[1].source, 9);
    // C++ code had: ACPI_MADT_FLAG_TRIGGER_MASK | ACPI_MADT_FLAG_POLARITY_HIGH
    // Wait, in structures.h:
    // #define ACPI_MADT_FLAG_TRIGGER_LEVEL 0b1100
    // In apic_test.cc:
    // EXPECT_EQ_PACKED(ACPI_MADT_FLAG_TRIGGER_MASK | ACPI_MADT_FLAG_POLARITY_HIGH, collector.items[1].flags);
    // Wait, in my apic_tests.rs I use ACPI_MADT_FLAG_TRIGGER_LEVEL | ACPI_MADT_FLAG_POLARITY_HIGH ?
    // Actually, ACPI_MADT_FLAG_TRIGGER_MASK is 0b1100, which is same as ACPI_MADT_FLAG_TRIGGER_LEVEL.
    // In my structures.rs:
    // pub const ACPI_MADT_FLAG_TRIGGER_LEVEL: u16 = 0b1100;
    // pub const ACPI_MADT_FLAG_TRIGGER_MASK: u16 = 0b1100;
    let flags = items[1].flags;
    assert_eq!(flags, ACPI_MADT_FLAG_TRIGGER_MASK | ACPI_MADT_FLAG_POLARITY_HIGH);
    let global_sys_interrupt = items[1].global_sys_interrupt;
    assert_eq!(global_sys_interrupt, 9);
}
