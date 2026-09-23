// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::memory::MemoryObject;
use crate::mm::{
    DesiredAddress, MappingName, MappingOptions, MemoryAccessor, MemoryManager, PAGE_SIZE,
    ProtectionFlags, VMEX_RESOURCE,
};
use crate::security;
use crate::task::CurrentTask;
use crate::vdso::vdso_loader::ZX_TIME_VALUES_MEMORY;
use crate::vfs::{FdNumber, FileMapping};
use process_builder::elf_load;
use starnix_logging::{log_error, log_warn};
use starnix_types::arch::ArchWidth;
use starnix_types::math::round_up_to_system_page_size;
use starnix_types::thread_start_info::ThreadStartInfo;
use starnix_types::time::SCHEDULER_CLOCK_HZ;
#[cfg(target_arch = "aarch64")]
use starnix_uapi::AT_PLATFORM;
use starnix_uapi::auth::Credentials;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{ArchSpecific, UserAddress};
use starnix_uapi::{
    AT_BASE, AT_CLKTCK, AT_EGID, AT_ENTRY, AT_EUID, AT_EXECFN, AT_GID, AT_HWCAP, AT_HWCAP2,
    AT_NULL, AT_PAGESZ, AT_PHDR, AT_PHENT, AT_PHNUM, AT_RANDOM, AT_SECURE, AT_SYSINFO_EHDR, AT_UID,
    ENODEV, errno, error, from_status_like_fdio,
};
use std::ffi::{CStr, CString};
use std::mem::size_of;
use std::ops::Deref as _;
use std::sync::Arc;

#[derive(Debug)]
struct StackResult {
    stack_pointer: UserAddress,
    auxv_start: UserAddress,
    auxv_end: UserAddress,
    argv_start: UserAddress,
    argv_end: UserAddress,
    environ_start: UserAddress,
    environ_end: UserAddress,
}

const RANDOM_SEED_BYTES: usize = 16;

fn get_initial_stack_size(
    path: &CStr,
    argv: &Vec<CString>,
    environ: &Vec<CString>,
    auxv: &Vec<(u32, u64)>,
    arch_width: ArchWidth,
) -> usize {
    // Worst-case, we overspec the initial stack size.
    let auxv_entry_size = if arch_width.is_arch32() { size_of::<u32>() } else { size_of::<u64>() };
    let ptr_size = if arch_width.is_arch32() { size_of::<u32>() } else { size_of::<*const u8>() };
    argv.iter().map(|x| x.as_bytes_with_nul().len()).sum::<usize>()
        + environ.iter().map(|x| x.as_bytes_with_nul().len()).sum::<usize>()
        + path.to_bytes_with_nul().len()
        + RANDOM_SEED_BYTES
        + (argv.len() + 1 + environ.len() + 1) * ptr_size
        + (auxv.len() + 5) * 2 * auxv_entry_size
}

fn populate_initial_stack(
    ma: &impl MemoryAccessor,
    path: &CStr,
    argv: &Vec<CString>,
    environ: &Vec<CString>,
    mut auxv: Vec<(u32, u64)>,
    original_stack_start_addr: UserAddress,
    arch_width: ArchWidth,
) -> Result<StackResult, Errno> {
    let mut stack_pointer = original_stack_start_addr;
    let write_stack = |data: &[u8], addr: UserAddress| ma.write_memory(addr, data);

    let argv_end = stack_pointer;
    for arg in argv.iter().rev() {
        stack_pointer = (stack_pointer - arg.as_bytes_with_nul().len())?;
        write_stack(arg.as_bytes_with_nul(), stack_pointer)?;
    }
    let argv_start = stack_pointer;

    let environ_end = stack_pointer;
    for env in environ.iter().rev() {
        stack_pointer = (stack_pointer - env.as_bytes_with_nul().len())?;
        write_stack(env.as_bytes_with_nul(), stack_pointer)?;
    }

    // TODO(https://fxbug.dev/380427153): Get rid of this probably and roll back r0, r1, r2 setting.
    let environ_start = stack_pointer;

    // Write the path used with execve.
    stack_pointer = (stack_pointer - path.to_bytes_with_nul().len())?;
    let execfn_addr = stack_pointer;
    write_stack(path.to_bytes_with_nul(), execfn_addr)?;

    let mut random_seed = [0; RANDOM_SEED_BYTES];
    starnix_crypto::cprng_draw(&mut random_seed);
    stack_pointer = (stack_pointer - random_seed.len())?;
    let random_seed_addr = stack_pointer;
    write_stack(&random_seed, random_seed_addr)?;

    // TODO(https://fxbug.dev/380427153): Move to arch specific inclusion
    #[cfg(target_arch = "aarch64")]
    if arch_width.is_arch32() {
        let platform = b"v7l\0";
        stack_pointer = (stack_pointer - platform.len())?;
        let platform_addr = stack_pointer;
        write_stack(platform, platform_addr)?;
        // Write the platform to the stack too
        // TODO(https://fxbug.dev/380427153): add arch helper
        auxv.push((AT_PLATFORM, platform_addr.ptr() as u64));
    }

    auxv.push((AT_EXECFN, execfn_addr.ptr() as u64));
    auxv.push((AT_RANDOM, random_seed_addr.ptr() as u64));
    auxv.push((AT_NULL, 0));

    // After the remainder (argc/argv/environ/auxv) is pushed, the stack pointer must be 16 byte
    // aligned. This is required by the ABI and assumed by the compiler to correctly align SSE
    // operations. But this can't be done after it's pushed, since it has to be right at the top of
    // the stack. So we collect it all, align the stack appropriately now that we know the size,
    // and push it all at once.
    //
    // Compatibility stacks use u32 rather than u64, so we wrap the extension in
    // a macro to automatically truncate -- assuming all the values we made
    // appropriate earlier.
    fn extend_vec(data: &mut Vec<u8>, slice: &[u8; 8], arch_width: ArchWidth) {
        if arch_width.is_arch32() {
            let value = u64::from_ne_bytes(*slice);
            let truncated_value = (value & (u32::MAX as u64)) as u32;
            data.extend_from_slice(&truncated_value.to_ne_bytes());
        } else {
            data.extend_from_slice(slice);
        }
    }

    let mut main_data = vec![];
    // argc
    let argc: u64 = argv.len() as u64;
    extend_vec(&mut main_data, &argc.to_ne_bytes(), arch_width);

    // argv
    const ZERO: [u8; 8] = [0; 8];
    let mut next_arg_addr = argv_start;
    for arg in argv {
        extend_vec(&mut main_data, &next_arg_addr.ptr().to_ne_bytes(), arch_width);
        next_arg_addr = (next_arg_addr + arg.as_bytes_with_nul().len())?;
    }
    extend_vec(&mut main_data, &ZERO, arch_width);
    // environ
    let mut next_env_addr = environ_start;
    for env in environ {
        extend_vec(&mut main_data, &next_env_addr.ptr().to_ne_bytes(), arch_width);
        next_env_addr = (next_env_addr + env.as_bytes_with_nul().len())?;
    }
    extend_vec(&mut main_data, &ZERO, arch_width);
    // auxv
    let auxv_start_offset = main_data.len();
    for (tag, val) in auxv {
        extend_vec(&mut main_data, &(tag as u64).to_ne_bytes(), arch_width);
        extend_vec(&mut main_data, &val.to_ne_bytes(), arch_width);
    }
    let auxv_end_offset = main_data.len();

    // Time to push.
    stack_pointer = (stack_pointer - main_data.len())?;
    stack_pointer = (stack_pointer - stack_pointer.ptr() % 16)?;
    write_stack(main_data.as_slice(), stack_pointer)?;
    let auxv_start = (stack_pointer + auxv_start_offset)?;
    let auxv_end = (stack_pointer + auxv_end_offset)?;

    Ok(StackResult {
        stack_pointer,
        auxv_start,
        auxv_end,
        argv_start,
        argv_end,
        environ_start,
        environ_end,
    })
}

struct LoadedElf {
    arch_width: ArchWidth,
    headers: elf_parse::Elf64Headers,
    file_base: usize,
    vaddr_bias: usize,
    length: usize,
}

// TODO: Improve the error reporting produced by this function by mapping ElfParseError to Errno more precisely.
fn elf_parse_error_to_errno(err: elf_parse::ElfParseError) -> Errno {
    log_warn!("elf parse error: {:?}", err);
    errno!(ENOEXEC)
}

// TODO: Improve the error reporting produced by this function by mapping ElfLoadError to Errno more precisely.
fn elf_load_error_to_errno(err: elf_load::ElfLoadError) -> Errno {
    log_warn!("elf load error: {:?}", err);
    errno!(EINVAL)
}

struct Mapper {
    file: Arc<FileMapping>,
    mm: Arc<MemoryManager>,
}

impl elf_load::Mapper for Mapper {
    fn map(
        &self,
        vmar_offset: usize,
        vmo: &zx::Vmo,
        vmo_offset: u64,
        length: usize,
        vmar_flags: zx::VmarFlags,
    ) -> Result<usize, zx::Status> {
        let memory = Arc::new(MemoryObject::from(vmo.duplicate_handle(zx::Rights::SAME_RIGHTS)?));
        self.mm
            .map_memory(
                // TODO(https://fxbug.dev/380427153): This checked_add won't help with arch32.
                DesiredAddress::Fixed(self.mm.base_addr.checked_add(vmar_offset).ok_or_else(
                    || {
                        log_error!(
                            "in elf load, addition overflow attempting to map at {:?} + {:#x}",
                            self.mm.base_addr,
                            vmar_offset
                        );
                        zx::Status::INVALID_ARGS
                    },
                )?),
                memory,
                vmo_offset,
                length,
                ProtectionFlags::from_vmar_flags(vmar_flags),
                MappingOptions::ELF_BINARY,
                MappingName::File(self.file.clone()),
            )
            .map_err(|e| {
                // TODO: Find a way to propagate this errno to the caller.
                log_error!("elf map error: {:?}", e);
                zx::Status::INVALID_ARGS
            })
            .map(|addr| addr.ptr())
    }
}

enum LoadElfUsage {
    MainElf,
    Interpreter,
}

fn parse_elf_headers(vmo: &zx::Vmo) -> Result<elf_parse::Elf64Headers, Errno> {
    if cfg!(target_arch = "aarch64") {
        elf_parse::Elf64Headers::from_vmo_with_arch32(vmo).map_err(elf_parse_error_to_errno)
    } else {
        elf_parse::Elf64Headers::from_vmo(vmo).map_err(elf_parse_error_to_errno)
    }
}

fn load_elf(
    current_task: &CurrentTask,
    elf_file: Arc<FileMapping>,
    elf_memory: Arc<MemoryObject>,
    headers: elf_parse::Elf64Headers,
    mm: &Arc<MemoryManager>,
    usage: LoadElfUsage,
) -> Result<LoadedElf, Errno> {
    security::mmap_file(
        current_task,
        Some(elf_file.file()),
        ProtectionFlags::READ | ProtectionFlags::EXEC,
        MappingOptions::ELF_BINARY,
    )?;
    let vmo = elf_memory.as_vmo().ok_or_else(|| errno!(EINVAL))?;
    let arch_width = get_arch_width(&headers);
    let elf_info = elf_load::loaded_elf_info(&headers);
    let length = elf_info.high - elf_info.low;
    let file_base = match headers.file_header().elf_type() {
        Ok(elf_parse::ElfType::SharedObject) => {
            match usage {
                // Location of main position-independent executable is subject to ASLR
                LoadElfUsage::MainElf => {
                    mm.get_random_base_for_executable(arch_width, length)?.ptr()
                }
                // Interpreter is mapped in the same range as regular `mmap` allocations.
                LoadElfUsage::Interpreter => mm
                    .state
                    .read()
                    .find_next_unused_range(length)
                    .ok_or_else(|| errno!(EINVAL))?
                    .ptr(),
            }
        }
        Ok(elf_parse::ElfType::Executable) => elf_info.low,
        _ => return error!(EINVAL),
    };
    // TODO(https://fxbug.dev/380427153): I think we need to do a 32-bit wrap here and then
    // a 32-bit wrap at the wrapping addition.  The Mapper may need a checked_add as well.
    let vaddr_bias = file_base.wrapping_sub(elf_info.low);
    let mapper = Mapper { file: elf_file, mm: mm.clone() };
    elf_load::map_elf_segments(vmo, &headers, &mapper, mm.base_addr.ptr(), vaddr_bias)
        .map_err(elf_load_error_to_errno)?;
    Ok(LoadedElf { arch_width, headers, file_base, vaddr_bias, length })
}

/// Program state and parameters staged during an `execve` call.
pub struct ResolvedProgram {
    /// Current executable file handle, updated by [`resolve_executable()`] if `#!` scripts are followed.
    pub file: Arc<FileMapping>,
    /// Path string passed to `execve()`.
    path: CString,
    /// Arguments for the new process (rewritten if `#!` scripts are resolved).
    argv: Vec<CString>,
    /// Environment variables for the new process.
    environ: Vec<CString>,
    /// Staged [`Credentials`] for the new process.
    pub creds: Credentials,
    /// Whether secure execution mode (`AT_SECURE`) is required for the new process.
    pub secure_exec: bool,
    /// Parsed ELF binary details, populated by [`resolve_executable()`].
    elf: Option<ResolvedElf>,
}

impl ResolvedProgram {
    /// Creates a new [`ResolvedProgram`] initialized with the caller's credentials.
    pub fn new(
        current_task: &CurrentTask,
        file: Arc<FileMapping>,
        path: CString,
        argv: Vec<CString>,
        environ: Vec<CString>,
    ) -> Self {
        Self {
            file,
            path,
            argv,
            environ,
            creds: Credentials::clone(&current_task.current_creds()),
            secure_exec: false,
            elf: None,
        }
    }

    /// Returns the path string passed to `execve()`.
    pub fn path(&self) -> &CStr {
        &self.path
    }

    /// Returns the [`ArchWidth`] of the resolved ELF executable, if resolved.
    pub fn arch_width(&self) -> Option<ArchWidth> {
        self.elf.as_ref().map(|elf| elf.arch_width)
    }
}

/// Parsed ELF binary details for the main executable and optional dynamic linker.
struct ResolvedElf {
    /// Backing [`MemoryObject`] for the executable.
    memory: Arc<MemoryObject>,
    /// Parsed [`elf_parse::Elf64Headers`] for the executable.
    headers: elf_parse::Elf64Headers,
    /// [`ArchWidth`] (32-bit or 64-bit) of the executable.
    arch_width: ArchWidth,
    /// Resolved dynamic linker (`PT_INTERP`), if specified in `headers`.
    interp: Option<ResolvedElfInterp>,
}

/// Resolved ELF dynamic linker (`PT_INTERP`).
struct ResolvedElfInterp {
    /// Opened [`FileMapping`] for the interpreter.
    file: Arc<FileMapping>,
    /// Backing [`MemoryObject`] for the interpreter.
    memory: Arc<MemoryObject>,
    /// Parsed [`elf_parse::Elf64Headers`] for the interpreter.
    headers: elf_parse::Elf64Headers,
}

// The magic bytes of a script file.
const HASH_BANG_SIZE: usize = 2;
const HASH_BANG: &[u8; HASH_BANG_SIZE] = b"#!";
const MAX_RECURSION_DEPTH: usize = 5;

/// Resolves [`ResolvedProgram::file`] into a validated ELF executable, following `#!` script
/// interpreters up to [`MAX_RECURSION_DEPTH`].
///
/// Updates [`ResolvedProgram::file`] and `resolved_program.argv` if scripts are followed, and
/// populates `resolved_program.elf`.
pub fn resolve_executable(
    current_task: &CurrentTask,
    resolved_program: &mut ResolvedProgram,
) -> Result<(), Errno> {
    let script_path = resolved_program.path.clone();
    resolve_executable_impl(
        current_task,
        resolved_program,
        script_path,
        /* recursion_depth = */ 0,
    )
}

/// Recursive helper for [`resolve_executable()`].
fn resolve_executable_impl(
    current_task: &CurrentTask,
    resolved_program: &mut ResolvedProgram,
    script_path: CString,
    recursion_depth: usize,
) -> Result<(), Errno> {
    if recursion_depth > MAX_RECURSION_DEPTH {
        return error!(ELOOP);
    }
    let memory = resolved_program
        .file
        .file()
        .get_memory(current_task, None, ProtectionFlags::READ | ProtectionFlags::EXEC)
        .map_err(|e| if e.code.error_code() == ENODEV { errno!(ENOEXEC) } else { e })?;
    let header = match memory.read_to_array::<u8, HASH_BANG_SIZE>(0) {
        Ok(header) => Ok(header),
        Err(zx::Status::OUT_OF_RANGE) => {
            // The file is empty, or it would have at least one page allocated to it.
            return error!(ENOEXEC);
        }
        Err(_) => return error!(EINVAL),
    }?;
    if &header == HASH_BANG {
        resolve_script(current_task, memory, resolved_program, script_path, recursion_depth)
    } else {
        resolved_program.elf = Some(resolve_elf(memory)?);
        Ok(())
    }
}

/// Resolves a #! script file into a validated executable ELF.
fn resolve_script(
    current_task: &CurrentTask,
    memory: Arc<MemoryObject>,
    resolved_program: &mut ResolvedProgram,
    script_path: CString,
    recursion_depth: usize,
) -> Result<(), Errno> {
    // All VMOs have sizes in multiple of the system page size, so as long as we only read a page or
    // less, we should never read past the end of the VMO.
    // Since Linux 5.1, the max length of the interpreter following the #! is 255.
    const HEADER_BUFFER_CAP: usize = 255 + HASH_BANG.len();
    let buffer = match memory.read_to_array::<u8, HEADER_BUFFER_CAP>(0) {
        Ok(b) => b,
        Err(_) => return error!(EINVAL),
    };

    let mut args = parse_interpreter_line(&buffer)?;
    let interpreter = current_task.open_file_for_exec(
        FdNumber::AT_FDCWD,
        args[0].as_bytes().into(),
        OpenFlags::empty(),
    )?;

    let next_script_path = args[0].clone();

    // Append the original script executable path as an argument.
    args.push(script_path);

    // Append the original arguments (minus argv[0]).
    args.extend(resolved_program.argv.drain(..).skip(1));

    resolved_program.file = interpreter;
    resolved_program.argv = args;

    // Recurse and resolve the interpreter executable
    resolve_executable_impl(current_task, resolved_program, next_script_path, recursion_depth + 1)
}

/// Parses a "#!" byte string and extracts CString arguments. The byte string must contain an
/// ASCII newline character or null-byte, or else it is considered truncated and parsing will fail.
/// If the byte string is empty or contains only whitespace, parsing fails.
/// If successful, the returned `Vec` will have at least one element (the interpreter path).

fn parse_interpreter_line(line: &[u8]) -> Result<Vec<CString>, Errno> {
    // Assuming the byte string starts with "#!", truncate the input to end at the first newline or
    // null-byte. If not found, assume the input was truncated and fail parsing.
    let end = line.iter().position(|&b| b == b'\n' || b == 0).ok_or_else(|| errno!(EINVAL))?;
    let line = &line[HASH_BANG.len()..end];

    // Skip whitespace at the start.
    let is_tab_or_space = |&b| b == b' ' || b == b'\t';
    let begin = line.iter().position(|b| !is_tab_or_space(b)).unwrap_or(0);
    let line = &line[begin..];

    // Split the byte string at the first whitespace character (or end of line). The first part
    // is the interpreter path.
    let first_whitespace = line.iter().position(is_tab_or_space).unwrap_or(line.len());
    let (interpreter, rest) = line.split_at(first_whitespace);
    if interpreter.is_empty() {
        return error!(ENOEXEC);
    }

    // The second part is the optional argument. Trim the leading and trailing whitespace, but
    // treat the middle as a single argument, even if whitespace is encountered.
    let begin = rest.iter().position(|b| !is_tab_or_space(b)).unwrap_or(rest.len());
    let end = rest.iter().rposition(|b| !is_tab_or_space(b)).map(|b| b + 1).unwrap_or(rest.len());
    let optional_arg = &rest[begin..end];

    // SAFETY: `CString::new` can only fail if it encounters a null-byte, which we've made sure
    // the input won't have.
    Ok(if optional_arg.is_empty() {
        vec![CString::new(interpreter).unwrap()]
    } else {
        vec![CString::new(interpreter).unwrap(), CString::new(optional_arg).unwrap()]
    })
}

/// Resolves a [`MemoryObject`] into a validated [`ResolvedElf`].
fn resolve_elf(memory: Arc<MemoryObject>) -> Result<ResolvedElf, Errno> {
    let vmo = memory.as_vmo().ok_or_else(|| errno!(EINVAL))?;
    let headers = parse_elf_headers(vmo)?;
    let arch_width = get_arch_width(&headers);
    Ok(ResolvedElf { memory, headers, arch_width, interp: None })
}

/// Opens and validates the ELF dynamic linker (`PT_INTERP`) for a [`ResolvedProgram`], if present.
pub fn resolve_elf_interpreter(
    current_task: &CurrentTask,
    resolved_program: &mut ResolvedProgram,
) -> Result<(), Errno> {
    let elf = resolved_program.elf.as_mut().ok_or_else(|| errno!(EINVAL))?;
    if let Some(interp_hdr) = elf
        .headers
        .program_header_with_type(elf_parse::SegmentType::Interp)
        .map_err(|_| errno!(EINVAL))?
    {
        // From <https://man7.org/linux/man-pages/man5/elf.5.html>:
        //
        //   PT_INTERP
        //          The array element specifies the location and size of a
        //          null-terminated path name to invoke as an interpreter.
        //
        // Reject segments smaller than 2 bytes (1 character plus NUL) or exceeding `PATH_MAX`.
        if interp_hdr.filesz > starnix_uapi::PATH_MAX as u64 || interp_hdr.filesz < 2 {
            return error!(ENOEXEC);
        }
        let interp = elf
            .memory
            .read_to_vec(interp_hdr.offset as u64, interp_hdr.filesz)
            .map_err(|status| from_status_like_fdio!(status))?;
        let interp = CStr::from_bytes_until_nul(&interp).map_err(|_| errno!(ENOEXEC))?;

        let interp_file = current_task.open_file_for_exec(
            FdNumber::AT_FDCWD,
            interp.to_bytes().into(),
            OpenFlags::empty(),
        )?;
        let interp_memory = interp_file
            .file()
            .get_memory(current_task, None, ProtectionFlags::READ | ProtectionFlags::EXEC)
            .map_err(|e| if e.code.error_code() == ENODEV { errno!(ENOEXEC) } else { e })?;

        let vmo = interp_memory.as_vmo().ok_or_else(|| errno!(EINVAL))?;
        let interp_headers = parse_elf_headers(vmo)?;
        let interp_arch_width = get_arch_width(&interp_headers);
        if elf.arch_width != interp_arch_width {
            log_warn!("interpreter elf and main elf are different architectures!");
            return error!(ENOEXEC);
        }

        elf.interp = Some(ResolvedElfInterp {
            file: interp_file,
            memory: interp_memory,
            headers: interp_headers,
        });
    }

    Ok(())
}

/// Maps a [`ResolvedProgram`] and its optional `PT_INTERP` dynamic linker into memory, and
/// initializes the stack.
pub fn load_executable(
    current_task: &CurrentTask,
    resolved_program: ResolvedProgram,
) -> Result<ThreadStartInfo, Errno> {
    let elf = resolved_program.elf.ok_or_else(|| errno!(EINVAL))?;
    let mm = current_task.mm()?;
    let main_elf = load_elf(
        current_task,
        resolved_program.file,
        elf.memory,
        elf.headers,
        &mm,
        LoadElfUsage::MainElf,
    )?;
    mm.initialize_brk_origin(
        main_elf.arch_width,
        UserAddress::from_ptr(main_elf.file_base)
            .checked_add(main_elf.length)
            .ok_or_else(|| errno!(EINVAL))?,
    )?;
    let interp_elf = elf
        .interp
        .map(|interp| {
            load_elf(
                current_task,
                interp.file,
                interp.memory,
                interp.headers,
                &mm,
                LoadElfUsage::Interpreter,
            )
        })
        .transpose()?;

    let entry_elf = interp_elf.as_ref().unwrap_or(&main_elf);
    let entry_addr = entry_elf.headers.file_header().entry.wrapping_add(entry_elf.vaddr_bias);
    let main_elf_entry = main_elf.headers.file_header().entry.wrapping_add(main_elf.vaddr_bias);
    let main_phdr = main_elf.file_base.wrapping_add(main_elf.headers.file_header().phoff);
    // For consistency with the loader, we wrap u64 then modulo the 3Gb limit
    // even though entry points would normally all be in the lower 1Gb.
    let entry = UserAddress::from_ptr(entry_addr);

    let vdso_memory = if main_elf.arch_width.is_arch32() {
        &current_task.kernel().vdso_arch32.as_ref().expect("an arch32 VDSO").memory
    } else {
        &current_task.kernel().vdso.memory
    };

    let vdso_size = vdso_memory.get_size();
    const VVAR_PROT_FLAGS: ProtectionFlags = ProtectionFlags::READ;

    let utc_clock_handle = crate::time::utc::duplicate_real_utc_clock_handle()
        .expect("clock should always be readable");
    let utc_clock = Arc::new(MemoryObject::from(utc_clock_handle));
    let utc_clock_size = utc_clock.get_size();

    // Map the time values VMO used by libfasttime. We map this right behind the vvar so that
    // userspace sees this as one big vvar block in memory.
    let time_values_size = ZX_TIME_VALUES_MEMORY.get_size();
    let time_values_address = mm.map_memory(
        DesiredAddress::Any,
        ZX_TIME_VALUES_MEMORY.clone(),
        0,
        (time_values_size as usize) + (utc_clock_size as usize) + (vdso_size as usize),
        VVAR_PROT_FLAGS,
        MappingOptions::empty(),
        MappingName::Vvar,
    )?;

    // Overwrite the second part of the vvar mapping with the memory mapped UTC clock.
    let vvar_map_address = mm.map_memory(
        DesiredAddress::FixedOverwrite(
            (time_values_address + time_values_size).expect("vvar should always be mappable"),
        ),
        utc_clock,
        /*memory_offset=*/ 0u64,
        utc_clock_size as usize,
        VVAR_PROT_FLAGS,
        MappingOptions::empty(),
        MappingName::Vvar,
    )?;

    // Create a private clone of the starnix kernel vDSO.
    let vdso_clone = vdso_memory
        .create_child(zx::VmoChildOptions::SNAPSHOT_AT_LEAST_ON_WRITE, 0, vdso_size)
        .map_err(|status| from_status_like_fdio!(status))?;

    let vdso_executable = Arc::new(
        vdso_clone
            .replace_as_executable(&VMEX_RESOURCE)
            .map_err(|status| from_status_like_fdio!(status))?,
    );

    const VDSO_PROT_FLAGS: ProtectionFlags = ProtectionFlags::READ.union(ProtectionFlags::EXEC);

    // Overwrite the third part of the vvar mapping to contain the vDSO clone.
    let vdso_base_address = mm.map_memory(
        DesiredAddress::FixedOverwrite(
            (vvar_map_address + utc_clock_size).expect("vdso should always be mappable"),
        ),
        vdso_executable,
        0,
        vdso_size as usize,
        VDSO_PROT_FLAGS,
        MappingOptions::DONT_SPLIT,
        MappingName::Vdso,
    )?;

    let auxv = {
        let creds = current_task.current_creds();
        let secure = if resolved_program.secure_exec { 1 } else { 0 };

        let hwcap = if main_elf.arch_width.is_arch32() {
            #[cfg(target_arch = "aarch64")]
            {
                current_task.kernel().hwcaps.arch32
            }
            #[cfg(not(target_arch = "aarch64"))]
            {
                unreachable!("32-bit programs are only supported on ARM.")
            }
        } else {
            current_task.kernel().hwcaps.arch64
        };
        vec![
            (AT_PAGESZ, *PAGE_SIZE),
            (AT_CLKTCK, SCHEDULER_CLOCK_HZ as u64),
            (AT_UID, creds.uid as u64),
            (AT_EUID, creds.euid as u64),
            (AT_GID, creds.gid as u64),
            (AT_EGID, creds.egid as u64),
            (AT_PHDR, main_phdr as u64),
            (AT_PHENT, main_elf.headers.file_header().phentsize as u64),
            (AT_PHNUM, main_elf.headers.file_header().phnum as u64),
            (AT_BASE, interp_elf.map_or(0, |interp| interp.file_base as u64)),
            (AT_ENTRY, main_elf_entry as u64),
            (AT_SYSINFO_EHDR, vdso_base_address.into()),
            (AT_SECURE, secure),
            (AT_HWCAP, hwcap.hwcap as u64),
            (AT_HWCAP2, hwcap.hwcap2 as u64),
        ]
    };

    // TODO(tbodt): implement MAP_GROWSDOWN and then reset this to 1 page. The current value of
    // this is based on adding 0x1000 each time a segfault appears.
    let stack_size: usize = round_up_to_system_page_size(
        get_initial_stack_size(
            &resolved_program.path,
            &resolved_program.argv,
            &resolved_program.environ,
            &auxv,
            main_elf.arch_width,
        ) + 0xf0000,
    )
    .expect("stack is too big");

    let prot_flags = ProtectionFlags::READ | ProtectionFlags::WRITE;
    let stack_base = mm.map_stack(stack_size, prot_flags)?;

    let stack = (stack_base + (stack_size - 8))?;

    let stack = populate_initial_stack(
        current_task.deref(),
        &resolved_program.path,
        &resolved_program.argv,
        &resolved_program.environ,
        auxv,
        stack,
        main_elf.arch_width,
    )?;

    let mut mm_state = mm.state.write();
    mm_state.stack_size = stack_size;
    mm_state.stack_start = stack.stack_pointer;
    mm_state.auxv_start = stack.auxv_start;
    mm_state.auxv_end = stack.auxv_end;
    mm_state.argv_start = stack.argv_start;
    mm_state.argv_end = stack.argv_end;
    mm_state.environ_start = stack.environ_start;
    mm_state.environ_end = stack.environ_end;

    mm_state.vdso_base = vdso_base_address;

    let ptr_size: usize =
        if main_elf.arch_width.is_arch32() { size_of::<u32>() } else { size_of::<*const u8>() };
    let envp = (stack.stack_pointer
        + ((resolved_program.argv.len() + 1 /* argc */ + 1/* NULL */) * ptr_size))?;
    Ok(ThreadStartInfo {
        entry,
        stack: stack.stack_pointer,
        environ: envp,
        arch_width: main_elf.arch_width,
    })
}

fn get_arch_width(#[allow(unused_variables)] headers: &elf_parse::Elf64Headers) -> ArchWidth {
    #[cfg(target_arch = "aarch64")]
    if headers.file_header().ident.is_arch32() {
        return ArchWidth::Arch32;
    }
    ArchWidth::Arch64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::spawn_kernel_and_run_with_pkgfs;
    use assert_matches::assert_matches;
    use std::mem::MaybeUninit;

    const TEST_STACK_ADDR: UserAddress = UserAddress::const_from(0x3000_0000);

    struct StackVmo(zx::Vmo);

    impl StackVmo {
        fn address_to_offset(&self, addr: UserAddress) -> u64 {
            (addr - TEST_STACK_ADDR) as u64
        }
    }

    impl MemoryAccessor for StackVmo {
        fn read_memory<'a>(
            &self,
            _addr: UserAddress,
            _bytes: &'a mut [MaybeUninit<u8>],
        ) -> Result<&'a mut [u8], Errno> {
            todo!()
        }

        fn read_memory_partial_until_null_byte<'a>(
            &self,
            _addr: UserAddress,
            _bytes: &'a mut [MaybeUninit<u8>],
        ) -> Result<&'a mut [u8], Errno> {
            todo!()
        }

        fn read_memory_partial<'a>(
            &self,
            _addr: UserAddress,
            _bytes: &'a mut [MaybeUninit<u8>],
        ) -> Result<&'a mut [u8], Errno> {
            todo!()
        }

        fn write_memory(&self, addr: UserAddress, bytes: &[u8]) -> Result<usize, Errno> {
            self.0.write(bytes, self.address_to_offset(addr)).map_err(|_| errno!(EFAULT))?;
            Ok(bytes.len())
        }

        fn write_memory_partial(&self, _addr: UserAddress, _bytes: &[u8]) -> Result<usize, Errno> {
            todo!()
        }

        fn zero(&self, _addr: UserAddress, _length: usize) -> Result<usize, Errno> {
            todo!()
        }
    }

    #[::fuchsia::test]
    fn test_trivial_initial_stack() {
        let stack_vmo = StackVmo(zx::Vmo::create(0x4000).expect("VMO creation should succeed."));
        let original_stack_start_addr = (TEST_STACK_ADDR + 0x1000u64).expect("OOB memory access.");

        let path = CString::new(&b""[..]).unwrap();
        let argv = &vec![];
        let environ = &vec![];

        let stack_start_addr = populate_initial_stack(
            &stack_vmo,
            &path,
            argv,
            environ,
            vec![],
            original_stack_start_addr,
            ArchWidth::Arch64,
        )
        .expect("Populate initial stack should succeed.")
        .stack_pointer;

        let argc_size: usize = 8;
        let argv_terminator_size: usize = 8;
        let environ_terminator_size: usize = 8;
        let aux_execfn_terminator_size: usize = 8;
        let aux_execfn: usize = 16;
        let aux_random: usize = 16;
        let aux_null: usize = 16;
        let random_seed: usize = 16;

        let mut payload_size = argc_size
            + argv_terminator_size
            + environ_terminator_size
            + aux_execfn_terminator_size
            + aux_execfn
            + aux_random
            + aux_null
            + random_seed;
        if payload_size % 16 > 0 {
            payload_size += 16 - (payload_size % 16);
        }

        assert_eq!(
            stack_start_addr,
            (original_stack_start_addr - payload_size).expect("OOB memory access.")
        );
    }

    fn exec_hello_starnix(current_task: &mut CurrentTask) -> Result<(), Errno> {
        let argv = vec![CString::new("data/tests/hello_starnix").unwrap()];
        let executable = current_task.open_file_for_exec(
            FdNumber::AT_FDCWD,
            argv[0].as_bytes().into(),
            OpenFlags::empty(),
        )?;
        current_task.exec(executable, argv[0].clone(), argv, vec![])?;
        Ok(())
    }

    #[fuchsia::test]
    async fn test_load_hello_starnix() {
        spawn_kernel_and_run_with_pkgfs(async |current_task| {
            exec_hello_starnix(current_task).expect("failed to load executable");
            assert!(current_task.mm().unwrap().get_mapping_count() > 0);
        })
        .await;
    }

    // TODO(https://fxbug.dev/42072654): Figure out why this snapshot fails.
    #[fuchsia::test]
    async fn test_snapshot_hello_starnix() {
        spawn_kernel_and_run_with_pkgfs(async |current_task| {
            exec_hello_starnix(current_task).expect("failed to load executable");

            let current2 = current_task.clone_task_for_test(0, None);

            assert_eq!(
                current_task.mm().unwrap().get_mapping_count(),
                current2.mm().unwrap().get_mapping_count()
            );
        })
        .await;
    }

    #[::fuchsia::test]
    fn test_parse_interpreter_line() {
        assert_matches!(parse_interpreter_line(b"#!"), Err(_));
        assert_matches!(parse_interpreter_line(b"#!\n"), Err(_));
        assert_matches!(parse_interpreter_line(b"#! \n"), Err(_));
        assert_matches!(parse_interpreter_line(b"#!/bin/bash"), Err(_));
        assert_eq!(
            parse_interpreter_line(b"#!/bin/bash\x00\n"),
            Ok(vec![CString::new("/bin/bash").unwrap()])
        );
        assert_eq!(
            parse_interpreter_line(b"#!/bin/bash\n"),
            Ok(vec![CString::new("/bin/bash").unwrap()])
        );
        assert_eq!(
            parse_interpreter_line(b"#!/bin/bash -e\n"),
            Ok(vec![CString::new("/bin/bash").unwrap(), CString::new("-e").unwrap()])
        );
        assert_eq!(
            parse_interpreter_line(b"#!/bin/bash -e \n"),
            Ok(vec![CString::new("/bin/bash").unwrap(), CString::new("-e").unwrap()])
        );
        assert_eq!(
            parse_interpreter_line(b"#!/bin/bash \t -e\n"),
            Ok(vec![CString::new("/bin/bash").unwrap(), CString::new("-e").unwrap()])
        );
        assert_eq!(
            parse_interpreter_line(b"#!/bin/bash -e -x\n"),
            Ok(vec![CString::new("/bin/bash").unwrap(), CString::new("-e -x").unwrap(),])
        );
        assert_eq!(
            parse_interpreter_line(b"#!/bin/bash -e  -x\t-l\n"),
            Ok(vec![CString::new("/bin/bash").unwrap(), CString::new("-e  -x\t-l").unwrap(),])
        );
        assert_eq!(
            parse_interpreter_line(b"#!/bin/bash\nfoobar"),
            Ok(vec![CString::new("/bin/bash").unwrap()])
        );
        assert_eq!(
            parse_interpreter_line(b"#! /bin/bash -e  -x\t-l\n"),
            Ok(vec![CString::new("/bin/bash").unwrap(), CString::new("-e  -x\t-l").unwrap(),])
        );
        assert_eq!(
            parse_interpreter_line(b"#!\t/bin/bash \t-l\n"),
            Ok(vec![CString::new("/bin/bash").unwrap(), CString::new("-l").unwrap(),])
        );
    }
}
