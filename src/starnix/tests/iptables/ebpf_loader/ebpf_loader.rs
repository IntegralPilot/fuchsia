// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::ffi::CString;
use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd};

use libc;
use linux_uapi::bpf_attr;
use zerocopy::FromBytes as _;

/// bpf() syscall wrapper.
unsafe fn bpf(command: linux_uapi::bpf_cmd, attr: &bpf_attr) -> Result<i32, std::io::Error> {
    // SAFETY: Caller is responsible for ensuring that the syscall arguments are valid.
    let result = unsafe {
        libc::syscall(
            linux_uapi::__NR_bpf.into(),
            command,
            attr as *const bpf_attr,
            std::mem::size_of_val(attr),
        )
    };
    (result >= 0)
        .then_some(result as i32)
        .ok_or_else(|| std::io::Error::from_raw_os_error(-result as i32))
}

/// Returns a zeroed bpf_attr.
fn zero_bpf_attr() -> bpf_attr {
    bpf_attr::read_from_bytes(&[0; std::mem::size_of::<bpf_attr>()])
        .expect("Failed to create bpf_attr")
}

/// Loads a BPF program.
fn bpf_prog_load(code: Vec<ebpf::EbpfInstruction>) -> Result<OwnedFd, std::io::Error> {
    let mut attr = zero_bpf_attr();

    let mut log = vec![0; 4096];
    let license = b"N/A\0";

    // SAFETY: `attr` is zeroed, so it's safe to access any union variant.
    let load_prog_attr = unsafe { &mut attr.__bindgen_anon_3 };
    load_prog_attr.prog_type = linux_uapi::bpf_prog_type_BPF_PROG_TYPE_SOCKET_FILTER;
    load_prog_attr.insns = code.as_ptr() as u64;
    load_prog_attr.insn_cnt = code.len() as u32;
    load_prog_attr.log_level = 1;
    load_prog_attr.log_size = 4096;
    load_prog_attr.log_buf = log.as_mut_ptr() as u64;
    load_prog_attr.license = license.as_ptr() as u64;

    // SAFETY: Calling bpf() syscall valid arguments.
    let result = unsafe { bpf(linux_uapi::bpf_cmd_BPF_PROG_LOAD, &attr) };

    // SAFETY: result is an FD when non-negative.
    result.map(|fd| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Pins a BPF object at the specified path.
fn bpf_obj_pin(fd: OwnedFd, pin_path: &str) -> Result<(), std::io::Error> {
    let pin_path = CString::new(pin_path).expect("Failed to create CString");
    let mut attr = zero_bpf_attr();
    attr.__bindgen_anon_4.pathname = pin_path.as_ptr() as u64;
    attr.__bindgen_anon_4.bpf_fd = fd.as_raw_fd() as u32;

    // SAFETY: Calling bpf() syscall valid arguments.
    unsafe { bpf(linux_uapi::bpf_cmd_BPF_OBJ_PIN, &attr) }.map(|_| ())
}

fn main() {
    let mut pinned_name = String::new();
    std::io::stdin().read_line(&mut pinned_name).expect("Failed to read stdin");
    let pinned_name = pinned_name.trim();

    // Load eBPF program.
    let code = [
        // r0 <- 0 (BPF_ALU64 | BPF_MOV | BPF_K)
        0xB700_0000_0000_0000,
        // exit (BPF_JMP | BPF_EXIT)
        0x9500_0000_0000_0000,
    ]
    .into_iter()
    .map(|x| u64::from_be(x).into())
    .collect();

    let program = bpf_prog_load(code).expect("Failed to load eBPF program");

    // Pin the program.
    bpf_obj_pin(program, &pinned_name).expect("Failed to pin eBPF program");
}
