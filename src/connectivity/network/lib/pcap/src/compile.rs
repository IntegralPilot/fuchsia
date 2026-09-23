// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_ebpf as febpf;
use fidl_fuchsia_net_debug as fnet_debug;

use std::ffi::{CString, NulError};
use std::os::raw::c_int;
use thiserror::Error;

use crate::bindings::{bpf_program, pcap_close, pcap_compile, pcap_freecode, pcap_open_dead};

/// Errors that can occur while compiling a pcap filter.
#[derive(Error, Debug)]
pub enum CompilationError {
    /// pcap_open_dead failed.
    #[error("pcap_open_dead failed")]
    OpenDeadFailed,

    /// pcap_compile failed.
    #[error("pcap_compile failed")]
    CompileFailed,

    /// The filter string contained a null byte.
    #[error("filter string contained a null byte")]
    NullByteInFilter,
}

// This value comes from pcap.h.
//
// See https://github.com/the-tcpdump-group/libpcap/blob/78415bc13031d145dc24d3c3ffffc281d3d629f1/pcap/pcap.h#L387
const PCAP_NETMASK_UNKNOWN: u32 = 0xffffffff;

/// Compiles a pcap filter string to BPF bytecode.
///
/// Currently the pcap filter is intended for Ethernet links.
pub fn compile_filter(filter: &str) -> Result<febpf::VerifiedProgram, CompilationError> {
    let c_filter =
        CString::new(filter).map_err(|_: NulError| CompilationError::NullByteInFilter)?;

    let link_type: c_int = u16::from(crate::LinkType::Ethernet).try_into().expect("fits in c_int");
    let snap_len: c_int = fnet_debug::DEFAULT_SNAP_LEN.try_into().expect("fits in c_int");

    // SAFETY: `pcap_open_dead` does not dereference any pointers. It merely
    // allocates and returns a placeholder pcap handle.
    let handle = unsafe { pcap_open_dead(link_type, snap_len) };
    if handle.is_null() {
        return Err(CompilationError::OpenDeadFailed);
    }

    let mut program = bpf_program { bf_len: 0, bf_insns: std::ptr::null_mut() };

    // SAFETY:
    // - `handle` is a valid pcap handle returned by `pcap_open_dead` and is not null.
    // - `program` is a valid pointer to a stack-allocated `bpf_program`.
    // - `c_filter` is a valid null-terminated C string.
    let res = unsafe {
        pcap_compile(
            handle,
            &mut program,
            c_filter.as_ptr(),
            1, /* optimize */
            PCAP_NETMASK_UNKNOWN,
        )
    };
    if res != 0 {
        // SAFETY: `handle` is a valid pcap handle that has not been closed yet.
        unsafe {
            pcap_close(handle);
        }
        return Err(CompilationError::CompileFailed);
    }
    // SAFETY: `pcap_compile` succeeded, so `program.bf_insns` points to a valid
    // array of `program.bf_len` instructions allocated by libpcap. This data is
    // valid until `pcap_freecode` is called below. We copy the data out before
    // freeing it.
    let insns = unsafe { std::slice::from_raw_parts(program.bf_insns, program.bf_len as usize) };
    // Convert generated bpf_insn to linux_uapi::sock_filter.
    let cbpf_insns: &[linux_uapi::sock_filter] = zerocopy::transmute_ref!(insns);

    let verified = ebpf::converter::convert_and_verify_cbpf(
        cbpf_insns,
        ebpf_api::SOCKET_FILTER_SK_BUF_TYPE.clone(),
        &ebpf_api::SOCKET_FILTER_CBPF_CONFIG,
    )
    .expect("failed to convert cBPF to eBPF");
    // These must always be empty when converting cBPF to eBPF.
    // cBPF does not support maps or direct struct access.
    assert_eq!(verified.struct_access_instructions(), &[]);
    assert_eq!(verified.maps(), &[]);

    // SAFETY:
    // - `program` was successfully initialized by `pcap_compile`.
    // - `handle` is a valid pcap handle that has not been closed yet.
    unsafe {
        pcap_freecode(&mut program);
        pcap_close(handle);
    }

    Ok(febpf::VerifiedProgram {
        code: Some(verified.code().iter().map(ebpf::EbpfInstruction::get).collect()),
        struct_access_instructions: Some(Vec::new()),
        maps: Some(Vec::new()),
        ..Default::default()
    })
}
