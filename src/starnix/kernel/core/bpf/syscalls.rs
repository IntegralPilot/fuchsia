// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://github.com/rust-lang/rust/issues/39371): remove
#![allow(non_upper_case_globals)]

use crate::bpf::attachments::{BpfAttachAttr, bpf_prog_attach, bpf_prog_detach};
use crate::bpf::fs::{BpfFsDir, BpfHandle, get_bpf_object, resolve_pinned_bpf_object};
use crate::bpf::map::{self, BpfMap, BpfMapHandle};
use crate::bpf::program::{Program, ProgramInfo};
use crate::mm::{MemoryAccessor, MemoryAccessorExt};
use crate::security;
use crate::task::CurrentTask;
use crate::vfs::socket::{Socket, ZxioBackedSocket};
use crate::vfs::{Anon, FdFlags, FdNumber, LookupContext, OutputBuffer, UserBuffersOutputBuffer};
use ebpf::{EbpfInstruction, MapFlags, MapSchema};
use ebpf_api::{MapKey, ProgramType};
use smallvec::smallvec;
use starnix_logging::{log_error, log_trace, log_warn, track_stub};
use starnix_syscalls::{SUCCESS, SyscallResult};
use starnix_types::user_buffer::UserBuffer;
use starnix_uapi::auth::CAP_SYS_ADMIN;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{UserAddress, UserCString, UserRef};
use starnix_uapi::{
    BPF_F_RDONLY, BPF_F_WRONLY, bpf_attr__bindgen_ty_1, bpf_attr__bindgen_ty_2,
    bpf_attr__bindgen_ty_4, bpf_attr__bindgen_ty_5, bpf_attr__bindgen_ty_8, bpf_attr__bindgen_ty_9,
    bpf_attr__bindgen_ty_10, bpf_attr__bindgen_ty_12, bpf_cmd, bpf_cmd_BPF_BTF_GET_FD_BY_ID,
    bpf_cmd_BPF_BTF_GET_NEXT_ID, bpf_cmd_BPF_BTF_LOAD, bpf_cmd_BPF_ENABLE_STATS,
    bpf_cmd_BPF_ITER_CREATE, bpf_cmd_BPF_LINK_CREATE, bpf_cmd_BPF_LINK_DETACH,
    bpf_cmd_BPF_LINK_GET_FD_BY_ID, bpf_cmd_BPF_LINK_GET_NEXT_ID, bpf_cmd_BPF_LINK_UPDATE,
    bpf_cmd_BPF_MAP_CREATE, bpf_cmd_BPF_MAP_DELETE_BATCH, bpf_cmd_BPF_MAP_DELETE_ELEM,
    bpf_cmd_BPF_MAP_FREEZE, bpf_cmd_BPF_MAP_GET_FD_BY_ID, bpf_cmd_BPF_MAP_GET_NEXT_ID,
    bpf_cmd_BPF_MAP_GET_NEXT_KEY, bpf_cmd_BPF_MAP_LOOKUP_AND_DELETE_BATCH,
    bpf_cmd_BPF_MAP_LOOKUP_AND_DELETE_ELEM, bpf_cmd_BPF_MAP_LOOKUP_BATCH,
    bpf_cmd_BPF_MAP_LOOKUP_ELEM, bpf_cmd_BPF_MAP_UPDATE_BATCH, bpf_cmd_BPF_MAP_UPDATE_ELEM,
    bpf_cmd_BPF_OBJ_GET, bpf_cmd_BPF_OBJ_GET_INFO_BY_FD, bpf_cmd_BPF_OBJ_PIN,
    bpf_cmd_BPF_PROG_ATTACH, bpf_cmd_BPF_PROG_BIND_MAP, bpf_cmd_BPF_PROG_DETACH,
    bpf_cmd_BPF_PROG_GET_FD_BY_ID, bpf_cmd_BPF_PROG_GET_NEXT_ID, bpf_cmd_BPF_PROG_LOAD,
    bpf_cmd_BPF_PROG_QUERY, bpf_cmd_BPF_PROG_RUN, bpf_cmd_BPF_RAW_TRACEPOINT_OPEN,
    bpf_cmd_BPF_TASK_FD_QUERY, bpf_cmd_BPF_TOKEN_CREATE, bpf_map_info,
    bpf_map_type_BPF_MAP_TYPE_DEVMAP, bpf_map_type_BPF_MAP_TYPE_DEVMAP_HASH,
    bpf_map_type_BPF_MAP_TYPE_SK_STORAGE, bpf_prog_info, errno, error,
};
use zerocopy::{FromBytes, IntoBytes};

/// Read the arguments for a BPF command. The ABI works like this: If the arguments struct
/// passed is larger than the kernel knows about, the excess must be zeros. Similarly, if the
/// arguments struct is smaller than the kernel knows about, the kernel fills the excess with
/// zero.
fn read_attr<Attr: FromBytes>(
    current_task: &CurrentTask,
    attr_addr: UserAddress,
    attr_size: u32,
) -> Result<Attr, Errno> {
    let mut attr_size = attr_size as usize;
    let sizeof_attr = std::mem::size_of::<Attr>();

    // Verify that the extra is all zeros.
    if attr_size > sizeof_attr {
        let tail_addr = attr_addr.checked_add(sizeof_attr).ok_or_else(|| errno!(EFAULT))?;
        let tail = current_task.read_memory_to_vec(tail_addr, attr_size - sizeof_attr)?;
        if tail.into_iter().any(|byte| byte != 0) {
            return error!(E2BIG);
        }

        attr_size = sizeof_attr;
    }

    // If the struct passed is smaller than our definition of the struct, let whatever is not
    // passed be zero.
    current_task.read_object_partial(UserRef::new(attr_addr), attr_size)
}

fn reopen_bpf_fd(
    current_task: &CurrentTask,
    handle: BpfHandle,
    open_flags: OpenFlags,
) -> Result<SyscallResult, Errno> {
    // All BPF FDs have the CLOEXEC flag turned on by default, and use a private anonymous `FsNode`,
    // so that `FsNode` access checks will not be performed.
    let name = handle.type_name();
    let file = Anon::new_private_file(
        current_task,
        Box::new(handle),
        open_flags | OpenFlags::CLOEXEC,
        name,
    );
    Ok(current_task.add_file(file, FdFlags::CLOEXEC)?.into())
}

fn install_bpf_fd(
    current_task: &CurrentTask,
    obj: impl Into<BpfHandle>,
) -> Result<SyscallResult, Errno> {
    let handle: BpfHandle = obj.into();
    handle.security_check_open_fd(current_task, None)?;
    let name = handle.type_name();

    // All BPF FDs have the CLOEXEC flag turned on by default.
    let file = Anon::new_private_file(
        current_task,
        Box::new(handle),
        OpenFlags::RDWR | OpenFlags::CLOEXEC,
        name,
    );
    Ok(current_task.add_file(file, FdFlags::CLOEXEC)?.into())
}

#[derive(Debug, Clone)]
pub struct BpfTypeFormat {
    #[allow(dead_code)]
    data: Vec<u8>,
}

fn read_map_key(
    current_task: &CurrentTask,
    addr: UserAddress,
    map: &BpfMapHandle,
) -> Result<MapKey, Errno> {
    let key_size = map.schema.key_size as usize;
    let mut key = current_task.read_objects_to_smallvec(UserRef::<u8>::new(addr), key_size)?;

    // With sk_storage maps the key is interpreted as a socket file descriptor.
    if map.schema.map_type == bpf_map_type_BPF_MAP_TYPE_SK_STORAGE {
        let fd = FdNumber::from_raw(
            i32::read_from_bytes(&key[..]).expect("invalid key size in sk_storage map"),
        );
        let file = current_task.files().get(fd)?;
        let socket = Socket::get_from_file(&file)?;
        let socket = socket.downcast_socket::<ZxioBackedSocket>().ok_or_else(|| errno!(EINVAL))?;
        let cookie = socket.get_socket_cookie()?;
        key = MapKey::from_slice(cookie.as_bytes());
    }

    Ok(key)
}

fn validate_bpf_name(name: &[u8]) -> Result<&str, Errno> {
    let name = std::ffi::CStr::from_bytes_until_nul(name)
        .map_err(|_| errno!(EINVAL))?
        .to_str()
        .map_err(|_| errno!(EINVAL))?;
    // Only alphanumeric characters, '_' and '.' are allowed in map names (see
    // https://docs.kernel.org/bpf/maps.html).
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
        return error!(EINVAL);
    }
    Ok(name)
}

pub fn sys_bpf(
    current_task: &CurrentTask,
    cmd: bpf_cmd,
    attr_addr: UserAddress,
    attr_size: u32,
) -> Result<SyscallResult, Errno> {
    // TODO: https://fxbug.dev/322874504 - Allow containers to configure the kernel's initial
    // "unprivileged_bpf_disabled" setting, and apply capability checks appropriately.

    // The best available documentation on the various BPF commands is at
    // https://www.kernel.org/doc/html/latest/userspace-api/ebpf/syscall.html.
    // Comments on commands are copied from there.

    match cmd {
        // Create a map and return a file descriptor that refers to the map.
        bpf_cmd_BPF_MAP_CREATE => {
            let map_attr: bpf_attr__bindgen_ty_1 = read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_MAP_CREATE {:?}", map_attr);
            security::check_bpf_access(current_task, cmd, &map_attr, attr_size)?;

            let map_type = map_attr.map_type;
            let mut flags =
                MapFlags::from_bits(map_attr.map_flags).ok_or_else(|| errno!(EINVAL))?;
            // To quote
            // https://cs.android.com/android/platform/superproject/+/master:system/bpf/libbpf_android/Loader.cpp;l=670;drc=28e295395471b33e662b7116378d15f1e88f0864
            // "DEVMAPs are readonly from the bpf program side's point of view, as such the kernel
            // in kernel/bpf/devmap.c dev_map_init_map() will set the flag"
            if map_type == bpf_map_type_BPF_MAP_TYPE_DEVMAP
                || map_type == bpf_map_type_BPF_MAP_TYPE_DEVMAP_HASH
            {
                flags |= MapFlags::ProgReadOnly;
            }

            let schema = MapSchema {
                map_type,
                key_size: map_attr.key_size,
                value_size: map_attr.value_size,
                max_entries: map_attr.max_entries,
                flags,
            };

            let name = validate_bpf_name(map_attr.map_name.as_bytes())?;
            let map =
                BpfMap::new(current_task, schema, name, security::bpf_map_alloc(current_task))?;
            install_bpf_fd(current_task, map)
        }

        bpf_cmd_BPF_MAP_LOOKUP_ELEM => {
            let elem_attr: bpf_attr__bindgen_ty_2 = read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_MAP_LOOKUP_ELEM");
            security::check_bpf_access(current_task, cmd, &elem_attr, attr_size)?;
            let map_fd = FdNumber::from_raw(elem_attr.map_fd as i32);
            let map = get_bpf_object(current_task, map_fd)?;
            let map = map.as_map()?;

            if map.schema.flags.contains(MapFlags::SyscallWriteOnly) {
                return error!(EPERM);
            }

            let key = read_map_key(current_task, UserAddress::from(elem_attr.key), map)?;

            // SAFETY: this union object was created with FromBytes so it's safe to access any
            // variant because all variants must be valid with all bit patterns.
            let user_value = UserAddress::from(unsafe { elem_attr.__bindgen_anon_1.value });

            let _suspend_lock =
                current_task.kernel().suspend_resume_manager.acquire_ebpf_suspend_lock();

            let value = map.load(&key).ok_or_else(|| errno!(ENOENT))?;
            current_task.write_memory(user_value, &value)?;

            Ok(SUCCESS)
        }

        // Create or update an element (key/value pair) in a specified map.
        bpf_cmd_BPF_MAP_UPDATE_ELEM => {
            let elem_attr: bpf_attr__bindgen_ty_2 = read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_MAP_UPDATE_ELEM");
            security::check_bpf_access(current_task, cmd, &elem_attr, attr_size)?;
            let map_fd = FdNumber::from_raw(elem_attr.map_fd as i32);
            let map = get_bpf_object(current_task, map_fd)?;
            let map = map.as_map()?;

            // Get the frozen state and keep the lock to prevent a race.
            let frozen = map.frozen();

            if *frozen || map.schema.flags.contains(MapFlags::SyscallReadOnly) {
                return error!(EPERM);
            }

            let flags = elem_attr.flags;
            let key = read_map_key(current_task, UserAddress::from(elem_attr.key), map)?;

            // SAFETY: this union object was created with FromBytes so it's safe to access any
            // variant because all variants must be valid with all bit patterns.
            let user_value = UserAddress::from(unsafe { elem_attr.__bindgen_anon_1.value });
            let mut value =
                current_task.read_memory_to_vec(user_value, map.schema.value_size as usize)?;

            let _suspend_lock =
                current_task.kernel().suspend_resume_manager.acquire_ebpf_suspend_lock();

            map.update(&key[..], value.as_mut_bytes().into(), flags)
                .map_err(map::map_error_to_errno)?;
            Ok(SUCCESS)
        }

        bpf_cmd_BPF_MAP_DELETE_ELEM => {
            let elem_attr: bpf_attr__bindgen_ty_2 = read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_MAP_DELETE_ELEM");
            security::check_bpf_access(current_task, cmd, &elem_attr, attr_size)?;
            let map_fd = FdNumber::from_raw(elem_attr.map_fd as i32);
            let map = get_bpf_object(current_task, map_fd)?;
            let map = map.as_map()?;

            // Get the frozen state and keep the lock to prevent a race.
            let frozen = map.frozen();

            if *frozen || map.schema.flags.contains(MapFlags::SyscallReadOnly) {
                return error!(EPERM);
            }

            let key = read_map_key(current_task, UserAddress::from(elem_attr.key), map)?;

            let _suspend_lock =
                current_task.kernel().suspend_resume_manager.acquire_ebpf_suspend_lock();

            map.delete(&key).map_err(map::map_error_to_errno)?;
            Ok(SUCCESS)
        }

        // Look up an element by key in a specified map and return the key of the next element. Can
        // be used to iterate over all elements in the map.
        bpf_cmd_BPF_MAP_GET_NEXT_KEY => {
            let elem_attr: bpf_attr__bindgen_ty_2 = read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_MAP_GET_NEXT_KEY");
            security::check_bpf_access(current_task, cmd, &elem_attr, attr_size)?;
            let map_fd = FdNumber::from_raw(elem_attr.map_fd as i32);
            let map = get_bpf_object(current_task, map_fd)?;
            let map = map.as_map()?;

            if map.schema.flags.contains(MapFlags::SyscallWriteOnly) {
                return error!(EPERM);
            }

            let key = if elem_attr.key != 0 {
                Some(read_map_key(current_task, UserAddress::from(elem_attr.key), map)?)
            } else {
                None
            };

            let next_key =
                map.get_next_key(key.as_ref().map(|k| &k[..])).map_err(map::map_error_to_errno)?;

            // SAFETY: this union object was created with FromBytes so it's safe to access any
            // variant (right?)
            let user_next_key = UserAddress::from(unsafe { elem_attr.__bindgen_anon_1.next_key });
            current_task.write_memory(user_next_key, &next_key)?;

            Ok(SUCCESS)
        }

        // Verify and load an eBPF program, returning a new file descriptor associated with the
        // program.
        bpf_cmd_BPF_PROG_LOAD => {
            let prog_attr: bpf_attr__bindgen_ty_4 = read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_PROG_LOAD");
            security::check_bpf_access(current_task, cmd, &prog_attr, attr_size)?;

            let user_code = UserRef::<EbpfInstruction>::new(UserAddress::from(prog_attr.insns));
            let code = current_task.read_objects_to_vec(user_code, prog_attr.insn_cnt as usize)?;

            let mut log_buffer = if prog_attr.log_buf != 0 && prog_attr.log_size > 1 {
                UserBuffersOutputBuffer::unified_new(
                    current_task,
                    smallvec![UserBuffer {
                        address: prog_attr.log_buf.into(),
                        length: (prog_attr.log_size - 1) as usize
                    }],
                )?
            } else {
                UserBuffersOutputBuffer::unified_new(current_task, smallvec![])?
            };
            let name = validate_bpf_name(prog_attr.prog_name.as_bytes())?;
            let info = ProgramInfo::try_from(&prog_attr)?;
            let program_type = info.program_type;
            let program = Program::new(current_task, info, &mut log_buffer, code);
            let program_or_stub = match (program, program_type) {
                (Ok(program), _) => BpfHandle::Program(program),

                // Create a stub only if it's allowed for the `program_type`
                // and bpf_v2 is not enabled.
                (Err(e), ProgramType::SockOps | ProgramType::SchedCls | ProgramType::Kprobe)
                    if !current_task.kernel().features.bpf_v2 =>
                {
                    log_warn!(
                        "Creating a stub for eBPF program {name}, type={program_type:?}: {e:?}"
                    );
                    BpfHandle::ProgramStub(prog_attr.prog_type)
                }

                (Err(e), _) => {
                    log_error!("Unable to load eBPF program {name}, type={program_type:?}: {e:?}");
                    return Err(e.into());
                }
            };
            // Ensures the log buffer ends with a 0.
            log_buffer.write(b"\0")?;
            install_bpf_fd(current_task, program_or_stub)
        }

        // Attach an eBPF program to a target_fd at the specified attach_type hook.
        bpf_cmd_BPF_PROG_ATTACH => {
            let attach_attr: BpfAttachAttr = read_attr(current_task, attr_addr, attr_size)?;
            security::check_bpf_access(current_task, cmd, &attach_attr, attr_size)?;
            bpf_prog_attach(current_task, attach_attr)
        }

        // Obtain information about eBPF programs associated with the specified attach_type hook.
        bpf_cmd_BPF_PROG_QUERY => {
            let mut prog_attr: bpf_attr__bindgen_ty_10 =
                read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_PROG_QUERY");
            security::check_bpf_access(current_task, cmd, &prog_attr, attr_size)?;
            track_stub!(TODO("https://fxbug.dev/322873416"), "Bpf::BPF_PROG_QUERY");
            current_task.write_memory(UserAddress::from(prog_attr.prog_ids), 1.as_bytes())?;
            prog_attr.__bindgen_anon_2.prog_cnt = std::mem::size_of::<u64>() as u32;
            current_task.write_memory(attr_addr, prog_attr.as_bytes())?;
            Ok(SUCCESS)
        }

        // Pin an eBPF program or map referred by the specified bpf_fd to the provided pathname on
        // the filesystem.
        bpf_cmd_BPF_OBJ_PIN => {
            let pin_attr: bpf_attr__bindgen_ty_5 = read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_OBJ_PIN {:?}", pin_attr);
            security::check_bpf_access(current_task, cmd, &pin_attr, attr_size)?;
            let bpf_fd = FdNumber::from_raw(pin_attr.bpf_fd as i32);
            let object = get_bpf_object(current_task, bpf_fd)?;
            let path_addr = UserCString::new(current_task, UserAddress::from(pin_attr.pathname));
            let pathname = current_task.read_path(path_addr)?;
            let (parent, basename) = current_task.lookup_parent_at(
                &mut LookupContext::default(),
                FdNumber::AT_FDCWD,
                pathname.as_ref(),
            )?;
            let bpf_dir =
                parent.entry.node.downcast_ops::<BpfFsDir>().ok_or_else(|| errno!(EINVAL))?;
            bpf_dir.register_pin(current_task, &parent, basename, object)?;
            Ok(SUCCESS)
        }

        // Open a file descriptor for the eBPF object pinned to the specified pathname.
        bpf_cmd_BPF_OBJ_GET => {
            let path_attr: bpf_attr__bindgen_ty_5 = read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_OBJ_GET {:?}", path_attr);
            security::check_bpf_access(current_task, cmd, &path_attr, attr_size)?;
            let path_addr = UserCString::new(current_task, UserAddress::from(path_attr.pathname));
            let open_flags = match path_attr.file_flags {
                BPF_F_RDONLY => OpenFlags::RDONLY,
                BPF_F_WRONLY => OpenFlags::WRONLY,
                0 => OpenFlags::RDWR,
                _ => return error!(EINVAL),
            };
            let pathname = current_task.read_path(path_addr)?;
            let handle = resolve_pinned_bpf_object(current_task, pathname.as_ref(), open_flags)?;
            reopen_bpf_fd(current_task, handle, open_flags)
        }

        // Obtain information about the eBPF object corresponding to bpf_fd.
        bpf_cmd_BPF_OBJ_GET_INFO_BY_FD => {
            let mut get_info_attr: bpf_attr__bindgen_ty_9 =
                read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_OBJ_GET_INFO_BY_FD {:?}", get_info_attr);
            security::check_bpf_access(current_task, cmd, &get_info_attr, attr_size)?;
            let bpf_fd = FdNumber::from_raw(get_info_attr.bpf_fd as i32);
            let object = get_bpf_object(current_task, bpf_fd)?;

            let mut info = match object {
                BpfHandle::Map(map) => bpf_map_info {
                    type_: map.schema.map_type,
                    id: map.id(),
                    key_size: map.schema.key_size,
                    value_size: map.schema.value_size,
                    max_entries: map.schema.max_entries,
                    map_flags: map.schema.flags.bits(),
                    ..Default::default()
                }
                .as_bytes()
                .to_owned(),
                BpfHandle::Program(prog) => {
                    #[allow(unknown_lints, clippy::unnecessary_struct_initialization)]
                    bpf_prog_info {
                        type_: prog.info.program_type.into(),
                        id: prog.id(),
                        // TODO: https://fxbug.dev/397389704 - return actual length.
                        jited_prog_len: 1,
                        ..Default::default()
                    }
                    .as_bytes()
                    .to_owned()
                }
                BpfHandle::ProgramStub(type_) => {
                    #[allow(unknown_lints, clippy::unnecessary_struct_initialization)]
                    bpf_prog_info {
                        type_,
                        // TODO: https://fxbug.dev/397389704 - return actual length.
                        jited_prog_len: 1,
                        ..Default::default()
                    }
                    .as_bytes()
                    .to_owned()
                }
                _ => {
                    return error!(EINVAL);
                }
            };

            // If info_len is larger than info, write out the full length of info and write the
            // smaller size into info_len. If info_len is smaller, truncate info.
            // TODO(tbodt): This is just a guess for the behavior. Works with BpfSyscallWrappers.h,
            // but could be wrong.
            info.truncate(get_info_attr.info_len as usize);
            get_info_attr.info_len = info.len() as u32;
            current_task.write_memory(UserAddress::from(get_info_attr.info), &info)?;
            current_task.write_memory(attr_addr, get_info_attr.as_bytes())?;
            Ok(SUCCESS)
        }

        // Verify and load BPF Type Format (BTF) metadata into the kernel, returning a new file
        // descriptor associated with the metadata. BTF is described in more detail at
        // https://www.kernel.org/doc/html/latest/bpf/btf.html.
        bpf_cmd_BPF_BTF_LOAD => {
            let btf_attr: bpf_attr__bindgen_ty_12 = read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_BTF_LOAD {:?}", btf_attr);
            security::check_bpf_access(current_task, cmd, &btf_attr, attr_size)?;
            let data = current_task
                .read_memory_to_vec(UserAddress::from(btf_attr.btf), btf_attr.btf_size as usize)?;
            install_bpf_fd(current_task, BpfTypeFormat { data })
        }
        bpf_cmd_BPF_PROG_DETACH => {
            let attach_attr: BpfAttachAttr = read_attr(current_task, attr_addr, attr_size)?;
            security::check_bpf_access(current_task, cmd, &attach_attr, attr_size)?;
            bpf_prog_detach(current_task, attach_attr)
        }
        bpf_cmd_BPF_PROG_RUN => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_PROG_RUN");
            error!(EINVAL)
        }
        bpf_cmd_BPF_PROG_GET_NEXT_ID => {
            security::check_task_capable(current_task, CAP_SYS_ADMIN)?;
            let mut get_next_attr: bpf_attr__bindgen_ty_8 =
                read_attr(current_task, attr_addr, attr_size)?;
            // SAFETY: Reading u32 value from a union is safe.
            let start_id = unsafe { get_next_attr.__bindgen_anon_1.start_id };
            get_next_attr.next_id = current_task
                .kernel()
                .ebpf_state
                .get_next_program_id(start_id)
                .ok_or_else(|| errno!(ENOENT))?;
            current_task.write_object(UserRef::new(attr_addr), &get_next_attr)?;
            Ok(SUCCESS)
        }
        bpf_cmd_BPF_MAP_GET_NEXT_ID => {
            security::check_task_capable(current_task, CAP_SYS_ADMIN)?;
            let mut get_next_attr: bpf_attr__bindgen_ty_8 =
                read_attr(current_task, attr_addr, attr_size)?;
            // SAFETY: Reading u32 value from a union is safe.
            let start_id = unsafe { get_next_attr.__bindgen_anon_1.start_id };
            get_next_attr.next_id = current_task
                .kernel()
                .ebpf_state
                .get_next_map_id(start_id)
                .ok_or_else(|| errno!(ENOENT))?;
            current_task.write_object(UserRef::new(attr_addr), &get_next_attr)?;
            Ok(SUCCESS)
        }
        bpf_cmd_BPF_PROG_GET_FD_BY_ID => {
            security::check_task_capable(current_task, CAP_SYS_ADMIN)?;
            let get_by_id_attr: bpf_attr__bindgen_ty_8 =
                read_attr(current_task, attr_addr, attr_size)?;
            // SAFETY: Reading u32 value from a union is safe.
            let prog_id = unsafe { get_by_id_attr.__bindgen_anon_1.prog_id };
            let prog = current_task
                .kernel()
                .ebpf_state
                .get_program_by_id(prog_id)
                .ok_or_else(|| errno!(ENOENT))?;
            install_bpf_fd(current_task, prog)
        }
        bpf_cmd_BPF_MAP_GET_FD_BY_ID => {
            security::check_task_capable(current_task, CAP_SYS_ADMIN)?;
            let get_by_id_attr: bpf_attr__bindgen_ty_8 =
                read_attr(current_task, attr_addr, attr_size)?;
            // SAFETY: Reading u32 value from a union is safe.
            let map_id = unsafe { get_by_id_attr.__bindgen_anon_1.map_id };
            let map = current_task
                .kernel()
                .ebpf_state
                .get_map_by_id(map_id)
                .ok_or_else(|| errno!(ENOENT))?;
            install_bpf_fd(current_task, map)
        }
        bpf_cmd_BPF_RAW_TRACEPOINT_OPEN => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_RAW_TRACEPOINT_OPEN");
            error!(EINVAL)
        }
        bpf_cmd_BPF_BTF_GET_FD_BY_ID => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_BTF_GET_FD_BY_ID");
            error!(EINVAL)
        }
        bpf_cmd_BPF_TASK_FD_QUERY => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_TASK_FD_QUERY");
            error!(EINVAL)
        }
        bpf_cmd_BPF_MAP_LOOKUP_AND_DELETE_ELEM => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_MAP_LOOKUP_AND_DELETE_ELEM");
            error!(EINVAL)
        }
        bpf_cmd_BPF_MAP_FREEZE => {
            let elem_attr: bpf_attr__bindgen_ty_2 = read_attr(current_task, attr_addr, attr_size)?;
            log_trace!("BPF_MAP_FREEZE");
            let map_fd = FdNumber::from_raw(elem_attr.map_fd as i32);
            let map = get_bpf_object(current_task, map_fd)?;
            let map = map.as_map()?;
            map.freeze()?;
            Ok(SUCCESS)
        }
        bpf_cmd_BPF_BTF_GET_NEXT_ID => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_BTF_GET_NEXT_ID");
            error!(EINVAL)
        }
        bpf_cmd_BPF_MAP_LOOKUP_BATCH => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_MAP_LOOKUP_BATCH");
            error!(EINVAL)
        }
        bpf_cmd_BPF_MAP_LOOKUP_AND_DELETE_BATCH => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_MAP_LOOKUP_AND_DELETE_BATCH");
            error!(EINVAL)
        }
        bpf_cmd_BPF_MAP_UPDATE_BATCH => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_MAP_UPDATE_BATCH");
            error!(EINVAL)
        }
        bpf_cmd_BPF_MAP_DELETE_BATCH => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_MAP_DELETE_BATCH");
            error!(EINVAL)
        }
        bpf_cmd_BPF_LINK_CREATE => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_LINK_CREATE");
            error!(EINVAL)
        }
        bpf_cmd_BPF_LINK_UPDATE => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_LINK_UPDATE");
            error!(EINVAL)
        }
        bpf_cmd_BPF_LINK_GET_FD_BY_ID => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_LINK_GET_FD_BY_ID");
            error!(EINVAL)
        }
        bpf_cmd_BPF_LINK_GET_NEXT_ID => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_LINK_GET_NEXT_ID");
            error!(EINVAL)
        }
        bpf_cmd_BPF_ENABLE_STATS => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_ENABLE_STATS");
            error!(EINVAL)
        }
        bpf_cmd_BPF_ITER_CREATE => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_ITER_CREATE");
            error!(EINVAL)
        }
        bpf_cmd_BPF_LINK_DETACH => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_LINK_DETACH");
            error!(EINVAL)
        }
        bpf_cmd_BPF_PROG_BIND_MAP => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_PROG_BIND_MAP");
            error!(EINVAL)
        }
        bpf_cmd_BPF_TOKEN_CREATE => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "BPF_TOKEN_CREATE");
            error!(EINVAL)
        }
        _ => {
            track_stub!(TODO("https://fxbug.dev/322874055"), "bpf", cmd);
            error!(EINVAL)
        }
    }
}

// Syscalls for arch32 usage
#[cfg(target_arch = "aarch64")]
mod arch32 {
    pub use super::sys_bpf as sys_arch32_bpf;
}

#[cfg(target_arch = "aarch64")]
pub use arch32::*;
