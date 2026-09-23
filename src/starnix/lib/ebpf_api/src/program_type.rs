// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ebpf::{
    CallingContext, CbpfConfig, CbpfLenInstruction, FieldDescriptor, FieldType, FunctionSignature,
    HelperDefinition, MapSchema, MapTypeFilter, MemoryId, MemoryParameterSize, StructDescriptor,
    Type,
};
use fidl_fuchsia_ebpf as febpf;
use linux_uapi::{
    __sk_buff, bpf_attach_type_BPF_CGROUP_DEVICE, bpf_attach_type_BPF_CGROUP_GETSOCKOPT,
    bpf_attach_type_BPF_CGROUP_INET_EGRESS, bpf_attach_type_BPF_CGROUP_INET_INGRESS,
    bpf_attach_type_BPF_CGROUP_INET_SOCK_CREATE, bpf_attach_type_BPF_CGROUP_INET_SOCK_RELEASE,
    bpf_attach_type_BPF_CGROUP_INET4_BIND, bpf_attach_type_BPF_CGROUP_INET4_CONNECT,
    bpf_attach_type_BPF_CGROUP_INET4_GETPEERNAME, bpf_attach_type_BPF_CGROUP_INET4_GETSOCKNAME,
    bpf_attach_type_BPF_CGROUP_INET4_POST_BIND, bpf_attach_type_BPF_CGROUP_INET6_BIND,
    bpf_attach_type_BPF_CGROUP_INET6_CONNECT, bpf_attach_type_BPF_CGROUP_INET6_GETPEERNAME,
    bpf_attach_type_BPF_CGROUP_INET6_GETSOCKNAME, bpf_attach_type_BPF_CGROUP_INET6_POST_BIND,
    bpf_attach_type_BPF_CGROUP_SETSOCKOPT, bpf_attach_type_BPF_CGROUP_SOCK_OPS,
    bpf_attach_type_BPF_CGROUP_SYSCTL, bpf_attach_type_BPF_CGROUP_UDP4_RECVMSG,
    bpf_attach_type_BPF_CGROUP_UDP4_SENDMSG, bpf_attach_type_BPF_CGROUP_UDP6_RECVMSG,
    bpf_attach_type_BPF_CGROUP_UDP6_SENDMSG, bpf_attach_type_BPF_CGROUP_UNIX_CONNECT,
    bpf_attach_type_BPF_CGROUP_UNIX_GETPEERNAME, bpf_attach_type_BPF_CGROUP_UNIX_GETSOCKNAME,
    bpf_attach_type_BPF_CGROUP_UNIX_RECVMSG, bpf_attach_type_BPF_CGROUP_UNIX_SENDMSG,
    bpf_attach_type_BPF_FLOW_DISSECTOR, bpf_attach_type_BPF_LIRC_MODE2,
    bpf_attach_type_BPF_LSM_CGROUP, bpf_attach_type_BPF_LSM_MAC, bpf_attach_type_BPF_MODIFY_RETURN,
    bpf_attach_type_BPF_NETFILTER, bpf_attach_type_BPF_NETKIT_PEER,
    bpf_attach_type_BPF_NETKIT_PRIMARY, bpf_attach_type_BPF_PERF_EVENT,
    bpf_attach_type_BPF_SK_LOOKUP, bpf_attach_type_BPF_SK_MSG_VERDICT,
    bpf_attach_type_BPF_SK_REUSEPORT_SELECT, bpf_attach_type_BPF_SK_REUSEPORT_SELECT_OR_MIGRATE,
    bpf_attach_type_BPF_SK_SKB_STREAM_PARSER, bpf_attach_type_BPF_SK_SKB_STREAM_VERDICT,
    bpf_attach_type_BPF_SK_SKB_VERDICT, bpf_attach_type_BPF_STRUCT_OPS,
    bpf_attach_type_BPF_TCX_EGRESS, bpf_attach_type_BPF_TCX_INGRESS,
    bpf_attach_type_BPF_TRACE_FENTRY, bpf_attach_type_BPF_TRACE_FEXIT,
    bpf_attach_type_BPF_TRACE_ITER, bpf_attach_type_BPF_TRACE_KPROBE_MULTI,
    bpf_attach_type_BPF_TRACE_KPROBE_SESSION, bpf_attach_type_BPF_TRACE_RAW_TP,
    bpf_attach_type_BPF_TRACE_UPROBE_MULTI, bpf_attach_type_BPF_XDP,
    bpf_attach_type_BPF_XDP_CPUMAP, bpf_attach_type_BPF_XDP_DEVMAP,
    bpf_func_id_BPF_FUNC_csum_update, bpf_func_id_BPF_FUNC_get_current_pid_tgid,
    bpf_func_id_BPF_FUNC_get_current_uid_gid, bpf_func_id_BPF_FUNC_get_netns_cookie,
    bpf_func_id_BPF_FUNC_get_retval, bpf_func_id_BPF_FUNC_get_smp_processor_id,
    bpf_func_id_BPF_FUNC_get_socket_cookie, bpf_func_id_BPF_FUNC_get_socket_uid,
    bpf_func_id_BPF_FUNC_ktime_get_boot_ns, bpf_func_id_BPF_FUNC_ktime_get_coarse_ns,
    bpf_func_id_BPF_FUNC_ktime_get_ns, bpf_func_id_BPF_FUNC_l3_csum_replace,
    bpf_func_id_BPF_FUNC_l4_csum_replace, bpf_func_id_BPF_FUNC_map_delete_elem,
    bpf_func_id_BPF_FUNC_map_lookup_elem, bpf_func_id_BPF_FUNC_map_update_elem,
    bpf_func_id_BPF_FUNC_probe_read_str, bpf_func_id_BPF_FUNC_probe_read_user,
    bpf_func_id_BPF_FUNC_probe_read_user_str, bpf_func_id_BPF_FUNC_redirect,
    bpf_func_id_BPF_FUNC_ringbuf_discard, bpf_func_id_BPF_FUNC_ringbuf_reserve,
    bpf_func_id_BPF_FUNC_ringbuf_submit, bpf_func_id_BPF_FUNC_set_retval,
    bpf_func_id_BPF_FUNC_sk_fullsock, bpf_func_id_BPF_FUNC_sk_lookup_tcp,
    bpf_func_id_BPF_FUNC_sk_lookup_udp, bpf_func_id_BPF_FUNC_sk_release,
    bpf_func_id_BPF_FUNC_sk_storage_get, bpf_func_id_BPF_FUNC_skb_adjust_room,
    bpf_func_id_BPF_FUNC_skb_change_head, bpf_func_id_BPF_FUNC_skb_change_proto,
    bpf_func_id_BPF_FUNC_skb_load_bytes, bpf_func_id_BPF_FUNC_skb_load_bytes_relative,
    bpf_func_id_BPF_FUNC_skb_pull_data, bpf_func_id_BPF_FUNC_skb_store_bytes,
    bpf_func_id_BPF_FUNC_trace_printk, bpf_map_type_BPF_MAP_TYPE_RINGBUF,
    bpf_map_type_BPF_MAP_TYPE_SK_STORAGE, bpf_prog_type_BPF_PROG_TYPE_CGROUP_DEVICE,
    bpf_prog_type_BPF_PROG_TYPE_CGROUP_SKB, bpf_prog_type_BPF_PROG_TYPE_CGROUP_SOCK,
    bpf_prog_type_BPF_PROG_TYPE_CGROUP_SOCK_ADDR, bpf_prog_type_BPF_PROG_TYPE_CGROUP_SOCKOPT,
    bpf_prog_type_BPF_PROG_TYPE_CGROUP_SYSCTL, bpf_prog_type_BPF_PROG_TYPE_EXT,
    bpf_prog_type_BPF_PROG_TYPE_FLOW_DISSECTOR, bpf_prog_type_BPF_PROG_TYPE_KPROBE,
    bpf_prog_type_BPF_PROG_TYPE_LIRC_MODE2, bpf_prog_type_BPF_PROG_TYPE_LSM,
    bpf_prog_type_BPF_PROG_TYPE_LWT_IN, bpf_prog_type_BPF_PROG_TYPE_LWT_OUT,
    bpf_prog_type_BPF_PROG_TYPE_LWT_SEG6LOCAL, bpf_prog_type_BPF_PROG_TYPE_LWT_XMIT,
    bpf_prog_type_BPF_PROG_TYPE_NETFILTER, bpf_prog_type_BPF_PROG_TYPE_PERF_EVENT,
    bpf_prog_type_BPF_PROG_TYPE_RAW_TRACEPOINT,
    bpf_prog_type_BPF_PROG_TYPE_RAW_TRACEPOINT_WRITABLE, bpf_prog_type_BPF_PROG_TYPE_SCHED_ACT,
    bpf_prog_type_BPF_PROG_TYPE_SCHED_CLS, bpf_prog_type_BPF_PROG_TYPE_SK_LOOKUP,
    bpf_prog_type_BPF_PROG_TYPE_SK_MSG, bpf_prog_type_BPF_PROG_TYPE_SK_REUSEPORT,
    bpf_prog_type_BPF_PROG_TYPE_SK_SKB, bpf_prog_type_BPF_PROG_TYPE_SOCK_OPS,
    bpf_prog_type_BPF_PROG_TYPE_SOCKET_FILTER, bpf_prog_type_BPF_PROG_TYPE_STRUCT_OPS,
    bpf_prog_type_BPF_PROG_TYPE_SYSCALL, bpf_prog_type_BPF_PROG_TYPE_TRACEPOINT,
    bpf_prog_type_BPF_PROG_TYPE_TRACING, bpf_prog_type_BPF_PROG_TYPE_UNSPEC,
    bpf_prog_type_BPF_PROG_TYPE_XDP, bpf_sock, bpf_sock_addr, bpf_sockopt, bpf_user_pt_regs_t,
    fuse_bpf_arg, fuse_bpf_args, fuse_entry_bpf_out, fuse_entry_out, seccomp_data, xdp_md,
};
use std::collections::HashMap;
use std::mem::{offset_of, size_of};
use std::sync::{Arc, LazyLock};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

pub const BPF_PROG_TYPE_FUSE: u32 = 0x77777777;

#[derive(Clone, Default, Debug)]
pub struct BpfTypeFilter(Vec<ProgramType>);

impl<T: IntoIterator<Item = ProgramType>> From<T> for BpfTypeFilter {
    fn from(types: T) -> Self {
        Self(types.into_iter().collect())
    }
}

impl BpfTypeFilter {
    pub fn accept(&self, program_type: ProgramType) -> bool {
        self.0.is_empty() || self.0.iter().find(|v| **v == program_type).is_some()
    }
}

// Map types that should not be allowed in `map_lookup_elem`, `map_update_elem`
// and `map_delete_elem`.
static SPECIAL_MAP_TYPES: &[linux_uapi::bpf_map_type] =
    &[bpf_map_type_BPF_MAP_TYPE_SK_STORAGE, bpf_map_type_BPF_MAP_TYPE_RINGBUF];

static BPF_HELPERS_DEFINITIONS: LazyLock<Vec<(BpfTypeFilter, HelperDefinition)>> =
    LazyLock::new(|| {
        vec![
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_map_lookup_elem,
                    name: "map_lookup_elem",
                    signature: FunctionSignature {
                        args: vec![
                            Type::ConstPtrToMapParameter {
                                filter: MapTypeFilter::DenyList(SPECIAL_MAP_TYPES),
                            },
                            Type::MapKeyParameter { map_ptr_index: 0 },
                        ],
                        return_value: Type::NullOrParameter(Box::new(Type::MapValueParameter {
                            map_ptr_index: 0,
                        })),
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_map_update_elem,
                    name: "map_update_elem",
                    signature: FunctionSignature {
                        args: vec![
                            Type::ConstPtrToMapParameter {
                                filter: MapTypeFilter::DenyList(SPECIAL_MAP_TYPES),
                            },
                            Type::MapKeyParameter { map_ptr_index: 0 },
                            Type::MapValueParameter { map_ptr_index: 0 },
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_map_delete_elem,
                    name: "map_delete_elem",
                    signature: FunctionSignature {
                        args: vec![
                            Type::ConstPtrToMapParameter {
                                filter: MapTypeFilter::DenyList(SPECIAL_MAP_TYPES),
                            },
                            Type::MapKeyParameter { map_ptr_index: 0 },
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_trace_printk,
                    name: "trace_printk",
                    signature: FunctionSignature {
                        // TODO("https://fxbug.dev/287120494"): Specify arguments
                        args: vec![],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_ktime_get_ns,
                    name: "ktime_get_ns",
                    signature: FunctionSignature {
                        args: vec![],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_probe_read_user,
                    name: "probe_read_user",
                    signature: FunctionSignature {
                        args: vec![
                            Type::MemoryParameter {
                                size: MemoryParameterSize::Reference { index: 1 },
                                input: false,
                                output: true,
                            },
                            Type::ScalarValueParameter,
                            Type::AnyParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_probe_read_user_str,
                    name: "probe_read_user_str",
                    signature: FunctionSignature {
                        args: vec![
                            Type::MemoryParameter {
                                size: MemoryParameterSize::Reference { index: 1 },
                                input: false,
                                output: true,
                            },
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![
                    ProgramType::CgroupSkb,
                    ProgramType::SchedAct,
                    ProgramType::SchedCls,
                    ProgramType::SocketFilter,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_get_socket_uid,
                    name: "get_socket_uid",
                    signature: FunctionSignature {
                        args: vec![Type::StructParameter { id: SK_BUF_ID.clone() }],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![
                    ProgramType::CgroupSock,
                    ProgramType::CgroupSockAddr,
                    ProgramType::CgroupSockopt,
                    ProgramType::Fuse,
                    ProgramType::Kprobe,
                    ProgramType::Tracepoint,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_get_current_uid_gid,
                    name: "get_current_uid_gid",
                    signature: FunctionSignature {
                        args: vec![],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![
                    ProgramType::CgroupSock,
                    ProgramType::CgroupSockAddr,
                    ProgramType::CgroupSockopt,
                    ProgramType::Fuse,
                    ProgramType::Kprobe,
                    ProgramType::Tracepoint,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_get_current_pid_tgid,
                    name: "get_current_pid_tgid",
                    signature: FunctionSignature {
                        args: vec![],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![ProgramType::SchedAct, ProgramType::SchedCls].into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_skb_pull_data,
                    name: "skb_pull_data",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: true,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_ringbuf_reserve,
                    name: "ringbuf_reserve",
                    signature: FunctionSignature {
                        args: vec![
                            Type::ConstPtrToMapParameter {
                                filter: MapTypeFilter::AllowList(&[
                                    bpf_map_type_BPF_MAP_TYPE_RINGBUF,
                                ]),
                            },
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::NullOrParameter(Box::new(Type::ReleasableParameter {
                            id: RING_BUFFER_RESERVATION.clone(),
                            inner: Box::new(Type::MemoryParameter {
                                size: MemoryParameterSize::Reference { index: 1 },
                                input: false,
                                output: false,
                            }),
                        })),
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_ringbuf_submit,
                    name: "ringbuf_submit",
                    signature: FunctionSignature {
                        args: vec![
                            Type::ReleaseParameter { id: RING_BUFFER_RESERVATION.clone() },
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::default(),
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_ringbuf_discard,
                    name: "ringbuf_discard",
                    signature: FunctionSignature {
                        args: vec![
                            Type::ReleaseParameter { id: RING_BUFFER_RESERVATION.clone() },
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::default(),
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![ProgramType::SchedAct, ProgramType::SchedCls].into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_skb_change_proto,
                    name: "skb_change_proto",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: true,
                    },
                },
            ),
            (
                vec![ProgramType::SchedAct, ProgramType::SchedCls].into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_csum_update,
                    name: "csum_update",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![ProgramType::Kprobe, ProgramType::Tracepoint].into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_probe_read_str,
                    name: "probe_read_str",
                    signature: FunctionSignature {
                        // TODO(347257215): Implement verifier feature
                        args: vec![],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![
                    ProgramType::CgroupSkb,
                    ProgramType::SchedAct,
                    ProgramType::SchedCls,
                    ProgramType::SocketFilter,
                    ProgramType::CgroupSock,
                    ProgramType::CgroupSockAddr,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_get_socket_cookie,
                    name: "get_socket_cookie",
                    signature: FunctionSignature {
                        args: vec![Type::ContextParameter { parameter_index: 0 }],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![ProgramType::SchedAct, ProgramType::SchedCls].into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_redirect,
                    name: "redirect",
                    signature: FunctionSignature {
                        args: vec![Type::ScalarValueParameter, Type::ScalarValueParameter],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![ProgramType::SchedAct, ProgramType::SchedCls].into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_skb_adjust_room,
                    name: "skb_adjust_room",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: true,
                    },
                },
            ),
            (
                vec![ProgramType::SchedAct, ProgramType::SchedCls].into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_l3_csum_replace,
                    name: "l3_csum_replace",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: true,
                    },
                },
            ),
            (
                vec![ProgramType::SchedAct, ProgramType::SchedCls].into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_l4_csum_replace,
                    name: "l4_csum_replace",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: true,
                    },
                },
            ),
            (
                vec![ProgramType::SchedAct, ProgramType::SchedCls].into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_skb_store_bytes,
                    name: "skb_store_bytes",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::ScalarValueParameter,
                            Type::MemoryParameter {
                                size: MemoryParameterSize::Reference { index: 3 },
                                input: true,
                                output: false,
                            },
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: true,
                    },
                },
            ),
            (
                vec![ProgramType::SchedAct, ProgramType::SchedCls].into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_skb_change_head,
                    name: "skb_change_head",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: true,
                    },
                },
            ),
            (
                vec![
                    ProgramType::CgroupSkb,
                    ProgramType::SchedAct,
                    ProgramType::SchedCls,
                    ProgramType::SocketFilter,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_skb_load_bytes,
                    name: "skb_load_bytes",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::ScalarValueParameter,
                            Type::MemoryParameter {
                                size: MemoryParameterSize::Reference { index: 3 },
                                input: false,
                                output: true,
                            },
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![
                    ProgramType::CgroupSkb,
                    ProgramType::SchedAct,
                    ProgramType::SchedCls,
                    ProgramType::SocketFilter,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_skb_load_bytes_relative,
                    name: "skb_load_bytes_relative",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::ScalarValueParameter,
                            Type::MemoryParameter {
                                size: MemoryParameterSize::Reference { index: 3 },
                                input: false,
                                output: true,
                            },
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_ktime_get_boot_ns,
                    name: "ktime_get_boot_ns",
                    signature: FunctionSignature {
                        args: vec![],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_ktime_get_coarse_ns,
                    name: "ktime_get_coarse_ns",
                    signature: FunctionSignature {
                        args: vec![],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![
                    ProgramType::CgroupSkb,
                    ProgramType::CgroupSock,
                    ProgramType::CgroupSockopt,
                    ProgramType::CgroupSockAddr,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_sk_storage_get,
                    name: "sk_storage_get",
                    signature: FunctionSignature {
                        args: vec![
                            Type::ConstPtrToMapParameter {
                                filter: MapTypeFilter::AllowList(&[
                                    bpf_map_type_BPF_MAP_TYPE_SK_STORAGE,
                                ]),
                            },
                            Type::StructParameter { id: BPF_SOCK_ID.clone() },
                            Type::NullOrParameter(Box::new(Type::MapValueParameter {
                                map_ptr_index: 0,
                            })),
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::NullOrParameter(Box::new(Type::MapValueParameter {
                            map_ptr_index: 0,
                        })),
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                BpfTypeFilter::default(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_get_smp_processor_id,
                    name: "get_smp_processor_id",
                    signature: FunctionSignature {
                        args: vec![],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![ProgramType::CgroupSkb, ProgramType::SchedAct, ProgramType::SchedCls].into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_sk_fullsock,
                    name: "sk_fullsock",
                    signature: FunctionSignature {
                        args: vec![Type::StructParameter { id: BPF_SOCK_ID.clone() }],
                        return_value: Type::NullOrParameter(Box::new(ptr_to_mem_type::<bpf_sock>(
                            BPF_SOCK_ID.clone(),
                        ))),
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![
                    ProgramType::CgroupDevice,
                    ProgramType::CgroupSockopt,
                    ProgramType::CgroupSysctl,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_set_retval,
                    name: "set_retval",
                    signature: FunctionSignature {
                        args: vec![Type::ScalarValueParameter],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![
                    ProgramType::CgroupDevice,
                    ProgramType::CgroupSockopt,
                    ProgramType::CgroupSysctl,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_get_retval,
                    name: "get_retval",
                    signature: FunctionSignature {
                        args: vec![],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                // Note that bpf_sk_lookup_tcp should also be available to CgroupAddr and Xdp
                // programs, but those use different type for the first context parameter.
                vec![
                    ProgramType::CgroupSkb,
                    ProgramType::SchedAct,
                    ProgramType::SchedCls,
                    ProgramType::SkSkb,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_sk_lookup_tcp,
                    name: "sk_lookup_tcp",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::MemoryParameter {
                                size: MemoryParameterSize::Reference { index: 2 },
                                input: true,
                                output: false,
                            },
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::NullOrParameter(Box::new(Type::ReleasableParameter {
                            id: RELEASABLE_BPF_SOCK_PTR.clone(),
                            inner: Box::new(BPF_SOCK_TYPE.clone()),
                        })),
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                // Note that bpf_sk_lookup_udp should also be available to CgroupAddr and Xdp
                // programs, but those use different type for the first context parameter.
                vec![
                    ProgramType::CgroupSkb,
                    ProgramType::SchedAct,
                    ProgramType::SchedCls,
                    ProgramType::SkSkb,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_sk_lookup_udp,
                    name: "sk_lookup_udp",
                    signature: FunctionSignature {
                        args: vec![
                            Type::StructParameter { id: SK_BUF_ID.clone() },
                            Type::MemoryParameter {
                                size: MemoryParameterSize::Reference { index: 2 },
                                input: true,
                                output: false,
                            },
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                            Type::ScalarValueParameter,
                        ],
                        return_value: Type::NullOrParameter(Box::new(Type::ReleasableParameter {
                            id: RELEASABLE_BPF_SOCK_PTR.clone(),
                            inner: Box::new(BPF_SOCK_TYPE.clone()),
                        })),
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![
                    ProgramType::CgroupSkb,
                    ProgramType::CgroupSockAddr,
                    ProgramType::SchedAct,
                    ProgramType::SchedCls,
                    ProgramType::SkSkb,
                    ProgramType::Xdp,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_sk_release,
                    name: "sk_release",
                    signature: FunctionSignature {
                        args: vec![Type::ReleaseParameter { id: RELEASABLE_BPF_SOCK_PTR.clone() }],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
            (
                vec![
                    ProgramType::CgroupSkb,
                    ProgramType::CgroupSock,
                    ProgramType::CgroupSockopt,
                    ProgramType::CgroupSockAddr,
                    ProgramType::SchedAct,
                    ProgramType::SchedCls,
                    ProgramType::SkMsg,
                    ProgramType::SocketFilter,
                    ProgramType::SockOps,
                ]
                .into(),
                HelperDefinition {
                    index: bpf_func_id_BPF_FUNC_get_netns_cookie,
                    name: "bpf_get_netns_cookie",
                    signature: FunctionSignature {
                        args: vec![Type::NullOrParameter(Box::new(Type::ContextParameter {
                            parameter_index: 0,
                        }))],
                        return_value: Type::UNKNOWN_SCALAR,
                        invalidate_array_bounds: false,
                    },
                },
            ),
        ]
    });

/// A type of a struct that may be passed to eBPF programs.
#[derive(Copy, Clone)]
pub enum StructId {
    SkBuff = 1,
    XdpMd = 2,
    BpfUserPtRegs = 3,
    BpfSock = 4,
    BpfSockOpt = 5,
    BpfSockAddr = 6,
    BpfFuse = 7,
}

impl StructId {
    pub const fn as_memory_id(self) -> MemoryId {
        MemoryId::from_raw(self as u64)
    }
}

impl From<StructId> for febpf::StructId {
    fn from(value: StructId) -> Self {
        match value {
            StructId::SkBuff => febpf::StructId::SkBuff,
            StructId::XdpMd => febpf::StructId::XdpMd,
            StructId::BpfUserPtRegs => febpf::StructId::BpfUserPtRegs,
            StructId::BpfSock => febpf::StructId::BpfSock,
            StructId::BpfSockOpt => febpf::StructId::BpfSockOpt,
            StructId::BpfSockAddr => febpf::StructId::BpfSockAddr,
            StructId::BpfFuse => febpf::StructId::BpfFuse,
        }
    }
}

impl TryFrom<&MemoryId> for StructId {
    type Error = ();

    fn try_from(value: &MemoryId) -> Result<Self, Self::Error> {
        if *value == StructId::SkBuff.as_memory_id() {
            Ok(StructId::SkBuff)
        } else if *value == StructId::XdpMd.as_memory_id() {
            Ok(StructId::XdpMd)
        } else if *value == StructId::BpfUserPtRegs.as_memory_id() {
            Ok(StructId::BpfUserPtRegs)
        } else if *value == StructId::BpfSock.as_memory_id() {
            Ok(StructId::BpfSock)
        } else if *value == StructId::BpfSockOpt.as_memory_id() {
            Ok(StructId::BpfSockOpt)
        } else if *value == StructId::BpfSockAddr.as_memory_id() {
            Ok(StructId::BpfSockAddr)
        } else if *value == StructId::BpfFuse.as_memory_id() {
            Ok(StructId::BpfFuse)
        } else {
            Err(())
        }
    }
}

impl From<febpf::StructId> for StructId {
    fn from(value: febpf::StructId) -> Self {
        match value {
            febpf::StructId::SkBuff => StructId::SkBuff,
            febpf::StructId::XdpMd => StructId::XdpMd,
            febpf::StructId::BpfUserPtRegs => StructId::BpfUserPtRegs,
            febpf::StructId::BpfSock => StructId::BpfSock,
            febpf::StructId::BpfSockOpt => StructId::BpfSockOpt,
            febpf::StructId::BpfSockAddr => StructId::BpfSockAddr,
            febpf::StructId::BpfFuse => StructId::BpfFuse,
            febpf::StructId::__SourceBreaking { unknown_ordinal } => {
                panic!("Invalid struct id: {}", unknown_ordinal)
            }
        }
    }
}

fn scalar_field(offset: usize, size: usize) -> FieldDescriptor {
    FieldDescriptor { offset, field_type: FieldType::Scalar { size } }
}

fn scalar_range(offset: usize, end_offset: usize) -> FieldDescriptor {
    FieldDescriptor { offset, field_type: FieldType::Scalar { size: end_offset - offset } }
}

fn scalar_mut_range(offset: usize, end_offset: usize) -> FieldDescriptor {
    FieldDescriptor { offset, field_type: FieldType::MutableScalar { size: end_offset - offset } }
}

fn scalar_u32_field(offset: usize) -> FieldDescriptor {
    FieldDescriptor { offset, field_type: FieldType::Scalar { size: std::mem::size_of::<u32>() } }
}

fn scalar_u32_mut_field(offset: usize) -> FieldDescriptor {
    FieldDescriptor {
        offset,
        field_type: FieldType::MutableScalar { size: std::mem::size_of::<u32>() },
    }
}

fn scalar_u64_field(offset: usize) -> FieldDescriptor {
    FieldDescriptor { offset, field_type: FieldType::Scalar { size: std::mem::size_of::<u64>() } }
}

fn array_start_field(offset: usize, id: MemoryId) -> FieldDescriptor {
    FieldDescriptor { offset, field_type: FieldType::PtrToArray { id, is_32_bit: false } }
}

fn array_end_field(offset: usize, id: MemoryId) -> FieldDescriptor {
    FieldDescriptor { offset, field_type: FieldType::PtrToEndArray { id, is_32_bit: false } }
}

fn array_start_32_field(offset: usize, id: MemoryId) -> FieldDescriptor {
    FieldDescriptor { offset, field_type: FieldType::PtrToArray { id, is_32_bit: true } }
}

fn array_end_32_field(offset: usize, id: MemoryId) -> FieldDescriptor {
    FieldDescriptor { offset, field_type: FieldType::PtrToEndArray { id, is_32_bit: true } }
}

fn ptr_to_mem_field<T: IntoBytes>(offset: usize, id: MemoryId) -> FieldDescriptor {
    FieldDescriptor {
        offset,
        field_type: FieldType::PtrToMemory {
            id,
            buffer_size: std::mem::size_of::<T>(),
            is_32_bit: false,
        },
    }
}

fn nullable_ptr_to_mem_field<T: IntoBytes>(offset: usize, id: MemoryId) -> FieldDescriptor {
    FieldDescriptor {
        offset,
        field_type: FieldType::NullablePtrToMemory {
            is_32_bit: false,
            id,
            buffer_size: std::mem::size_of::<T>(),
        },
    }
}

fn ptr_to_struct_type(id: MemoryId, fields: Vec<FieldDescriptor>) -> Type {
    Type::PtrToStruct { id, offset: 0.into(), descriptor: Arc::new(StructDescriptor { fields }) }
}

fn ptr_to_mem_type<T: IntoBytes>(id: MemoryId) -> Type {
    Type::PtrToMemory { id, offset: 0.into(), buffer_size: std::mem::size_of::<T>() as u64 }
}

static RING_BUFFER_RESERVATION: LazyLock<MemoryId> = LazyLock::new(MemoryId::new);

// Id for the value returned from `bpf_sk_lookup_xxx` helpers. It has to be
// released by calling `bpf_sk_release`.
static RELEASABLE_BPF_SOCK_PTR: LazyLock<MemoryId> = LazyLock::new(MemoryId::new);

pub static SK_BUF_ID: MemoryId = StructId::SkBuff.as_memory_id();

/// Type for the `__sk_buff` passed to `BPF_PROG_TYPE_SOCKET_FILTER` programs.
pub static SOCKET_FILTER_SK_BUF_TYPE: LazyLock<Type> = LazyLock::new(|| {
    ptr_to_struct_type(
        SK_BUF_ID.clone(),
        vec![
            // All fields from the start of `__sk_buff` to `cb` are read-only scalars.
            scalar_range(0, offset_of!(__sk_buff, cb)),
            // `cb` is a mutable array.
            scalar_mut_range(offset_of!(__sk_buff, cb), offset_of!(__sk_buff, hash)),
            scalar_u32_field(offset_of!(__sk_buff, hash)),
            scalar_u32_field(offset_of!(__sk_buff, napi_id)),
            scalar_u32_field(offset_of!(__sk_buff, tstamp)),
            scalar_u32_field(offset_of!(__sk_buff, gso_segs)),
            scalar_u32_field(offset_of!(__sk_buff, gso_size)),
        ],
    )
});
pub static SOCKET_FILTER_ARGS: LazyLock<Vec<Type>> =
    LazyLock::new(|| vec![SOCKET_FILTER_SK_BUF_TYPE.clone()]);

/// Type for the `__sk_buff` passed to `BPF_PROG_TYPE_SCHED_CLS` and
/// `BPF_PROG_TYPE_SCHED_ACT` programs.
pub static SCHED_ARG_TYPE: LazyLock<Type> = LazyLock::new(|| {
    let data_id = MemoryId::new();
    ptr_to_struct_type(
        SK_BUF_ID.clone(),
        vec![
            // All fields from the start of `__sk_buff` to `cb` are read-only scalars, except mark
            // that is read-write.
            scalar_range(0, offset_of!(__sk_buff, mark)),
            scalar_u32_mut_field(offset_of!(__sk_buff, mark)),
            scalar_range(offset_of!(__sk_buff, queue_mapping), offset_of!(__sk_buff, cb)),
            // `cb` is a mutable array.
            scalar_mut_range(offset_of!(__sk_buff, cb), offset_of!(__sk_buff, hash)),
            scalar_u32_field(offset_of!(__sk_buff, hash)),
            scalar_u32_field(offset_of!(__sk_buff, tc_classid)),
            array_start_32_field(offset_of!(__sk_buff, data), data_id.clone()),
            array_end_32_field(offset_of!(__sk_buff, data_end), data_id),
            scalar_u32_field(offset_of!(__sk_buff, napi_id)),
            scalar_u32_field(offset_of!(__sk_buff, data_meta)),
            scalar_range(offset_of!(__sk_buff, tstamp), size_of::<__sk_buff>()),
        ],
    )
});
pub static SCHED_ARGS: LazyLock<Vec<Type>> = LazyLock::new(|| vec![SCHED_ARG_TYPE.clone()]);

/// Type for the `__sk_buff` passed to `BPF_PROG_TYPE_CGROUP_SKB` programs.
pub static CGROUP_SKB_SK_BUF_TYPE: LazyLock<Type> = LazyLock::new(|| {
    let data_id = MemoryId::new();
    assert!(offset_of!(__sk_buff, __bindgen_anon_2) == 168);
    ptr_to_struct_type(
        SK_BUF_ID.clone(),
        vec![
            // All fields from the start of `__sk_buff` to `cb` are read-only scalars, except mark
            // that is read-write.
            scalar_range(0, offset_of!(__sk_buff, mark)),
            scalar_u32_mut_field(offset_of!(__sk_buff, mark)),
            scalar_range(offset_of!(__sk_buff, queue_mapping), offset_of!(__sk_buff, cb)),
            // `cb` is a mutable array.
            scalar_mut_range(offset_of!(__sk_buff, cb), offset_of!(__sk_buff, hash)),
            scalar_u32_field(offset_of!(__sk_buff, hash)),
            array_start_32_field(offset_of!(__sk_buff, data), data_id.clone()),
            array_end_32_field(offset_of!(__sk_buff, data_end), data_id),
            scalar_range(offset_of!(__sk_buff, napi_id), offset_of!(__sk_buff, data_meta)),
            scalar_u64_field(offset_of!(__sk_buff, tstamp)),
            scalar_u32_field(offset_of!(__sk_buff, gso_segs)),
            // `sk` field
            nullable_ptr_to_mem_field::<bpf_sock>(
                offset_of!(__sk_buff, __bindgen_anon_2),
                BPF_SOCK_ID.clone(),
            ),
            scalar_u32_field(offset_of!(__sk_buff, gso_size)),
            scalar_u64_field(offset_of!(__sk_buff, hwtstamp)),
        ],
    )
});
pub static CGROUP_SKB_ARGS: LazyLock<Vec<Type>> =
    LazyLock::new(|| vec![CGROUP_SKB_SK_BUF_TYPE.clone()]);

static XDP_MD_ID: MemoryId = StructId::XdpMd.as_memory_id();
static XDP_MD_TYPE: LazyLock<Type> = LazyLock::new(|| {
    let data_id = MemoryId::new();

    ptr_to_struct_type(
        XDP_MD_ID.clone(),
        vec![
            array_start_32_field(offset_of!(xdp_md, data), data_id.clone()),
            array_end_32_field(offset_of!(xdp_md, data_end), data_id),
            // All fields starting from `data_meta` are readable.
            {
                let data_meta_offset = offset_of!(xdp_md, data_meta);
                scalar_field(data_meta_offset, std::mem::size_of::<xdp_md>() - data_meta_offset)
            },
        ],
    )
});
static XDP_MD_ARGS: LazyLock<Vec<Type>> = LazyLock::new(|| vec![XDP_MD_TYPE.clone()]);

pub static BPF_USER_PT_REGS_T_ID: MemoryId = StructId::BpfUserPtRegs.as_memory_id();
pub static BPF_USER_PT_REGS_T_ARGS: LazyLock<Vec<Type>> =
    LazyLock::new(|| vec![ptr_to_mem_type::<bpf_user_pt_regs_t>(BPF_USER_PT_REGS_T_ID.clone())]);

pub static BPF_SOCK_ID: MemoryId = StructId::BpfSock.as_memory_id();
pub static BPF_SOCK_TYPE: LazyLock<Type> =
    LazyLock::new(|| ptr_to_mem_type::<bpf_sock>(BPF_SOCK_ID.clone()));
pub static BPF_SOCK_ARGS: LazyLock<Vec<Type>> = LazyLock::new(|| vec![BPF_SOCK_TYPE.clone()]);

pub static BPF_SOCKOPT_ID: MemoryId = StructId::BpfSockOpt.as_memory_id();
pub static BPF_SOCKOPT_TYPE: LazyLock<Type> = LazyLock::new(|| {
    let optval_id = MemoryId::new();
    ptr_to_struct_type(
        BPF_SOCKOPT_ID.clone(),
        vec![
            // sk
            ptr_to_mem_field::<bpf_sock>(
                offset_of!(bpf_sockopt, __bindgen_anon_1),
                BPF_SOCK_ID.clone(),
            ),
            // optval
            array_start_field(offset_of!(bpf_sockopt, __bindgen_anon_2), optval_id.clone()),
            // optval_end
            array_end_field(offset_of!(bpf_sockopt, __bindgen_anon_3), optval_id),
            scalar_u32_field(offset_of!(bpf_sockopt, level)),
            scalar_u32_mut_field(offset_of!(bpf_sockopt, optname)),
            scalar_u32_mut_field(offset_of!(bpf_sockopt, optlen)),
            scalar_u32_mut_field(offset_of!(bpf_sockopt, retval)),
        ],
    )
});
pub static BPF_SOCKOPT_ARGS: LazyLock<Vec<Type>> = LazyLock::new(|| vec![BPF_SOCKOPT_TYPE.clone()]);

// Verifier allows access only to some fields of the `bfp_sock_addr` struct
// depending on the `expected_attach_type` passed when the program is loaded.
// The INET4 and INET6 versions defined below should be used by the verifier
// for INET4 and INET6 attachments. `BPF_SOCK_ADDR_TYPE` contains all fields
// from that struct. It's used by the `ProgramArgument` implementation for
// the struct. This allows to share the `EbpfProgramContext` for all programs
// that take `bpf_sock_addr`. Linkers considers `BPF_SOCK_ADDR_TYPE` as is
// a subtype of both INET4 and INET6 versions.
pub static BPF_SOCK_ADDR_ID: MemoryId = StructId::BpfSockAddr.as_memory_id();
pub static BPF_SOCK_ADDR_TYPE: LazyLock<Type> = LazyLock::new(|| {
    ptr_to_struct_type(
        BPF_SOCK_ADDR_ID.clone(),
        vec![
            scalar_u32_field(offset_of!(bpf_sock_addr, user_family)),
            scalar_u32_field(offset_of!(bpf_sock_addr, user_ip4)),
            scalar_field(offset_of!(bpf_sock_addr, user_ip6), 16),
            scalar_u32_field(offset_of!(bpf_sock_addr, user_port)),
            scalar_u32_field(offset_of!(bpf_sock_addr, family)),
            scalar_u32_field(offset_of!(bpf_sock_addr, type_)),
            scalar_u32_field(offset_of!(bpf_sock_addr, protocol)),
            scalar_u32_field(offset_of!(bpf_sock_addr, msg_src_ip4)),
            scalar_field(offset_of!(bpf_sock_addr, msg_src_ip6), 16),
            // sk
            ptr_to_mem_field::<bpf_sock>(
                offset_of!(bpf_sock_addr, __bindgen_anon_1),
                BPF_SOCK_ID.clone(),
            ),
        ],
    )
});

pub static BPF_SOCK_ADDR_INET4_TYPE: LazyLock<Type> = LazyLock::new(|| {
    ptr_to_struct_type(
        BPF_SOCK_ADDR_ID.clone(),
        vec![
            scalar_u32_field(offset_of!(bpf_sock_addr, user_family)),
            scalar_u32_field(offset_of!(bpf_sock_addr, user_ip4)),
            scalar_u32_field(offset_of!(bpf_sock_addr, user_port)),
            scalar_u32_field(offset_of!(bpf_sock_addr, family)),
            scalar_u32_field(offset_of!(bpf_sock_addr, type_)),
            scalar_u32_field(offset_of!(bpf_sock_addr, protocol)),
            // sk
            ptr_to_mem_field::<bpf_sock>(
                offset_of!(bpf_sock_addr, __bindgen_anon_1),
                BPF_SOCK_ID.clone(),
            ),
        ],
    )
});
pub static BPF_SOCK_ADDR_INET4_ARGS: LazyLock<Vec<Type>> =
    LazyLock::new(|| vec![BPF_SOCK_ADDR_INET4_TYPE.clone()]);

pub static BPF_SOCK_ADDR_INET6_TYPE: LazyLock<Type> = LazyLock::new(|| {
    ptr_to_struct_type(
        BPF_SOCK_ADDR_ID.clone(),
        vec![
            scalar_u32_field(offset_of!(bpf_sock_addr, user_family)),
            scalar_field(offset_of!(bpf_sock_addr, user_ip6), 16),
            scalar_u32_field(offset_of!(bpf_sock_addr, user_port)),
            scalar_u32_field(offset_of!(bpf_sock_addr, family)),
            scalar_u32_field(offset_of!(bpf_sock_addr, type_)),
            scalar_u32_field(offset_of!(bpf_sock_addr, protocol)),
            // sk
            ptr_to_mem_field::<bpf_sock>(
                offset_of!(bpf_sock_addr, __bindgen_anon_1),
                BPF_SOCK_ID.clone(),
            ),
        ],
    )
});
pub static BPF_SOCK_ADDR_INET6_ARGS: LazyLock<Vec<Type>> =
    LazyLock::new(|| vec![BPF_SOCK_ADDR_INET6_TYPE.clone()]);

static BPF_FUSE_ID: MemoryId = StructId::BpfFuse.as_memory_id();
static BPF_FUSE_TYPE: LazyLock<Type> = LazyLock::new(|| {
    ptr_to_struct_type(
        BPF_FUSE_ID.clone(),
        vec![
            scalar_field(0, offset_of!(fuse_bpf_args, out_args)),
            ptr_to_mem_field::<fuse_entry_out>(
                offset_of!(fuse_bpf_args, out_args) + offset_of!(fuse_bpf_arg, value),
                MemoryId::new(),
            ),
            ptr_to_mem_field::<fuse_entry_bpf_out>(
                offset_of!(fuse_bpf_args, out_args)
                    + std::mem::size_of::<fuse_bpf_arg>()
                    + offset_of!(fuse_bpf_arg, value),
                MemoryId::new(),
            ),
        ],
    )
});
static BPF_FUSE_ARGS: LazyLock<Vec<Type>> = LazyLock::new(|| vec![BPF_FUSE_TYPE.clone()]);

#[repr(C)]
#[derive(Copy, Clone, IntoBytes, Immutable, KnownLayout, FromBytes)]
struct TraceEntry {
    type_: u16,
    flags: u8,
    preemp_count: u8,
    pid: u32,
}

#[repr(C)]
#[derive(Copy, Clone, IntoBytes, Immutable, KnownLayout, FromBytes)]
struct TraceEvent {
    trace_entry: TraceEntry,
    id: u64,
    // This is defined a being big enough for all expected tracepoint. It is not clear how the
    // verifier can know which tracepoint is targeted when the program is loaded. Instead, this
    // array will be big enough, and will be filled with 0 when running a given program.
    args: [u64; 16],
}

#[repr(C)]
#[derive(Copy, Clone, IntoBytes, Immutable, KnownLayout, FromBytes)]
struct RawTraceEvent {
    // This is defined a being big enough for all expected tracepoint. It is not clear how the
    // verifier can know which tracepoint is targeted when the program is loaded. Instead, this
    // array will be big enough, and will be filled with 0 when running a given program.
    args: [u64; 2],
}

static BPF_TRACEPOINT_ID: LazyLock<MemoryId> = LazyLock::new(MemoryId::new);
static BPF_TRACEPOINT_ARGS: LazyLock<Vec<Type>> =
    LazyLock::new(|| vec![ptr_to_mem_type::<TraceEvent>(BPF_TRACEPOINT_ID.clone())]);

static BPF_RAW_TRACEPOINT_ID: LazyLock<MemoryId> = LazyLock::new(MemoryId::new);
static BPF_RAW_TRACEPOINT_ARGS: LazyLock<Vec<Type>> =
    LazyLock::new(|| vec![ptr_to_mem_type::<RawTraceEvent>(BPF_RAW_TRACEPOINT_ID.clone())]);

/// The different type of BPF programs.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProgramType {
    CgroupDevice,
    CgroupSkb,
    CgroupSock,
    CgroupSockAddr,
    CgroupSockopt,
    CgroupSysctl,
    Ext,
    FlowDissector,
    Kprobe,
    LircMode2,
    Lsm,
    LwtIn,
    LwtOut,
    LwtSeg6Local,
    LwtXmit,
    Netfilter,
    PerfEvent,
    RawTracepoint,
    RawTracepointWritable,
    SchedAct,
    SchedCls,
    SkLookup,
    SkMsg,
    SkReuseport,
    SkSkb,
    SocketFilter,
    SockOps,
    StructOps,
    Syscall,
    Tracepoint,
    Tracing,
    Unspec,
    Xdp,
    /// Custom id for Fuse
    Fuse,
}

#[derive(thiserror::Error, Debug, PartialEq, Eq)]
pub enum EbpfApiError {
    #[error("Invalid program type: 0x{0:x}")]
    InvalidProgramType(u32),

    #[error("Unsupported program type: {0:?}")]
    UnsupportedProgramType(ProgramType),

    #[error("Invalid expected_attach_type: 0x{0:?}")]
    InvalidExpectedAttachType(AttachType),
}

impl TryFrom<u32> for ProgramType {
    type Error = EbpfApiError;

    fn try_from(program_type: u32) -> Result<Self, Self::Error> {
        match program_type {
            #![allow(non_upper_case_globals)]
            bpf_prog_type_BPF_PROG_TYPE_CGROUP_DEVICE => Ok(Self::CgroupDevice),
            bpf_prog_type_BPF_PROG_TYPE_CGROUP_SKB => Ok(Self::CgroupSkb),
            bpf_prog_type_BPF_PROG_TYPE_CGROUP_SOCK => Ok(Self::CgroupSock),
            bpf_prog_type_BPF_PROG_TYPE_CGROUP_SOCK_ADDR => Ok(Self::CgroupSockAddr),
            bpf_prog_type_BPF_PROG_TYPE_CGROUP_SOCKOPT => Ok(Self::CgroupSockopt),
            bpf_prog_type_BPF_PROG_TYPE_CGROUP_SYSCTL => Ok(Self::CgroupSysctl),
            bpf_prog_type_BPF_PROG_TYPE_EXT => Ok(Self::Ext),
            bpf_prog_type_BPF_PROG_TYPE_FLOW_DISSECTOR => Ok(Self::FlowDissector),
            bpf_prog_type_BPF_PROG_TYPE_KPROBE => Ok(Self::Kprobe),
            bpf_prog_type_BPF_PROG_TYPE_LIRC_MODE2 => Ok(Self::LircMode2),
            bpf_prog_type_BPF_PROG_TYPE_LSM => Ok(Self::Lsm),
            bpf_prog_type_BPF_PROG_TYPE_LWT_IN => Ok(Self::LwtIn),
            bpf_prog_type_BPF_PROG_TYPE_LWT_OUT => Ok(Self::LwtOut),
            bpf_prog_type_BPF_PROG_TYPE_LWT_SEG6LOCAL => Ok(Self::LwtSeg6Local),
            bpf_prog_type_BPF_PROG_TYPE_LWT_XMIT => Ok(Self::LwtXmit),
            bpf_prog_type_BPF_PROG_TYPE_NETFILTER => Ok(Self::Netfilter),
            bpf_prog_type_BPF_PROG_TYPE_PERF_EVENT => Ok(Self::PerfEvent),
            bpf_prog_type_BPF_PROG_TYPE_RAW_TRACEPOINT => Ok(Self::RawTracepoint),
            bpf_prog_type_BPF_PROG_TYPE_RAW_TRACEPOINT_WRITABLE => Ok(Self::RawTracepointWritable),
            bpf_prog_type_BPF_PROG_TYPE_SCHED_ACT => Ok(Self::SchedAct),
            bpf_prog_type_BPF_PROG_TYPE_SCHED_CLS => Ok(Self::SchedCls),
            bpf_prog_type_BPF_PROG_TYPE_SK_LOOKUP => Ok(Self::SkLookup),
            bpf_prog_type_BPF_PROG_TYPE_SK_MSG => Ok(Self::SkMsg),
            bpf_prog_type_BPF_PROG_TYPE_SK_REUSEPORT => Ok(Self::SkReuseport),
            bpf_prog_type_BPF_PROG_TYPE_SK_SKB => Ok(Self::SkSkb),
            bpf_prog_type_BPF_PROG_TYPE_SOCK_OPS => Ok(Self::SockOps),
            bpf_prog_type_BPF_PROG_TYPE_SOCKET_FILTER => Ok(Self::SocketFilter),
            bpf_prog_type_BPF_PROG_TYPE_STRUCT_OPS => Ok(Self::StructOps),
            bpf_prog_type_BPF_PROG_TYPE_SYSCALL => Ok(Self::Syscall),
            bpf_prog_type_BPF_PROG_TYPE_TRACEPOINT => Ok(Self::Tracepoint),
            bpf_prog_type_BPF_PROG_TYPE_TRACING => Ok(Self::Tracing),
            bpf_prog_type_BPF_PROG_TYPE_UNSPEC => Ok(Self::Unspec),
            bpf_prog_type_BPF_PROG_TYPE_XDP => Ok(Self::Xdp),
            BPF_PROG_TYPE_FUSE => Ok(Self::Fuse),
            program_type @ _ => Err(EbpfApiError::InvalidProgramType(program_type)),
        }
    }
}

impl From<ProgramType> for u32 {
    fn from(program_type: ProgramType) -> u32 {
        match program_type {
            ProgramType::CgroupDevice => bpf_prog_type_BPF_PROG_TYPE_CGROUP_DEVICE,
            ProgramType::CgroupSkb => bpf_prog_type_BPF_PROG_TYPE_CGROUP_SKB,
            ProgramType::CgroupSock => bpf_prog_type_BPF_PROG_TYPE_CGROUP_SOCK,
            ProgramType::CgroupSockAddr => bpf_prog_type_BPF_PROG_TYPE_CGROUP_SOCK_ADDR,
            ProgramType::CgroupSockopt => bpf_prog_type_BPF_PROG_TYPE_CGROUP_SOCKOPT,
            ProgramType::CgroupSysctl => bpf_prog_type_BPF_PROG_TYPE_CGROUP_SYSCTL,
            ProgramType::Ext => bpf_prog_type_BPF_PROG_TYPE_EXT,
            ProgramType::FlowDissector => bpf_prog_type_BPF_PROG_TYPE_FLOW_DISSECTOR,
            ProgramType::Kprobe => bpf_prog_type_BPF_PROG_TYPE_KPROBE,
            ProgramType::LircMode2 => bpf_prog_type_BPF_PROG_TYPE_LIRC_MODE2,
            ProgramType::Lsm => bpf_prog_type_BPF_PROG_TYPE_LSM,
            ProgramType::LwtIn => bpf_prog_type_BPF_PROG_TYPE_LWT_IN,
            ProgramType::LwtOut => bpf_prog_type_BPF_PROG_TYPE_LWT_OUT,
            ProgramType::LwtSeg6Local => bpf_prog_type_BPF_PROG_TYPE_LWT_SEG6LOCAL,
            ProgramType::LwtXmit => bpf_prog_type_BPF_PROG_TYPE_LWT_XMIT,
            ProgramType::Netfilter => bpf_prog_type_BPF_PROG_TYPE_NETFILTER,
            ProgramType::PerfEvent => bpf_prog_type_BPF_PROG_TYPE_PERF_EVENT,
            ProgramType::RawTracepoint => bpf_prog_type_BPF_PROG_TYPE_RAW_TRACEPOINT,
            ProgramType::RawTracepointWritable => {
                bpf_prog_type_BPF_PROG_TYPE_RAW_TRACEPOINT_WRITABLE
            }
            ProgramType::SchedAct => bpf_prog_type_BPF_PROG_TYPE_SCHED_ACT,
            ProgramType::SchedCls => bpf_prog_type_BPF_PROG_TYPE_SCHED_CLS,
            ProgramType::SkLookup => bpf_prog_type_BPF_PROG_TYPE_SK_LOOKUP,
            ProgramType::SkMsg => bpf_prog_type_BPF_PROG_TYPE_SK_MSG,
            ProgramType::SkReuseport => bpf_prog_type_BPF_PROG_TYPE_SK_REUSEPORT,
            ProgramType::SkSkb => bpf_prog_type_BPF_PROG_TYPE_SK_SKB,
            ProgramType::SockOps => bpf_prog_type_BPF_PROG_TYPE_SOCK_OPS,
            ProgramType::SocketFilter => bpf_prog_type_BPF_PROG_TYPE_SOCKET_FILTER,
            ProgramType::StructOps => bpf_prog_type_BPF_PROG_TYPE_STRUCT_OPS,
            ProgramType::Syscall => bpf_prog_type_BPF_PROG_TYPE_SYSCALL,
            ProgramType::Tracepoint => bpf_prog_type_BPF_PROG_TYPE_TRACEPOINT,
            ProgramType::Tracing => bpf_prog_type_BPF_PROG_TYPE_TRACING,
            ProgramType::Unspec => bpf_prog_type_BPF_PROG_TYPE_UNSPEC,
            ProgramType::Xdp => bpf_prog_type_BPF_PROG_TYPE_XDP,
            ProgramType::Fuse => BPF_PROG_TYPE_FUSE,
        }
    }
}

impl ProgramType {
    pub fn get_helpers(self) -> HashMap<u32, &'static HelperDefinition> {
        BPF_HELPERS_DEFINITIONS
            .iter()
            .filter_map(|(filter, helper)| filter.accept(self).then_some((helper.index, helper)))
            .collect()
    }

    pub fn get_args(
        self,
        expected_attach_type: AttachType,
    ) -> Result<&'static [Type], EbpfApiError> {
        let args = match self {
            Self::SocketFilter => &SOCKET_FILTER_ARGS,
            Self::SchedAct | Self::SchedCls => &SCHED_ARGS,
            Self::CgroupSkb => match expected_attach_type {
                AttachType::Unspecified
                | AttachType::CgroupInetIngress
                | AttachType::CgroupInetEgress => &CGROUP_SKB_ARGS,
                _ => return Err(EbpfApiError::InvalidExpectedAttachType(expected_attach_type)),
            },

            Self::Xdp => &XDP_MD_ARGS,
            Self::Kprobe => &BPF_USER_PT_REGS_T_ARGS,
            Self::Tracepoint => &BPF_TRACEPOINT_ARGS,
            Self::RawTracepoint => &BPF_RAW_TRACEPOINT_ARGS,

            Self::CgroupSock => match expected_attach_type {
                AttachType::Unspecified
                | AttachType::CgroupInetIngress
                | AttachType::CgroupInetSockCreate
                | AttachType::CgroupInet4PostBind
                | AttachType::CgroupInet6PostBind
                | AttachType::CgroupInetSockRelease => &BPF_SOCK_ARGS,
                _ => return Err(EbpfApiError::InvalidExpectedAttachType(expected_attach_type)),
            },

            Self::CgroupSockopt => match expected_attach_type {
                AttachType::CgroupGetsockopt | AttachType::CgroupSetsockopt => &BPF_SOCKOPT_ARGS,
                _ => return Err(EbpfApiError::InvalidExpectedAttachType(expected_attach_type)),
            },

            Self::CgroupSockAddr => match expected_attach_type {
                AttachType::CgroupInet4Bind
                | AttachType::CgroupInet4Connect
                | AttachType::CgroupUdp4Sendmsg
                | AttachType::CgroupUdp4Recvmsg
                | AttachType::CgroupInet4Getpeername
                | AttachType::CgroupInet4Getsockname => &BPF_SOCK_ADDR_INET4_ARGS,

                AttachType::CgroupInet6Bind
                | AttachType::CgroupInet6Connect
                | AttachType::CgroupUdp6Sendmsg
                | AttachType::CgroupUdp6Recvmsg
                | AttachType::CgroupInet6Getpeername
                | AttachType::CgroupInet6Getsockname => &BPF_SOCK_ADDR_INET6_ARGS,

                _ => return Err(EbpfApiError::InvalidExpectedAttachType(expected_attach_type)),
            },

            Self::Fuse => &BPF_FUSE_ARGS,

            Self::CgroupDevice
            | Self::CgroupSysctl
            | Self::Ext
            | Self::FlowDissector
            | Self::LircMode2
            | Self::Lsm
            | Self::LwtIn
            | Self::LwtOut
            | Self::LwtSeg6Local
            | Self::LwtXmit
            | Self::Netfilter
            | Self::PerfEvent
            | Self::RawTracepointWritable
            | Self::SkLookup
            | Self::SkMsg
            | Self::SkReuseport
            | Self::SkSkb
            | Self::SockOps
            | Self::StructOps
            | Self::Syscall
            | Self::Tracing
            | Self::Unspec => return Err(EbpfApiError::UnsupportedProgramType(self)),
        };
        Ok(args)
    }

    pub fn create_calling_context(
        self,
        expected_attach_type: AttachType,
        maps: Vec<MapSchema>,
    ) -> Result<CallingContext, EbpfApiError> {
        let args = self.get_args(expected_attach_type)?.to_vec();
        let packet_type = match self {
            Self::SocketFilter => Some(SOCKET_FILTER_SK_BUF_TYPE.clone()),
            Self::SchedAct | Self::SchedCls => Some(SCHED_ARG_TYPE.clone()),
            _ => None,
        };
        Ok(CallingContext { maps, helpers: self.get_helpers(), args, packet_type })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachType {
    CgroupInetIngress,
    CgroupInetEgress,
    CgroupInetSockCreate,
    CgroupSockOps,
    SkSkbStreamParser,
    SkSkbStreamVerdict,
    CgroupDevice,
    SkMsgVerdict,
    CgroupInet4Bind,
    CgroupInet6Bind,
    CgroupInet4Connect,
    CgroupInet6Connect,
    CgroupInet4PostBind,
    CgroupInet6PostBind,
    CgroupUdp4Sendmsg,
    CgroupUdp6Sendmsg,
    LircMode2,
    FlowDissector,
    CgroupSysctl,
    CgroupUdp4Recvmsg,
    CgroupUdp6Recvmsg,
    CgroupGetsockopt,
    CgroupSetsockopt,
    TraceRawTp,
    TraceFentry,
    TraceFexit,
    ModifyReturn,
    LsmMac,
    TraceIter,
    CgroupInet4Getpeername,
    CgroupInet6Getpeername,
    CgroupInet4Getsockname,
    CgroupInet6Getsockname,
    XdpDevmap,
    CgroupInetSockRelease,
    XdpCpumap,
    SkLookup,
    Xdp,
    SkSkbVerdict,
    SkReuseportSelect,
    SkReuseportSelectOrMigrate,
    PerfEvent,
    TraceKprobeMulti,
    LsmCgroup,
    StructOps,
    Netfilter,
    TcxIngress,
    TcxEgress,
    TraceUprobeMulti,
    CgroupUnixConnect,
    CgroupUnixSendmsg,
    CgroupUnixRecvmsg,
    CgroupUnixGetpeername,
    CgroupUnixGetsockname,
    NetkitPrimary,
    NetkitPeer,
    TraceKprobeSession,

    // Corresponds to `attach_type=-1`. Linux allows this value in
    // `expected_attach_type` for some `program_types`
    Unspecified,

    // Corresponds to any `attach_type` value other than -1 and
    // `bpf_attach_type` enum values.
    Invalid(u32),
}

impl From<u32> for AttachType {
    fn from(attach_type: u32) -> Self {
        match attach_type {
            #![allow(non_upper_case_globals)]
            bpf_attach_type_BPF_CGROUP_INET_INGRESS => Self::CgroupInetIngress,
            bpf_attach_type_BPF_CGROUP_INET_EGRESS => Self::CgroupInetEgress,
            bpf_attach_type_BPF_CGROUP_INET_SOCK_CREATE => Self::CgroupInetSockCreate,
            bpf_attach_type_BPF_CGROUP_SOCK_OPS => Self::CgroupSockOps,
            bpf_attach_type_BPF_SK_SKB_STREAM_PARSER => Self::SkSkbStreamParser,
            bpf_attach_type_BPF_SK_SKB_STREAM_VERDICT => Self::SkSkbStreamVerdict,
            bpf_attach_type_BPF_CGROUP_DEVICE => Self::CgroupDevice,
            bpf_attach_type_BPF_SK_MSG_VERDICT => Self::SkMsgVerdict,
            bpf_attach_type_BPF_CGROUP_INET4_BIND => Self::CgroupInet4Bind,
            bpf_attach_type_BPF_CGROUP_INET6_BIND => Self::CgroupInet6Bind,
            bpf_attach_type_BPF_CGROUP_INET4_CONNECT => Self::CgroupInet4Connect,
            bpf_attach_type_BPF_CGROUP_INET6_CONNECT => Self::CgroupInet6Connect,
            bpf_attach_type_BPF_CGROUP_INET4_POST_BIND => Self::CgroupInet4PostBind,
            bpf_attach_type_BPF_CGROUP_INET6_POST_BIND => Self::CgroupInet6PostBind,
            bpf_attach_type_BPF_CGROUP_UDP4_SENDMSG => Self::CgroupUdp4Sendmsg,
            bpf_attach_type_BPF_CGROUP_UDP6_SENDMSG => Self::CgroupUdp6Sendmsg,
            bpf_attach_type_BPF_LIRC_MODE2 => Self::LircMode2,
            bpf_attach_type_BPF_FLOW_DISSECTOR => Self::FlowDissector,
            bpf_attach_type_BPF_CGROUP_SYSCTL => Self::CgroupSysctl,
            bpf_attach_type_BPF_CGROUP_UDP4_RECVMSG => Self::CgroupUdp4Recvmsg,
            bpf_attach_type_BPF_CGROUP_UDP6_RECVMSG => Self::CgroupUdp6Recvmsg,
            bpf_attach_type_BPF_CGROUP_GETSOCKOPT => Self::CgroupGetsockopt,
            bpf_attach_type_BPF_CGROUP_SETSOCKOPT => Self::CgroupSetsockopt,
            bpf_attach_type_BPF_TRACE_RAW_TP => Self::TraceRawTp,
            bpf_attach_type_BPF_TRACE_FENTRY => Self::TraceFentry,
            bpf_attach_type_BPF_TRACE_FEXIT => Self::TraceFexit,
            bpf_attach_type_BPF_MODIFY_RETURN => Self::ModifyReturn,
            bpf_attach_type_BPF_LSM_MAC => Self::LsmMac,
            bpf_attach_type_BPF_TRACE_ITER => Self::TraceIter,
            bpf_attach_type_BPF_CGROUP_INET4_GETPEERNAME => Self::CgroupInet4Getpeername,
            bpf_attach_type_BPF_CGROUP_INET6_GETPEERNAME => Self::CgroupInet6Getpeername,
            bpf_attach_type_BPF_CGROUP_INET4_GETSOCKNAME => Self::CgroupInet4Getsockname,
            bpf_attach_type_BPF_CGROUP_INET6_GETSOCKNAME => Self::CgroupInet6Getsockname,
            bpf_attach_type_BPF_XDP_DEVMAP => Self::XdpDevmap,
            bpf_attach_type_BPF_CGROUP_INET_SOCK_RELEASE => Self::CgroupInetSockRelease,
            bpf_attach_type_BPF_XDP_CPUMAP => Self::XdpCpumap,
            bpf_attach_type_BPF_SK_LOOKUP => Self::SkLookup,
            bpf_attach_type_BPF_XDP => Self::Xdp,
            bpf_attach_type_BPF_SK_SKB_VERDICT => Self::SkSkbVerdict,
            bpf_attach_type_BPF_SK_REUSEPORT_SELECT => Self::SkReuseportSelect,
            bpf_attach_type_BPF_SK_REUSEPORT_SELECT_OR_MIGRATE => Self::SkReuseportSelectOrMigrate,
            bpf_attach_type_BPF_PERF_EVENT => Self::PerfEvent,
            bpf_attach_type_BPF_TRACE_KPROBE_MULTI => Self::TraceKprobeMulti,
            bpf_attach_type_BPF_LSM_CGROUP => Self::LsmCgroup,
            bpf_attach_type_BPF_STRUCT_OPS => Self::StructOps,
            bpf_attach_type_BPF_NETFILTER => Self::Netfilter,
            bpf_attach_type_BPF_TCX_INGRESS => Self::TcxIngress,
            bpf_attach_type_BPF_TCX_EGRESS => Self::TcxEgress,
            bpf_attach_type_BPF_TRACE_UPROBE_MULTI => Self::TraceUprobeMulti,
            bpf_attach_type_BPF_CGROUP_UNIX_CONNECT => Self::CgroupUnixConnect,
            bpf_attach_type_BPF_CGROUP_UNIX_SENDMSG => Self::CgroupUnixSendmsg,
            bpf_attach_type_BPF_CGROUP_UNIX_RECVMSG => Self::CgroupUnixRecvmsg,
            bpf_attach_type_BPF_CGROUP_UNIX_GETPEERNAME => Self::CgroupUnixGetpeername,
            bpf_attach_type_BPF_CGROUP_UNIX_GETSOCKNAME => Self::CgroupUnixGetsockname,
            bpf_attach_type_BPF_NETKIT_PRIMARY => Self::NetkitPrimary,
            bpf_attach_type_BPF_NETKIT_PEER => Self::NetkitPeer,
            bpf_attach_type_BPF_TRACE_KPROBE_SESSION => Self::TraceKprobeSession,

            u32::MAX => Self::Unspecified,
            _ => Self::Invalid(attach_type),
        }
    }
}

impl From<AttachType> for u32 {
    fn from(attach_type: AttachType) -> Self {
        match attach_type {
            AttachType::CgroupInetIngress => bpf_attach_type_BPF_CGROUP_INET_INGRESS,
            AttachType::CgroupInetEgress => bpf_attach_type_BPF_CGROUP_INET_EGRESS,
            AttachType::CgroupInetSockCreate => bpf_attach_type_BPF_CGROUP_INET_SOCK_CREATE,
            AttachType::CgroupSockOps => bpf_attach_type_BPF_CGROUP_SOCK_OPS,
            AttachType::SkSkbStreamParser => bpf_attach_type_BPF_SK_SKB_STREAM_PARSER,
            AttachType::SkSkbStreamVerdict => bpf_attach_type_BPF_SK_SKB_STREAM_VERDICT,
            AttachType::CgroupDevice => bpf_attach_type_BPF_CGROUP_DEVICE,
            AttachType::SkMsgVerdict => bpf_attach_type_BPF_SK_MSG_VERDICT,
            AttachType::CgroupInet4Bind => bpf_attach_type_BPF_CGROUP_INET4_BIND,
            AttachType::CgroupInet6Bind => bpf_attach_type_BPF_CGROUP_INET6_BIND,
            AttachType::CgroupInet4Connect => bpf_attach_type_BPF_CGROUP_INET4_CONNECT,
            AttachType::CgroupInet6Connect => bpf_attach_type_BPF_CGROUP_INET6_CONNECT,
            AttachType::CgroupInet4PostBind => bpf_attach_type_BPF_CGROUP_INET4_POST_BIND,
            AttachType::CgroupInet6PostBind => bpf_attach_type_BPF_CGROUP_INET6_POST_BIND,
            AttachType::CgroupUdp4Sendmsg => bpf_attach_type_BPF_CGROUP_UDP4_SENDMSG,
            AttachType::CgroupUdp6Sendmsg => bpf_attach_type_BPF_CGROUP_UDP6_SENDMSG,
            AttachType::LircMode2 => bpf_attach_type_BPF_LIRC_MODE2,
            AttachType::FlowDissector => bpf_attach_type_BPF_FLOW_DISSECTOR,
            AttachType::CgroupSysctl => bpf_attach_type_BPF_CGROUP_SYSCTL,
            AttachType::CgroupUdp4Recvmsg => bpf_attach_type_BPF_CGROUP_UDP4_RECVMSG,
            AttachType::CgroupUdp6Recvmsg => bpf_attach_type_BPF_CGROUP_UDP6_RECVMSG,
            AttachType::CgroupGetsockopt => bpf_attach_type_BPF_CGROUP_GETSOCKOPT,
            AttachType::CgroupSetsockopt => bpf_attach_type_BPF_CGROUP_SETSOCKOPT,
            AttachType::TraceRawTp => bpf_attach_type_BPF_TRACE_RAW_TP,
            AttachType::TraceFentry => bpf_attach_type_BPF_TRACE_FENTRY,
            AttachType::TraceFexit => bpf_attach_type_BPF_TRACE_FEXIT,
            AttachType::ModifyReturn => bpf_attach_type_BPF_MODIFY_RETURN,
            AttachType::LsmMac => bpf_attach_type_BPF_LSM_MAC,
            AttachType::TraceIter => bpf_attach_type_BPF_TRACE_ITER,
            AttachType::CgroupInet4Getpeername => bpf_attach_type_BPF_CGROUP_INET4_GETPEERNAME,
            AttachType::CgroupInet6Getpeername => bpf_attach_type_BPF_CGROUP_INET6_GETPEERNAME,
            AttachType::CgroupInet4Getsockname => bpf_attach_type_BPF_CGROUP_INET4_GETSOCKNAME,
            AttachType::CgroupInet6Getsockname => bpf_attach_type_BPF_CGROUP_INET6_GETSOCKNAME,
            AttachType::XdpDevmap => bpf_attach_type_BPF_XDP_DEVMAP,
            AttachType::CgroupInetSockRelease => bpf_attach_type_BPF_CGROUP_INET_SOCK_RELEASE,
            AttachType::XdpCpumap => bpf_attach_type_BPF_XDP_CPUMAP,
            AttachType::SkLookup => bpf_attach_type_BPF_SK_LOOKUP,
            AttachType::Xdp => bpf_attach_type_BPF_XDP,
            AttachType::SkSkbVerdict => bpf_attach_type_BPF_SK_SKB_VERDICT,
            AttachType::SkReuseportSelect => bpf_attach_type_BPF_SK_REUSEPORT_SELECT,
            AttachType::SkReuseportSelectOrMigrate => {
                bpf_attach_type_BPF_SK_REUSEPORT_SELECT_OR_MIGRATE
            }
            AttachType::PerfEvent => bpf_attach_type_BPF_PERF_EVENT,
            AttachType::TraceKprobeMulti => bpf_attach_type_BPF_TRACE_KPROBE_MULTI,
            AttachType::LsmCgroup => bpf_attach_type_BPF_LSM_CGROUP,
            AttachType::StructOps => bpf_attach_type_BPF_STRUCT_OPS,
            AttachType::Netfilter => bpf_attach_type_BPF_NETFILTER,
            AttachType::TcxIngress => bpf_attach_type_BPF_TCX_INGRESS,
            AttachType::TcxEgress => bpf_attach_type_BPF_TCX_EGRESS,
            AttachType::TraceUprobeMulti => bpf_attach_type_BPF_TRACE_UPROBE_MULTI,
            AttachType::CgroupUnixConnect => bpf_attach_type_BPF_CGROUP_UNIX_CONNECT,
            AttachType::CgroupUnixSendmsg => bpf_attach_type_BPF_CGROUP_UNIX_SENDMSG,
            AttachType::CgroupUnixRecvmsg => bpf_attach_type_BPF_CGROUP_UNIX_RECVMSG,
            AttachType::CgroupUnixGetpeername => bpf_attach_type_BPF_CGROUP_UNIX_GETPEERNAME,
            AttachType::CgroupUnixGetsockname => bpf_attach_type_BPF_CGROUP_UNIX_GETSOCKNAME,
            AttachType::NetkitPrimary => bpf_attach_type_BPF_NETKIT_PRIMARY,
            AttachType::NetkitPeer => bpf_attach_type_BPF_NETKIT_PEER,
            AttachType::TraceKprobeSession => bpf_attach_type_BPF_TRACE_KPROBE_SESSION,
            AttachType::Unspecified => u32::MAX,
            AttachType::Invalid(attach_type) => attach_type,
        }
    }
}

impl From<AttachType> for u64 {
    fn from(attach_type: AttachType) -> Self {
        (u32::from(attach_type)).into()
    }
}

impl AttachType {
    pub fn is_cgroup(&self) -> bool {
        match self {
            Self::CgroupInetIngress
            | Self::CgroupInetEgress
            | Self::CgroupInetSockCreate
            | Self::CgroupSockOps
            | Self::CgroupDevice
            | Self::CgroupInet4Bind
            | Self::CgroupInet6Bind
            | Self::CgroupInet4Connect
            | Self::CgroupInet6Connect
            | Self::CgroupInet4PostBind
            | Self::CgroupInet6PostBind
            | Self::CgroupUdp4Sendmsg
            | Self::CgroupUdp6Sendmsg
            | Self::CgroupSysctl
            | Self::CgroupUdp4Recvmsg
            | Self::CgroupUdp6Recvmsg
            | Self::CgroupGetsockopt
            | Self::CgroupSetsockopt
            | Self::CgroupInet4Getpeername
            | Self::CgroupInet6Getpeername
            | Self::CgroupInet4Getsockname
            | Self::CgroupInet6Getsockname
            | Self::CgroupInetSockRelease
            | Self::CgroupUnixConnect
            | Self::CgroupUnixSendmsg
            | Self::CgroupUnixRecvmsg
            | Self::CgroupUnixGetpeername
            | Self::CgroupUnixGetsockname => true,
            _ => false,
        }
    }

    pub fn get_program_type(&self) -> ProgramType {
        match self {
            Self::CgroupInetIngress | Self::CgroupInetEgress => ProgramType::CgroupSkb,
            Self::CgroupInetSockCreate
            | Self::CgroupInet4PostBind
            | Self::CgroupInet6PostBind
            | Self::CgroupInetSockRelease => ProgramType::CgroupSock,
            Self::CgroupSockOps | Self::CgroupGetsockopt | Self::CgroupSetsockopt => {
                ProgramType::CgroupSockopt
            }
            Self::CgroupDevice => ProgramType::CgroupDevice,
            Self::CgroupInet4Bind
            | Self::CgroupInet6Bind
            | Self::CgroupInet4Connect
            | Self::CgroupInet6Connect
            | Self::CgroupUdp4Sendmsg
            | Self::CgroupUdp6Sendmsg
            | Self::CgroupUdp4Recvmsg
            | Self::CgroupUdp6Recvmsg
            | Self::CgroupInet4Getpeername
            | Self::CgroupInet6Getpeername
            | Self::CgroupInet4Getsockname
            | Self::CgroupInet6Getsockname
            | Self::CgroupUnixConnect
            | Self::CgroupUnixSendmsg
            | Self::CgroupUnixRecvmsg
            | Self::CgroupUnixGetpeername
            | Self::CgroupUnixGetsockname => ProgramType::CgroupSockAddr,
            Self::CgroupSysctl => ProgramType::CgroupSysctl,
            Self::FlowDissector => ProgramType::FlowDissector,
            Self::LircMode2 => ProgramType::LircMode2,
            Self::LsmMac | Self::LsmCgroup => ProgramType::Lsm,
            Self::Netfilter => ProgramType::Netfilter,
            Self::PerfEvent => ProgramType::PerfEvent,
            Self::SkLookup => ProgramType::SkLookup,
            Self::SkMsgVerdict | Self::SkSkbVerdict => ProgramType::SkMsg,
            Self::SkReuseportSelect | Self::SkReuseportSelectOrMigrate => ProgramType::SkReuseport,
            Self::SkSkbStreamParser | Self::SkSkbStreamVerdict => ProgramType::SkSkb,
            Self::StructOps => ProgramType::StructOps,
            Self::TcxIngress | Self::TcxEgress | Self::NetkitPrimary | Self::NetkitPeer => {
                ProgramType::SchedCls
            }
            Self::TraceKprobeMulti | Self::TraceUprobeMulti | Self::TraceKprobeSession => {
                ProgramType::Kprobe
            }
            Self::TraceRawTp
            | Self::TraceFentry
            | Self::TraceFexit
            | Self::ModifyReturn
            | Self::TraceIter => ProgramType::Tracing,
            Self::XdpDevmap | Self::XdpCpumap | Self::Xdp => ProgramType::Xdp,
            Self::Unspecified | Self::Invalid(_) => ProgramType::Unspec,
        }
    }

    // Returns true if the attachment should allow programs created with the
    // specified `expected_attach_type`.
    pub fn is_compatible_with_expected_attach_type(self, expected: AttachType) -> bool {
        // See https://docs.ebpf.io/linux/syscall/BPF_PROG_LOAD/#expected_attach_type.
        match self {
            // Egress and Ingress attachments are interchangeable. Also `expected_attach_type=-1` is
            // allowed in both cases.
            Self::CgroupInetIngress | Self::CgroupInetEgress => matches!(
                expected,
                Self::Unspecified | Self::CgroupInetIngress | Self::CgroupInetEgress
            ),

            // These attachments allow `expected_attach_type` to be set to
            // -1 or 0 (BPF_CGROUP_INET_INGRESS).
            Self::CgroupInetSockCreate | Self::CgroupSockOps => {
                self == expected || matches!(expected, Self::Unspecified | Self::CgroupInetIngress)
            }

            // For these attachments `expected_attach_type` must match.
            Self::CgroupGetsockopt
            | Self::CgroupInet4Bind
            | Self::CgroupInet4Connect
            | Self::CgroupInet4Getpeername
            | Self::CgroupInet4Getsockname
            | Self::CgroupInet4PostBind
            | Self::CgroupInet6Bind
            | Self::CgroupInet6Connect
            | Self::CgroupInet6Getpeername
            | Self::CgroupInet6Getsockname
            | Self::CgroupInet6PostBind
            | Self::CgroupInetSockRelease
            | Self::CgroupSetsockopt
            | Self::CgroupUdp4Recvmsg
            | Self::CgroupUdp4Sendmsg
            | Self::CgroupUdp6Recvmsg
            | Self::CgroupUdp6Sendmsg
            | Self::CgroupUnixConnect
            | Self::CgroupUnixGetpeername
            | Self::CgroupUnixGetsockname
            | Self::CgroupUnixRecvmsg
            | Self::CgroupUnixSendmsg => self == expected,

            // `expected_attach_type` is ignored for all other attachments.
            _ => true,
        }
    }
}

// Offset used to access auxiliary packet information in cBPF.
pub const SKF_AD_OFF: i32 = linux_uapi::SKF_AD_OFF;
pub const SKF_AD_PROTOCOL: i32 = linux_uapi::SKF_AD_PROTOCOL as i32;
pub const SKF_AD_PKTTYPE: i32 = linux_uapi::SKF_AD_PKTTYPE as i32;
pub const SKF_AD_IFINDEX: i32 = linux_uapi::SKF_AD_IFINDEX as i32;
pub const SKF_AD_NLATTR: i32 = linux_uapi::SKF_AD_NLATTR as i32;
pub const SKF_AD_NLATTR_NEST: i32 = linux_uapi::SKF_AD_NLATTR_NEST as i32;
pub const SKF_AD_MARK: i32 = linux_uapi::SKF_AD_MARK as i32;
pub const SKF_AD_QUEUE: i32 = linux_uapi::SKF_AD_QUEUE as i32;
pub const SKF_AD_HATYPE: i32 = linux_uapi::SKF_AD_HATYPE as i32;
pub const SKF_AD_RXHASH: i32 = linux_uapi::SKF_AD_RXHASH as i32;
pub const SKF_AD_CPU: i32 = linux_uapi::SKF_AD_CPU as i32;
pub const SKF_AD_ALU_XOR_X: i32 = linux_uapi::SKF_AD_ALU_XOR_X as i32;
pub const SKF_AD_VLAN_TAG: i32 = linux_uapi::SKF_AD_VLAN_TAG as i32;
pub const SKF_AD_VLAN_TAG_PRESENT: i32 = linux_uapi::SKF_AD_VLAN_TAG_PRESENT as i32;
pub const SKF_AD_PAY_OFFSET: i32 = linux_uapi::SKF_AD_PAY_OFFSET as i32;
pub const SKF_AD_RANDOM: i32 = linux_uapi::SKF_AD_RANDOM as i32;
pub const SKF_AD_VLAN_TPID: i32 = linux_uapi::SKF_AD_VLAN_TPID as i32;
pub const SKF_AD_MAX: i32 = linux_uapi::SKF_AD_MAX as i32;

// Offset used to reference IP headers in cBPF.
pub const SKF_NET_OFF: i32 = linux_uapi::SKF_NET_OFF;

// Offset used to reference Ethernet headers in cBPF.
pub const SKF_LL_OFF: i32 = linux_uapi::SKF_LL_OFF;

pub const SECCOMP_CBPF_CONFIG: CbpfConfig = CbpfConfig {
    len: CbpfLenInstruction::Static { len: size_of::<seccomp_data>() as i32 },
    allow_msh: false,
};

pub const SOCKET_FILTER_CBPF_CONFIG: CbpfConfig = CbpfConfig {
    len: CbpfLenInstruction::ContextField { offset: offset_of!(__sk_buff, len) as i16 },
    allow_msh: true,
};
