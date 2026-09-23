// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#![cfg(test)]

extern crate std;

use crate::numa::AcpiNumaDomain;
use crate::test_util::{sys2970wx_acpi_parser, z840_acpi_parser};

#[test]
fn test_parse_z840() {
    let mut domain_counts = [0; 2];
    let mut domains = [AcpiNumaDomain::default(); 2];
    let parser = z840_acpi_parser();

    crate::numa::enumerate_cpu_numa_pairs(&parser, |region, _apic_id| {
        domain_counts[region.domain as usize] += 1;
        domains[region.domain as usize] = *region;
    })
    .unwrap();

    assert_eq!(domain_counts[0], 28);
    assert_eq!(domain_counts[1], 28);
    assert_eq!(domains[0].memory_count, 1);
    assert_eq!(domains[1].memory_count, 1);

    assert_eq!(domains[0].memory[0].base_address, 0);
    assert_eq!(domains[0].memory[0].length, 0x1030000000);

    assert_eq!(domains[1].memory[0].base_address, 0x1030000000);
    assert_eq!(domains[1].memory[0].length, 0x1000000000);
}

#[test]
fn test_parse_2970wx() {
    let mut domain_counts = [0; 4];
    let mut domains = [AcpiNumaDomain::default(); 4];
    let parser = sys2970wx_acpi_parser();

    crate::numa::enumerate_cpu_numa_pairs(&parser, |region, _apic_id| {
        domain_counts[region.domain as usize] += 1;
        domains[region.domain as usize] = *region;
    })
    .unwrap();

    assert_eq!(domain_counts[0], 12);
    assert_eq!(domain_counts[1], 12);
    assert_eq!(domain_counts[2], 12);
    assert_eq!(domain_counts[3], 12);
    assert_eq!(domains[0].memory_count, 3);
    assert_eq!(domains[1].memory_count, 0);
    assert_eq!(domains[2].memory_count, 1);
    assert_eq!(domains[3].memory_count, 0);

    assert_eq!(domains[0].memory[0].base_address, 0x0);
    assert_eq!(domains[0].memory[0].length, 0xa0000);
    assert_eq!(domains[0].memory[1].base_address, 0x100000);
    assert_eq!(domains[0].memory[1].length, 0x7ff00000);
    assert_eq!(domains[0].memory[2].base_address, 0x100000000);
    assert_eq!(domains[0].memory[2].length, 0x180000000);

    assert_eq!(domains[2].memory[0].base_address, 0x280000000);
    assert_eq!(domains[2].memory[0].length, 0x200000000);
}
