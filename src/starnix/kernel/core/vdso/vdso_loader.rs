// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::arch::vdso::VDSO_SIGRETURN_NAME;
use crate::mm::memory::MemoryObject;
use fidl_fuchsia_io as fio;
use starnix_uapi::errors::Errno;
use starnix_uapi::{errno, from_status_like_fdio};
use std::sync::{Arc, LazyLock};

pub static ZX_TIME_VALUES_MEMORY: LazyLock<Arc<MemoryObject>> = LazyLock::new(|| {
    load_time_values_memory().expect(
        "Could not find time values VMO! Please ensure /boot/kernel was routed to the starnix kernel.",
    )
});

pub struct Vdso {
    pub memory: Arc<MemoryObject>,
    pub sigreturn_offset: u64,
}

impl Vdso {
    pub fn new() -> Self {
        let memory = load_vdso_from_file().expect("Couldn't read vDSO from disk");
        let sigreturn_offset = match VDSO_SIGRETURN_NAME {
            Some(name) => get_sigreturn_offset(&memory, name)
                .expect("Couldn't find sigreturn trampoline code in vDSO"),
            None => 0,
        };

        Self { memory, sigreturn_offset }
    }

    pub fn new_arch32() -> Option<Self> {
        let maybe_memory = load_vdso_arch32_from_file();
        if maybe_memory.is_err() {
            return None;
        }
        let memory = maybe_memory.unwrap();
        let sigreturn_offset = match VDSO_SIGRETURN_NAME {
            Some(name) => get_sigreturn32_offset(&memory, name)
                .expect("Couldn't find sigreturn trampoline code in arch32 vDSO"),
            None => 0,
        };

        Some(Self { memory, sigreturn_offset })
    }
}

fn sync_open_in_namespace(
    path: &str,
    flags: fio::Flags,
) -> Result<fio::DirectorySynchronousProxy, Errno> {
    let (client, server) = fidl::Channel::create();
    let dir_proxy = fio::DirectorySynchronousProxy::new(client);

    let namespace = fdio::Namespace::installed().map_err(|_| errno!(EINVAL))?;
    namespace.open(path, flags, server).map_err(|_| errno!(ENOENT))?;
    Ok(dir_proxy)
}

/// Reads the vDSO file and returns the backing VMO.
fn load_vdso_from_file() -> Result<Arc<MemoryObject>, Errno> {
    const VDSO_FILENAME: &str = "libvdso.so";
    const VDSO_LOCATION: &str = "/pkg/data";

    let dir_proxy = sync_open_in_namespace(VDSO_LOCATION, fio::PERM_READABLE)?;
    let vdso_vmo = syncio::directory_open_vmo(
        &dir_proxy,
        VDSO_FILENAME,
        fio::VmoFlags::READ,
        zx::MonotonicInstant::INFINITE,
    )
    .map_err(|status| from_status_like_fdio!(status))?;

    Ok(Arc::new(MemoryObject::from(vdso_vmo)))
}

/// Reads the vDSO file and returns the backing VMO.
fn load_vdso_arch32_from_file() -> Result<Arc<MemoryObject>, Errno> {
    const VDSO_FILENAME: &str = "libvdso_arch32.so";
    const VDSO_LOCATION: &str = "/pkg/data";

    let dir_proxy = sync_open_in_namespace(VDSO_LOCATION, fio::PERM_READABLE)?;
    let vdso_vmo = syncio::directory_open_vmo(
        &dir_proxy,
        VDSO_FILENAME,
        fio::VmoFlags::READ,
        zx::MonotonicInstant::INFINITE,
    )
    .map_err(|status| from_status_like_fdio!(status))?;

    Ok(Arc::new(MemoryObject::from(vdso_vmo)))
}

fn load_time_values_memory() -> Result<Arc<MemoryObject>, Errno> {
    const FILENAME: &str = "time_values";
    const DIR: &str = "/boot/kernel";

    let (client, server) = fidl::Channel::create();
    let dir_proxy = fio::DirectorySynchronousProxy::new(client);

    let namespace = fdio::Namespace::installed().map_err(|_| errno!(EINVAL))?;
    namespace.open(DIR, fuchsia_fs::PERM_READABLE, server).map_err(|_| errno!(ENOENT))?;

    let vmo = syncio::directory_open_vmo(
        &dir_proxy,
        FILENAME,
        fio::VmoFlags::READ,
        zx::MonotonicInstant::INFINITE,
    )
    .map_err(|status| from_status_like_fdio!(status))?;

    // Check that the time values VMO is the expected size of 1 page. If it is not,
    // panic the kernel, as it means that the size of the time values VMO has changed
    // and the starnix vDSO linker script at //src/starnix/kernel/vdso/vdso.ld should
    // be updated.
    let vmo_size = vmo.get_size().expect("failed to get time values VMO size");
    let expected_size = 0x1000u64;
    if vmo_size != expected_size {
        panic!(
            "time values VMO has unexpected size; got {:?}, expected {:?}",
            vmo_size, expected_size
        );
    }
    Ok(Arc::new(MemoryObject::from(vmo)))
}

fn get_string_index(string_table: &[u8], value: &[u8]) -> Option<usize> {
    for (position, window) in string_table.windows(value.len()).enumerate() {
        if window == value {
            return Some(position);
        }
    }
    None
}

fn get_sigreturn_offset(vdso_memory: &MemoryObject, sigreturn_name: &[u8]) -> Result<u64, Errno> {
    let vdso_vmo = vdso_memory.as_vmo().ok_or_else(|| errno!(EINVAL))?;
    let dyn_section = elf_parse::Elf64DynSection::from_vmo(vdso_vmo).map_err(|_| errno!(EINVAL))?;
    let symtab = dyn_section
        .dynamic_entry_with_tag(elf_parse::Elf64DynTag::Symtab)
        .ok_or_else(|| errno!(EINVAL))?;
    let strtab = dyn_section
        .dynamic_entry_with_tag(elf_parse::Elf64DynTag::Strtab)
        .ok_or_else(|| errno!(EINVAL))?;
    let strsz = dyn_section
        .dynamic_entry_with_tag(elf_parse::Elf64DynTag::Strsz)
        .ok_or_else(|| errno!(EINVAL))?;

    // Find the name of the signal trampoline in the string table and store the index.
    let strtab_bytes = vdso_vmo
        .read_to_vec(strtab.value, strsz.value)
        .map_err(|status| from_status_like_fdio!(status))?;
    let strtab_idx =
        get_string_index(&strtab_bytes, sigreturn_name).ok_or_else(|| errno!(ENOENT))?;

    const SYM_ENTRY_SIZE: usize = std::mem::size_of::<elf_parse::Elf64Sym>();

    // In the symbolic table, find a symbol with a name index pointing to the name we're looking for.
    let mut symtab_offset = symtab.value;
    loop {
        let sym_entry = vdso_vmo
            .read_to_object::<elf_parse::Elf64Sym>(symtab_offset)
            .map_err(|status| from_status_like_fdio!(status))?;
        if sym_entry.st_name as usize == strtab_idx {
            return Ok(sym_entry.st_value);
        }
        symtab_offset += SYM_ENTRY_SIZE as u64;
    }
}

fn get_sigreturn32_offset(vdso_memory: &MemoryObject, sigreturn_name: &[u8]) -> Result<u64, Errno> {
    let vdso_vmo = vdso_memory.as_vmo().ok_or_else(|| errno!(EINVAL))?;
    let dyn_section =
        elf_parse::Elf64DynSection::from_vmo_with_arch32(vdso_vmo).map_err(|_| errno!(EINVAL))?;
    let symtab = dyn_section
        .dynamic_entry_with_tag(elf_parse::Elf64DynTag::Symtab)
        .ok_or_else(|| errno!(EINVAL))?;
    let strtab = dyn_section
        .dynamic_entry_with_tag(elf_parse::Elf64DynTag::Strtab)
        .ok_or_else(|| errno!(EINVAL))?;
    let strsz = dyn_section
        .dynamic_entry_with_tag(elf_parse::Elf64DynTag::Strsz)
        .ok_or_else(|| errno!(EINVAL))?;

    // Find the name of the signal trampoline in the string table and store the index.
    let strtab_bytes = vdso_vmo
        .read_to_vec(strtab.value, strsz.value)
        .map_err(|status| from_status_like_fdio!(status))?;
    let strtab_idx =
        get_string_index(&strtab_bytes, sigreturn_name).ok_or_else(|| errno!(ENOENT))?;

    const SYM_ENTRY_SIZE: usize = std::mem::size_of::<elf_parse::Elf32Sym>();

    // In the symbolic table, find a symbol with a name index pointing to the name we're looking for.
    let mut symtab_offset = symtab.value;
    loop {
        let sym_entry = vdso_vmo
            .read_to_object::<elf_parse::Elf32Sym>(symtab_offset)
            .map_err(|status| from_status_like_fdio!(status))?;
        if sym_entry.st_name as usize == strtab_idx {
            return Ok(sym_entry.st_value as u64);
        }
        symtab_offset += SYM_ENTRY_SIZE as u64;
    }
}
