// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![cfg(test)]

use crate::AcpiParser;
use crate::debug_port::AcpiDebugPortType;
use crate::test_util::{intel_nuc7i5dn_phys_mem_reader, pixelbook_atlas_acpi_parser};

#[test]
fn test_parse_chromebook_atlas() {
    let parser = pixelbook_atlas_acpi_parser();

    let debug_port = crate::debug_port::get_debug_port(&parser).unwrap();
    assert_eq!(debug_port.r#type, AcpiDebugPortType::Mmio);
    assert_eq!(debug_port.address, 0xfe034000);
    assert_eq!(debug_port.length, 0x1000);
}

#[test]
fn test_parse_intel_nuc() {
    let reader = intel_nuc7i5dn_phys_mem_reader();
    let parser = AcpiParser::init(&reader, reader.rsdp()).unwrap();

    let debug_port = crate::debug_port::get_debug_port(&parser).unwrap();
    assert_eq!(debug_port.r#type, AcpiDebugPortType::Pio);
    assert_eq!(debug_port.address, 0x3f8);
    assert_eq!(debug_port.length, 12);
}
