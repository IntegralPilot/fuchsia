// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{Mapping, Module};
use elf_parse::{Elf64FileHeader, SegmentFlags};
use zerocopy::{FromBytes, Immutable, Unalign};

/// Collect a list of linked code objects from the dynamic linker.
pub fn collect_modules() -> Vec<Module> {
    /// SAFETY: must only be called by the dynamic linker as part of dl_iterate_phdr.
    unsafe extern "C" fn dl_iterate_callback(
        info: *mut libc::dl_phdr_info,
        size: usize,
        context: *mut std::ffi::c_void,
    ) -> i32 {
        assert_eq!(size, std::mem::size_of::<libc::dl_phdr_info>());

        // SAFETY: we pass in a mutable reference for the context pointer below.
        let modules = unsafe { (context as *mut Vec<Module>).as_mut().unwrap() };

        // SAFETY: this info pointer is provided by the dynamic linker and must be valid.
        let module_info = unsafe { info.as_ref().unwrap() };

        // SAFETY: it's sound to resolve the module from pointers since they are from the linker.
        if let Some(module) = unsafe { resolve_module(module_info) } {
            modules.push(module);
        }

        0
    }

    let mut modules = Vec::new();

    // SAFETY: FFI call to dynamic linker helper. Function pointer is valid and we're in charge of
    // validity requirements for the context pointer.
    unsafe {
        libc::dl_iterate_phdr(
            Some(dl_iterate_callback),
            &mut modules as *mut Vec<Module> as *mut std::ffi::c_void,
        )
    };

    modules
}

/// SAFETY: requires the provided program header info to be valid and from the dynamic linker. All
/// program headers must be in readable memory.
unsafe fn resolve_module(module: &libc::dl_phdr_info) -> Option<Module> {
    // SAFETY: this pointer is provided by the linker and must point to valid file headers.
    let file_header = unsafe { (module.dlpi_addr as *const Elf64FileHeader).as_ref().unwrap() };

    // SAFETY: this name is provided by the linker and must point to a valid null-terminated name.
    let name = unsafe { std::ffi::CStr::from_ptr(module.dlpi_name) };
    let name = name.to_string_lossy().to_string();

    let mut build_id = None;
    let mut mappings = vec![];

    for i in 0..file_header.phnum {
        // SAFETY: trust the linker to provide a `phnum` that will not overflow the pointer and
        // that when offset points to valid memory.
        let program_header = unsafe {
            (module.dlpi_phdr as *const libc::Elf64_Phdr).offset(i as isize).as_ref().unwrap()
        };
        let program_flags = SegmentFlags::from_bits_retain(program_header.p_flags);
        let segment_start = module.dlpi_addr + program_header.p_vaddr as u64;
        match program_header.p_type {
            libc::PT_NOTE => {
                // SAFETY: trust the linker offsets and lengths.
                let mut notes_bytes = unsafe {
                    std::slice::from_raw_parts(
                        segment_start as *const u8,
                        program_header.p_memsz as usize,
                    )
                };

                while notes_bytes.len() >= std::mem::size_of::<NoteHeader>() {
                    let (header_bytes, remainder) =
                        notes_bytes.split_at(std::mem::size_of::<NoteHeader>());
                    let note_header =
                        Unalign::<NoteHeader>::ref_from_bytes(header_bytes).unwrap().get();

                    let (note_name, remainder) = split_padded(
                        remainder,
                        note_header.name_size as usize,
                        program_header.p_align as usize,
                    );
                    let (note_desc, remainder) = split_padded(
                        remainder,
                        note_header.desc_size as usize,
                        program_header.p_align as usize,
                    );

                    if note_name == b"GNU\0" && note_header.r#type == NT_GNU_BUILD_ID {
                        // Found the build id, stop looking.
                        build_id = Some(note_desc.to_vec());
                        break;
                    }

                    // Advance our iteration.
                    notes_bytes = remainder;
                }
            }
            libc::PT_LOAD => {
                mappings.push(Mapping {
                    start_addr: segment_start,
                    size: round_up_to_page_size(program_header.p_memsz),
                    vaddr: program_header.p_vaddr as u64,
                    readable: program_flags.contains(SegmentFlags::READ),
                    writeable: program_flags.contains(SegmentFlags::WRITE),
                    executable: program_flags.contains(SegmentFlags::EXECUTE),
                });
            }
            _ => (),
        }
    }

    Some(Module { name, build_id: build_id.unwrap(), mappings })
}

fn split_padded(b: &[u8], split_at: usize, align: usize) -> (&[u8], &[u8]) {
    if split_at == 0 {
        (b, &[])
    } else if split_at % align == 0 {
        b.split_at(split_at)
    } else {
        let first_split = round_up_to(split_at as u64, align as u64) as usize;
        let (padded, remainder) = b.split_at(first_split);
        let (unpadded, _padding) = padded.split_at(split_at);
        (unpadded, remainder)
    }
}

fn round_up_to(size: u64, align: u64) -> u64 {
    let remainder = size % align;
    if remainder == 0 { size } else { size + (align - remainder) }
}

fn round_up_to_page_size(size: u64) -> u64 {
    round_up_to(size, zx::system_get_page_size() as u64)
}

const NT_GNU_BUILD_ID: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug, FromBytes, Immutable)]
struct NoteHeader {
    name_size: u32,
    desc_size: u32,
    r#type: u32,
}
