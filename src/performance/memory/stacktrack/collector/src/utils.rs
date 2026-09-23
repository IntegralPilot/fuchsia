// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use elf_parse::{SegmentFlags, SegmentType};
use fidl_fuchsia_memory_stacktrack_client as fstacktrack_client;

/// Returns the list of the executable regions in a given process' address space.
pub fn find_executable_regions(
    process: &zx::Process,
) -> Result<Vec<fstacktrack_client::ExecutableRegion>, zx::Status> {
    let mut output_vec = Vec::new();
    elf_search::for_each_module(process, |info| {
        for phdr in info.phdrs {
            if phdr.segment_type == SegmentType::Load as u32
                && (phdr.flags & SegmentFlags::EXECUTE.bits()) != 0
            {
                let mut build_id = info.build_id.to_vec();
                build_id.truncate(fstacktrack_client::MAX_BUILD_ID_LENGTH as usize);

                output_vec.push(fstacktrack_client::ExecutableRegion {
                    address: Some((info.vaddr + phdr.vaddr) as u64),
                    size: Some(phdr.memsz),
                    vaddr: Some(phdr.vaddr as u64),
                    build_id: Some(fstacktrack_client::BuildId { value: build_id }),
                    name: Some(info.name.to_string()),
                    ..Default::default()
                });
            }
        }
    })?;
    Ok(output_vec)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Verify that this function's address belongs to exactly one executable region.
    #[test]
    fn test_find_executable_regions() {
        let executable_regions = find_executable_regions(&fuchsia_runtime::process_self()).unwrap();

        let test_address = test_find_executable_regions as *const () as u64;
        let region_count = executable_regions
            .iter()
            .filter(|region| {
                let start = region.address.unwrap();
                let end = start + region.size.unwrap();
                (start..end).contains(&test_address)
            })
            .count();

        assert_eq!(
            region_count, 1,
            "test address {:x} not covered exactly once by {:x?}",
            test_address, executable_regions
        );
    }
}
