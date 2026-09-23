// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bitflags::bitflags;
use linux_uapi::bpf_map_type;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct MapFlags: u32 {
        const NoPrealloc = linux_uapi::BPF_F_NO_PREALLOC;
        const NoCommonLru = linux_uapi::BPF_F_NO_COMMON_LRU;
        const NumaNode = linux_uapi::BPF_F_NUMA_NODE;
        const SyscallReadOnly = linux_uapi::BPF_F_RDONLY;
        const SyscallWriteOnly = linux_uapi::BPF_F_WRONLY;
        const StackBuildId = linux_uapi::BPF_F_STACK_BUILD_ID;
        const ZeroSeed = linux_uapi::BPF_F_ZERO_SEED;
        const ProgReadOnly = linux_uapi::BPF_F_RDONLY_PROG;
        const ProgWriteOnly = linux_uapi::BPF_F_WRONLY_PROG;
        const Clone = linux_uapi::BPF_F_CLONE;
        const Mmapable = linux_uapi::BPF_F_MMAPABLE;
        const PreserveElems = linux_uapi::BPF_F_PRESERVE_ELEMS;
        const InnerMap = linux_uapi::BPF_F_INNER_MAP;
        const Link = linux_uapi::BPF_F_LINK;
        const PathFd = linux_uapi::BPF_F_PATH_FD;
        const VtypeBtfObjFd = linux_uapi::BPF_F_VTYPE_BTF_OBJ_FD;
        const TokenFd = linux_uapi::BPF_F_TOKEN_FD;
        const SegvOnFault = linux_uapi::BPF_F_SEGV_ON_FAULT;
        const NoUserConv = linux_uapi::BPF_F_NO_USER_CONV;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapSchema {
    pub map_type: bpf_map_type,
    pub key_size: u32,
    pub value_size: u32,
    pub max_entries: u32,
    pub flags: MapFlags,
}
