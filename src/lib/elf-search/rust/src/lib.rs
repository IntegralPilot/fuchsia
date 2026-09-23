// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use elf_parse::{Elf64FileHeader, Elf64ProgramHeader};
use std::borrow::Cow;
use std::ffi::{c_char, c_void};

mod sys;
use sys::{Elf64_Ehdr, Elf64_Phdr, elf_search_wrapper};

#[derive(Debug)]
pub struct ModuleInfo<'a> {
    pub name: Cow<'a, str>,
    pub vaddr: usize,
    pub build_id: &'a [u8],
    pub ehdr: &'a Elf64FileHeader,
    pub phdrs: &'a [Elf64ProgramHeader],
}

pub fn for_each_module<F: FnMut(&ModuleInfo<'_>)>(
    process: &zx::Process,
    mut func: F,
) -> Result<(), zx::Status> {
    // SAFETY: the callback function will interpret the func pointer as F.
    let status = unsafe {
        elf_search_wrapper(
            process.raw_handle(),
            Some(callback::<F>),
            &mut func as *mut F as *mut c_void,
        )
    };
    zx::Status::ok(status)
}

unsafe extern "C" fn callback<F: FnMut(&ModuleInfo<'_>)>(
    name: *const c_char,
    name_len: usize,
    vaddr: u64,
    build_id: *const u8,
    build_id_len: usize,
    ehdr: *const Elf64_Ehdr,
    phdrs: *const Elf64_Phdr,
    phdrs_len: usize,
    arg: *mut c_void,
) {
    // SAFETY: all the following pointers are guaranteed to be valid by the caller.
    let name = unsafe { std::slice::from_raw_parts(name as *const u8, name_len) };
    let build_id = unsafe { std::slice::from_raw_parts(build_id, build_id_len) };
    let ehdr = unsafe { &*ehdr };
    let phdrs = unsafe { std::slice::from_raw_parts(phdrs, phdrs_len) };

    // Convert ehdr and phdrs into the corresponding Rust types from the elf_parse library.
    let ehdr: &Elf64FileHeader = zerocopy::transmute_ref!(ehdr);
    let phdrs: &[Elf64ProgramHeader] = zerocopy::transmute_ref!(phdrs);

    let info = ModuleInfo {
        name: String::from_utf8_lossy(name),
        vaddr: vaddr as usize,
        build_id,
        ehdr,
        phdrs,
    };

    // SAFETY: for_each_module guarantees that arg is a valid pointer to F.
    let func = unsafe { &mut *(arg as *mut F) };
    func(&info);
}

#[cfg(test)]
mod tests {
    use super::*;
    use elf_parse::{SegmentFlags, SegmentType};

    #[test]
    fn test_find_executable_regions() {
        // Enumerate all executable regions in the current process.
        let mut regions = Vec::new();
        for_each_module(&fuchsia_runtime::process_self(), |info| {
            for phdr in info.phdrs {
                if phdr.segment_type == SegmentType::Load as u32
                    && (phdr.flags & SegmentFlags::EXECUTE.bits()) != 0
                {
                    let start = info.vaddr + phdr.vaddr as usize;
                    let size = phdr.memsz as usize;
                    regions.push((start, size));
                }
            }
        })
        .unwrap();

        // Locate the address of this test function within the regions.
        let test_address = test_find_executable_regions as *const () as usize;
        let count = regions
            .iter()
            .filter(|(start, size)| (*start..*start + *size).contains(&test_address))
            .count();

        assert_eq!(
            count, 1,
            "test address {:x} not covered exactly once by {:?}",
            test_address, regions
        );
    }
}
