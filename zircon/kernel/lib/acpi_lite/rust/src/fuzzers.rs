// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use fuzz::fuzz;
use zx_status::Status;

// Due to the way fuzzers get built avoid unused code warnings when building and running the fuzz
// cases that do not need the FuzzedPhysMemReader.
#[allow(unused)]
struct FuzzedPhysMemReader<'a> {
    addr: usize,
    data: &'a [u8],
}

impl<'a> FuzzedPhysMemReader<'a> {
    #[allow(unused)]
    fn new(addr: usize, data: &'a [u8]) -> Self {
        let addr = std::cmp::min(addr, usize::MAX - data.len());
        Self { addr, data }
    }
}

impl<'a> crate::PhysMemReader for FuzzedPhysMemReader<'a> {
    fn phys_to_slice(&self, phys: usize, length: usize) -> Result<&[u8], Status> {
        if length == 0 {
            return Err(Status::OUT_OF_RANGE);
        }
        let phys_end = phys.checked_add(length - 1).ok_or(Status::OUT_OF_RANGE)?;
        let data_end = self.addr + self.data.len();

        if self.addr <= phys && phys_end < data_end {
            let offset = phys - self.addr;
            return Ok(&self.data[offset..offset + length]);
        }
        Err(Status::NOT_FOUND)
    }
}

#[fuzz]
fn acpi_lite_fuzztest(data: &[u8]) {
    if data.len() < 16 {
        return;
    }
    let (remaining, suffix) = data.split_at(data.len() - 16);
    let region = usize::from_le_bytes(suffix[0..8].try_into().unwrap());
    let paddr = usize::from_le_bytes(suffix[8..16].try_into().unwrap());

    let reader = FuzzedPhysMemReader::new(region, remaining);

    if let Ok(parser) = crate::AcpiParser::init(&reader, paddr) {
        let _ = crate::get_table_by_signature(&parser, crate::structures::AcpiSignature(*b"APIC"));
    }
}

#[fuzz]
fn apic_fuzztest(data: &[u8]) {
    let size = core::mem::size_of::<crate::structures::AcpiSdtHeader>();
    if data.len() < size {
        return;
    }
    let mut data_vec = data.to_vec();
    let len = data_vec.len() as u32;
    let r = zerocopy::Ref::<_, crate::structures::AcpiSdtHeader>::from_bytes(&mut data_vec[..size])
        .unwrap();
    let header = zerocopy::Ref::into_mut(r);
    header.length = len;

    let parser = crate::test_util::FakeAcpiParser::new_from_headers(&[&*header]);
    let _ = crate::apic::enumerate_io_apics(&parser, |_entry| Ok(()));
}

#[fuzz]
fn debug_port_fuzztest(data: &[u8]) {
    let size = core::mem::size_of::<crate::structures::AcpiDbg2Table>();
    if data.len() < size {
        return;
    }
    let mut data_vec = data.to_vec();
    let len = data_vec.len() as u32;
    let r = zerocopy::Ref::<_, crate::structures::AcpiDbg2Table>::from_bytes(&mut data_vec[..size])
        .unwrap();
    let table = zerocopy::Ref::into_mut(r);
    table.header.length = len;
    let _ = crate::debug_port::parse_acpi_dbg2_table(table);
}

#[fuzz]
fn numa_fuzztest(data: &[u8]) {
    let size = core::mem::size_of::<crate::structures::AcpiSratTable>();
    if data.len() < size {
        return;
    }
    let mut data_vec = data.to_vec();
    let len = data_vec.len() as u32;
    let r = zerocopy::Ref::<_, crate::structures::AcpiSratTable>::from_bytes(&mut data_vec[..size])
        .unwrap();
    let table = zerocopy::Ref::into_mut(r);
    table.header.length = len;
    let _ = crate::numa::enumerate_cpu_numa_pairs_from_srat(table, |_, _| {});
}
