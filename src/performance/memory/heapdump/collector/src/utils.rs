// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use elf_parse::{SegmentFlags, SegmentType};
use fidl_fuchsia_memory_heapdump_client as fheapdump_client;
use zx::sys::zx_vaddr_t;

#[cfg(target_arch = "aarch64")]
fn untag_ptr(ptr: u64) -> u64 {
    ptr & !(0xFFu64 << 56)
}

#[cfg(not(target_arch = "aarch64"))]
fn untag_ptr(ptr: u64) -> u64 {
    ptr
}

/// Reads the contents of the given process' memory range.
pub fn read_process_memory(
    process: &zx::Process,
    address: u64,
    num_bytes: u64,
) -> Result<Vec<u8>, zx::Status> {
    let mut buf = vec![0; num_bytes.try_into().or(Err(zx::Status::FILE_BIG))?];
    process.read_memory(untag_ptr(address) as zx_vaddr_t, &mut buf)?;
    Ok(buf)
}

/// Returns the list of the executable regions in a given process' address space.
pub fn find_executable_regions(
    process: &zx::Process,
) -> Result<Vec<fheapdump_client::ExecutableRegion>, zx::Status> {
    let mut output_vec = Vec::new();
    elf_search::for_each_module(process, |info| {
        for phdr in info.phdrs {
            if phdr.segment_type == SegmentType::Load as u32
                && (phdr.flags & SegmentFlags::EXECUTE.bits()) != 0
            {
                let mut build_id = info.build_id.to_vec();
                build_id.truncate(fheapdump_client::MAX_BUILD_ID_LENGTH as usize);

                output_vec.push(fheapdump_client::ExecutableRegion {
                    address: Some((info.vaddr + phdr.vaddr) as u64),
                    size: Some(phdr.memsz),
                    file_offset: Some(phdr.offset as u64),
                    build_id: Some(fheapdump_client::BuildId { value: build_id }),
                    vaddr: Some(phdr.vaddr as u64),
                    name: Some(info.name.to_string()),
                    ..Default::default()
                });
            }
        }
    })?;
    Ok(output_vec)
}
