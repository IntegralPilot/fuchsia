// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::bindings::BindingsCtx;
use crate::bindings::util::IntoCore;
use ebpf::{
    BpfProgramContext, BpfValue, CbpfConfig, DataWidth, EbpfBufferPtr, EbpfInstruction,
    EbpfProgram, EbpfProgramContext, FieldMapping, FromBpfValue, MapReference, MapSchema, Packet,
    ProgramArgument, Type, VerifiedEbpfProgram,
};
use ebpf_api::{
    __sk_buff, BpfSockContext, CGROUP_SKB_ARGS, CGROUP_SKB_SK_BUF_TYPE, Map, MapError, MapValueRef,
    PacketWithLoadBytes, PinnedMap, SKF_AD_OFF, SKF_AD_PKTTYPE, SKF_AD_PROTOCOL, SKF_LL_OFF,
    SKF_NET_OFF, SOCKET_FILTER_ARGS, SOCKET_FILTER_CBPF_CONFIG, SOCKET_FILTER_SK_BUF_TYPE,
    SocketRef, StructId, bpf_sock, uaddr, uid_t,
};
use fidl_fuchsia_ebpf as febpf;
use fidl_fuchsia_net as fnet;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_posix as fposix;
use fidl_table_validation::ValidFidlTable;
use log::{error, warn};
use net_types::ip::IpVersion;
use netstack3_core::NetworkSerializationContext;
use netstack3_core::device::DeviceId;
use netstack3_core::filter::{
    BindingsPacketMatcher, EitherIpProto, FilterIpExt, FilterIpPacket, FilterPacketMetadata,
    Interfaces, SocketEgressFilterResult, SocketInfo, SocketIngressFilterResult, SocketOpsFilter,
};
use netstack3_core::ip::{Mark, Marks};

use netstack3_core::sync::{Mutex, RwLock};
use netstack3_core::trace::trace_duration;
use packet::{Buf, FragmentedByteSlice, LayoutBufferAlloc, PartialSerializeResult};
use packet_formats::ethernet::EtherType;
use packet_formats::ip::{IpProto, Ipv4Proto, Ipv6Proto};
use smallvec::SmallVec;
use std::collections::{HashMap, hash_map};
use std::convert::Infallible as Never;
use std::mem::offset_of;
use std::sync::{Arc, Weak};
use zerocopy::FromBytes;

fn get_linux_packet_mark(marks: &Marks) -> u32 {
    let Mark(mark) = marks.get(fnet::MARK_DOMAIN_SO_MARK.into_core());
    // Default to 0 if the mark is not set.
    mark.unwrap_or(0)
}

// Transmutes `Vec<u64>` to `Vec<EbpfInstruction>`.
fn code_from_vec(code: Vec<u64>) -> Vec<EbpfInstruction> {
    // SAFETY:  This is safe because `EbpfInstruction` is 64 bits.
    unsafe {
        let mut code = std::mem::ManuallyDrop::new(code);
        Vec::from_raw_parts(code.as_mut_ptr() as *mut EbpfInstruction, code.len(), code.capacity())
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct BpfSock {
    // Must be first field.
    value: bpf_sock,

    socket_cookie: u64,
    socket_uid: Option<uid_t>,
}

impl BpfSock {
    pub fn new(socket_info: SocketInfo, marks: &Marks) -> Option<Self> {
        let type_ = match socket_info.proto {
            EitherIpProto::V4(Ipv4Proto::Proto(IpProto::Udp))
            | EitherIpProto::V6(Ipv6Proto::Proto(IpProto::Udp)) => libc::SOCK_DGRAM,
            EitherIpProto::V4(Ipv4Proto::Proto(IpProto::Tcp))
            | EitherIpProto::V6(Ipv6Proto::Proto(IpProto::Tcp)) => libc::SOCK_STREAM,
            EitherIpProto::V4(Ipv4Proto::Icmp) | EitherIpProto::V6(Ipv6Proto::Icmpv6) => {
                libc::SOCK_RAW
            }
            _ => 0,
        };
        let family = match socket_info.proto.ip_version() {
            IpVersion::V4 => libc::AF_INET,
            IpVersion::V6 => libc::AF_INET6,
        };
        let socket_cookie = socket_info.cookie.export_value();
        let Mark(socket_uid) = marks.get(fnet::MARK_DOMAIN_SOCKET_UID.into_core()).clone();
        Some(Self {
            value: bpf_sock {
                family: family as u32,
                type_: type_ as u32,
                protocol: socket_info.proto.u8_value() as u32,
                ..Default::default()
            },
            socket_cookie,
            socket_uid,
        })
    }
}

impl FromBpfValue<EbpfRunContext<'_>> for &'_ BpfSock {
    unsafe fn from_bpf_value(_context: &mut EbpfRunContext<'_>, v: BpfValue) -> Self {
        // SAFETY: Caller is expected to call this method only when verifier
        // checks that `v` is a pointer to `BpfSock`.
        unsafe { &*v.as_ptr::<BpfSock>() }
    }
}

impl SocketRef for &'_ BpfSock {
    fn get_socket_cookie(&self) -> Option<u64> {
        Some(self.socket_cookie)
    }

    fn get_socket_uid(&self) -> Option<uid_t> {
        self.socket_uid
    }
}

/// `__sk_buff` representation passed to eBPF programs.
///
/// `C` is used to define the type of the argument, see `SkBuffContext`. It is
/// needed because different `__sk_buff` fields are available depending on the
/// program type.
#[repr(C)]
pub struct SkBuff<'a, C> {
    sk_buff: __sk_buff,

    data_ptr: *const u8,
    data_end_ptr: *const u8,

    data: &'a [u8],

    // Offset of the network-layer header in the `data`.
    ip_offset: usize,

    // Default offset for packet load instructions.
    default_offset: usize,

    bpf_sock: Option<&'a BpfSock>,

    // Marker is `fn(C) -> C` to ensure that `SkBuff` is invariant over `C` and that
    // `Send` and `Sync` do not depend on `C`.
    _marker: std::marker::PhantomData<fn(C) -> C>,
}

struct SmallVecAlloc<'a, const N: usize> {
    buf: &'a mut SmallVec<[u8; N]>,
}

impl<'a, const N: usize> LayoutBufferAlloc<Buf<&'a mut [u8]>> for SmallVecAlloc<'a, N> {
    type Error = Never;

    fn layout_alloc(
        self,
        prefix: usize,
        body: usize,
        suffix: usize,
    ) -> Result<Buf<&'a mut [u8]>, Self::Error> {
        self.buf.resize(prefix + body + suffix, 0);
        Ok::<_, Never>(Buf::new(&mut self.buf[..], prefix..(prefix + body)))
    }
}

impl<'a, C> SkBuff<'a, C> {
    pub fn new(
        ethertype: Option<u16>,
        packet_len: usize,
        ifindex: u32,
        mark: u32,
        data: &'a [u8],
        ip_offset: usize,
        default_offset: usize,
        bpf_sock: Option<&'a BpfSock>,
    ) -> Self {
        // Offsets should be within the data buffer. They may be set to `data.len()`
        // if the packet is empty or it is not serialized.
        assert!(ip_offset <= data.len());
        assert!(default_offset <= data.len());

        let mut result = SkBuff {
            sk_buff: __sk_buff {
                len: packet_len.try_into().unwrap_or(0),
                mark,
                protocol: ethertype.unwrap_or(0).to_be().into(),
                ifindex,

                ..__sk_buff::default()
            },
            data_ptr: data.as_ptr(),
            // SAFETY: `data_end_ptr` points at the end of the data buffer, but it's never
            // dereferenced directly.
            data_end_ptr: unsafe { data.as_ptr().add(data.len()) },
            data,
            ip_offset,
            default_offset,
            bpf_sock,
            _marker: std::marker::PhantomData,
        };

        if let Some(bpf_sock) = bpf_sock {
            result.sk_buff.__bindgen_anon_2.sk =
                (uaddr { addr: BpfValue::from(bpf_sock).into() }).into();
        }

        result
    }

    fn from_ip_packet<I: FilterIpExt, P: FilterIpPacket<I>>(
        packet: &'a P,
        ifindex: u32,
        marks: &Marks,
        data_buffer: &'a mut SmallVec<[u8; PACKET_BUF_STACK_SIZE]>,
        bpf_sock: Option<&'a BpfSock>,
    ) -> Self {
        // TODO(https://fxbug.dev/424212358): Implement lazy packet serialization.
        let alloc = SmallVecAlloc { buf: data_buffer };
        let serialize_result = packet
            .partial_serialize(&mut NetworkSerializationContext::default(), alloc)
            .expect("Packet serialization failed");
        let (packet_data, packet_len) = match serialize_result {
            PartialSerializeResult::Slice(slice) => (slice, slice.len()),
            PartialSerializeResult::NewBuffer { buffer: _, total_size } => {
                (&data_buffer[..], total_size)
            }
        };

        let mark = get_linux_packet_mark(marks);
        let mut result = SkBuff {
            sk_buff: __sk_buff {
                len: packet_len.try_into().unwrap_or(0),
                mark,
                protocol: u16::from(I::ETHER_TYPE).to_be().into(),
                ifindex,
                ..__sk_buff::default()
            },
            data_ptr: packet_data.as_ptr(),
            // SAFETY: `data_end_ptr` points at the end of the data buffer, but it's never
            // dereferenced directly.
            data_end_ptr: unsafe { packet_data.as_ptr().add(packet_data.len()) },
            data: packet_data,
            ip_offset: 0,
            default_offset: 0,
            bpf_sock,
            _marker: std::marker::PhantomData,
        };

        if let Some(bpf_sock) = bpf_sock {
            result.sk_buff.__bindgen_anon_2.sk =
                (uaddr { addr: BpfValue::from(bpf_sock).into() }).into();
        }

        result
    }
}

impl<'a, C> PacketWithLoadBytes for &'a SkBuff<'a, C> {
    fn load_bytes_relative(
        &self,
        base: ebpf_api::LoadBytesBase,
        offset: usize,
        buf: EbpfBufferPtr<'_>,
    ) -> i64 {
        let base_offset = match base {
            ebpf_api::LoadBytesBase::MacHeader => {
                if self.ip_offset == 0 {
                    warn!("eBPF program tried to access Ethernet header when it's not available");
                    return -1;
                }
                0
            }
            ebpf_api::LoadBytesBase::NetworkHeader => self.ip_offset,
        };

        let Some(offset) = base_offset.checked_add(offset) else {
            return -1;
        };

        let Some(end) = offset.checked_add(buf.len()) else {
            return -1;
        };

        let Some(data) = self.data.get(offset..end) else {
            return -1;
        };

        buf.store(data);
        0
    }
}

impl<C> Packet for &'_ SkBuff<'_, C> {
    fn load<'a>(&self, offset: i32, width: DataWidth) -> Option<BpfValue> {
        // cBPF Socket Filters use non-negative offset to access packet content.
        // Negative offsets are handled as follows:
        //   SKF_AD_OFF (-0x1000) - Auxiliary info that may be outside of the packet.
        //      Currently only SKF_AD_PROTOCOL is implemented.
        //   SKF_NET_OFF (-0x100000) - Packet content relative to the IP header.
        //   SKF_LL_OFF (-0x200000) - Packet content relative to the link-level header.
        let (offset, slice) = if offset >= 0 {
            (offset, &self.data[self.default_offset..])
        } else if offset >= SKF_AD_OFF {
            let ad_offset = offset - SKF_AD_OFF;
            return match ad_offset {
                SKF_AD_PROTOCOL => Some(u16::from_be(self.sk_buff.protocol as u16).into()),
                SKF_AD_PKTTYPE => {
                    // TODO(https://fxbug.dev/538258894): support
                    // `SKF_AD_PKTTYPE` (special-cased here to cut down on log
                    // spam).
                    None
                }
                ad_offset => {
                    log::info!(
                        "cBPF program tried to access unimplemented SKF_AD_OFF offset: {ad_offset}",
                    );
                    None
                }
            };
        } else if offset >= SKF_NET_OFF {
            // Access network level packet.
            (offset - SKF_NET_OFF, &self.data[self.ip_offset..])
        } else if offset >= SKF_LL_OFF {
            if self.ip_offset == 0 {
                warn!("cBPF program tried to access link-level header when it's not available");
                return None;
            }
            // Access link-level packet.
            (offset - SKF_LL_OFF, self.data)
        } else {
            return None;
        };

        let offset = offset.try_into().unwrap();

        if offset >= slice.len() {
            return None;
        }

        // The packet is stored in network byte order, so multi-byte loads need to fix endianness.
        // Potentially this could be handled in the cBPF converter but then it would need to be
        // disabled from seccomp filter, which always runs in the host byte order.
        let slice = &slice[offset..];
        match width {
            DataWidth::U8 => u8::read_from_prefix(slice).ok().map(|(v, _)| v.into()),
            DataWidth::U16 => zerocopy::U16::<zerocopy::NetworkEndian>::read_from_prefix(slice)
                .ok()
                .map(|(v, _)| v.get().into()),
            DataWidth::U32 => zerocopy::U32::<zerocopy::NetworkEndian>::read_from_prefix(slice)
                .ok()
                .map(|(v, _)| v.get().into()),
            DataWidth::U64 => zerocopy::U64::<zerocopy::NetworkEndian>::read_from_prefix(slice)
                .ok()
                .map(|(v, _)| v.get().into()),
        }
    }
}

impl<'a, C> SocketRef for &'a SkBuff<'a, C> {
    fn get_socket_cookie(&self) -> Option<u64> {
        self.bpf_sock.and_then(|sock| sock.get_socket_cookie())
    }

    fn get_socket_uid(&self) -> Option<uid_t> {
        self.bpf_sock.and_then(|sock| sock.get_socket_uid())
    }
}

trait SkBuffContext {
    fn get_sk_buff_type() -> &'static Type;
}

impl<C: SkBuffContext> ProgramArgument for &'_ SkBuff<'_, C> {
    fn get_type() -> &'static Type {
        C::get_sk_buff_type()
    }

    fn field_mappings() -> &'static [FieldMapping] {
        // Field layout is the same for all flavors.
        static FIELD_MAPPINGS: [FieldMapping; 2] = [
            FieldMapping {
                source_offset: offset_of!(__sk_buff, data),
                target_offset: offset_of!(SkBuff<'_, ()>, data_ptr),
            },
            FieldMapping {
                source_offset: offset_of!(__sk_buff, data_end),
                target_offset: offset_of!(SkBuff<'_, ()>, data_end_ptr),
            },
        ];
        &FIELD_MAPPINGS
    }
}

#[derive(Default)]
pub struct EbpfRunContext<'a> {
    map_refs: Vec<MapValueRef<'a>>,
}

impl<'a> ebpf_api::MapsContext<'a> for EbpfRunContext<'a> {
    fn on_map_access(&mut self, _map: &Map) {
        // Starnix uses `on_map_access` to block suspension while executing
        // eBPF programs that access eBPF maps. This is not a concern here
        // since netstack doesn't get suspended.
    }

    fn add_value_ref(&mut self, map_ref: MapValueRef<'a>) {
        self.map_refs.push(map_ref)
    }
}

impl<'a> BpfSockContext for EbpfRunContext<'a> {
    type BpfSockRef = &'a BpfSock;
}

/// An eBPF programs of type `BPF_PROG_TYPE_SOCKET_FILTER`. These programs can
/// be attached to socket or used as matchers in filters.
#[derive(Debug)]
pub(crate) struct SocketFilterProgram {
    program: EbpfProgram<SocketFilterProgram>,
}

impl BpfProgramContext for SocketFilterProgram {
    type RunContext<'a> = EbpfRunContext<'a>;
    type Packet<'a> = &'a SocketFilterSkBuff<'a>;
    type Map = CachedMapRef;
    const CBPF_CONFIG: &'static CbpfConfig = &SOCKET_FILTER_CBPF_CONFIG;
}

ebpf_api::ebpf_program_context_type!(SocketFilterProgram, ebpf_api::SocketFilterProgramContext);

pub type SocketFilterSkBuff<'a> = SkBuff<'a, SocketFilterProgram>;

impl SkBuffContext for SocketFilterProgram {
    fn get_sk_buff_type() -> &'static Type {
        &SOCKET_FILTER_SK_BUF_TYPE
    }
}

pub(crate) enum SocketFilterResult {
    // If the packet is accepted it should be trimmed to the specified size
    // when the filter is attached to a socket. The value is ignored when
    // the filter is used as a filter matcher.
    Accept(usize),
    Reject,
}

impl SocketFilterProgram {
    pub(crate) fn new(
        program: ValidVerifiedProgram,
        map_cache: &Arc<EbpfMapCache>,
    ) -> Result<Self, EbpfError> {
        // TODO(https://fxbug.dev/370043219): Currently we assume that the code has been verified.
        // This is safe because fuchsia.posix.socket.packet is routed only to Starnix,
        // but that may change in the future. We need a better mechanism for permissions & BPF
        // verification.
        let (program, maps) =
            parse_verified_program_fidl(program, map_cache, SOCKET_FILTER_ARGS.clone())?;

        let program = ebpf::link_program(&program, maps).map_err(|e| {
            error!("Failed to link eBPF program: {:?}", e);
            EbpfError::LinkFailed
        })?;

        Ok(Self { program })
    }

    pub(crate) fn run(&self, mut skb: SocketFilterSkBuff<'_>) -> SocketFilterResult {
        trace_duration!(
            c"ebpf::socket_filter::run",
            "len" => skb.sk_buff.len,
            "protocol" => skb.sk_buff.protocol
        );

        let mut context = EbpfRunContext::default();
        let result = self.program.run(&mut context, &mut skb);
        match result {
            0 => SocketFilterResult::Reject,
            n => SocketFilterResult::Accept(n.try_into().unwrap()),
        }
    }
}

impl diagnostics_traits::InspectableValue for SocketFilterProgram {
    fn record<I: diagnostics_traits::Inspector>(&self, _name: &str, _inspector: &mut I) {
        // TODO(https://fxbug.dev/467448866): Record program name.
    }
}

impl<D> BindingsPacketMatcher<D> for SocketFilterProgram
where
    D: DeviceIfIndex,
{
    fn matches<I: FilterIpExt, P: FilterIpPacket<I>>(
        &self,
        packet: &P,
        interfaces: Interfaces<'_, D>,
        packet_metadata: &impl FilterPacketMetadata,
    ) -> bool {
        trace_duration!(c"ebpf::packet_matcher");

        let marks = packet_metadata.marks();
        let socket_info = packet_metadata.socket_info();
        let bpf_sock = socket_info.and_then(|info| BpfSock::new(info, &marks));

        // `ifindex` field is set to either ingress or ingress interface index
        // depending on the context. When executing forwarding hooks we have
        // both ingress and egress interface. In this case `ifindex` is set to
        // the ingress interface index.
        let ifindex =
            interfaces.ingress.or(interfaces.egress).map(|d| d.get_ifindex()).unwrap_or(0);

        let mut data_buffer = SmallVec::new();
        let sk_buff = SkBuff::<'_, SocketFilterProgram>::from_ip_packet(
            packet,
            ifindex,
            &marks,
            &mut data_buffer,
            bpf_sock.as_ref(),
        );

        match self.run(sk_buff) {
            SocketFilterResult::Accept(_) => true,
            SocketFilterResult::Reject => false,
        }
    }
}

/// An eBPF programs of type `BPF_PROG_TYPE_CGROUP_SKB`, attachment type ether
/// `CGROUP_EGRESS` or `CGROUP_INGRESS`.
#[derive(Debug)]
pub(crate) struct CgroupSkbProgram {
    program: EbpfProgram<CgroupSkbProgram>,
}

type CgroupSkBuff<'a> = SkBuff<'a, CgroupSkbProgram>;

impl SkBuffContext for CgroupSkbProgram {
    fn get_sk_buff_type() -> &'static Type {
        &CGROUP_SKB_SK_BUF_TYPE
    }
}

impl EbpfProgramContext for CgroupSkbProgram {
    type RunContext<'a> = EbpfRunContext<'a>;
    type Packet<'a> = ();
    type Map = CachedMapRef;

    type Arg1<'a> = &'a CgroupSkBuff<'a>;
    type Arg2<'a> = ();
    type Arg3<'a> = ();
    type Arg4<'a> = ();
    type Arg5<'a> = ();
}

ebpf_api::ebpf_program_context_type!(CgroupSkbProgram, ebpf_api::CgroupSkbProgramContext);

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) enum EbpfError {
    LinkFailed,
    MapFailed,
}

impl From<EbpfError> for fposix::Errno {
    fn from(e: EbpfError) -> Self {
        match e {
            EbpfError::LinkFailed => fposix::Errno::Eio,
            EbpfError::MapFailed => fposix::Errno::Eio,
        }
    }
}

impl From<EbpfError> for fnet_filter::SocketControlAttachEbpfProgramError {
    fn from(e: EbpfError) -> Self {
        match e {
            EbpfError::LinkFailed => fnet_filter::SocketControlAttachEbpfProgramError::LinkFailed,
            EbpfError::MapFailed => fnet_filter::SocketControlAttachEbpfProgramError::MapFailed,
        }
    }
}

impl From<EbpfError> for fnet_filter::RegisterEbpfProgramError {
    fn from(e: EbpfError) -> Self {
        match e {
            EbpfError::LinkFailed => fnet_filter::RegisterEbpfProgramError::LinkFailed,
            EbpfError::MapFailed => fnet_filter::RegisterEbpfProgramError::MapFailed,
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) enum AttachmentError {
    DuplicateAttachment,
}

impl From<AttachmentError> for fnet_filter::SocketControlAttachEbpfProgramError {
    fn from(e: AttachmentError) -> Self {
        match e {
            AttachmentError::DuplicateAttachment => {
                fnet_filter::SocketControlAttachEbpfProgramError::DuplicateAttachment
            }
        }
    }
}

#[derive(ValidFidlTable)]
#[fidl_table_src(febpf::VerifiedProgram)]
#[fidl_table_strict]
pub(crate) struct ValidVerifiedProgram {
    pub code: Vec<u64>,
    pub struct_access_instructions: Vec<febpf::StructAccess>,
    pub maps: Vec<febpf::Map>,
}

/// Translate FIDL representation of a verified eBPF program to
/// `VerifiedEbpfProgram`. Initializes all included eBPF maps and adds them
/// to the `map_cache`.
fn parse_verified_program_fidl(
    program: ValidVerifiedProgram,
    map_cache: &Arc<EbpfMapCache>,
    args: Vec<Type>,
) -> Result<(VerifiedEbpfProgram, Vec<CachedMapRef>), EbpfError> {
    let ValidVerifiedProgram { code, struct_access_instructions, maps } = program;

    let maps = map_cache.init_maps(maps).map_err(|e| {
        error!("Failed to initialize eBPF map: {:?}", e);
        EbpfError::MapFailed
    })?;
    let map_schemas = maps.iter().map(|m| m.schema().clone()).collect();

    let struct_access_instructions = struct_access_instructions
        .iter()
        .map(|value| ebpf::StructAccess {
            pc: value.pc.try_into().unwrap(),
            memory_id: StructId::from(value.struct_id).as_memory_id(),
            field_offset: value.field_offset.try_into().unwrap(),
            is_32_bit_ptr_load: value.is_32_bit_ptr_load,
        })
        .collect();

    let program = VerifiedEbpfProgram::from_verified_code(
        code_from_vec(code),
        args,
        struct_access_instructions,
        map_schemas,
    );

    Ok((program, maps))
}

impl CgroupSkbProgram {
    // Both `CGROUP_EGRESS` and `CGROUP_INGRESS` returns result where the first bit indicates if
    // the packet should be passed or dropped.
    const RESULT_PASS_BIT: u64 = 1;

    // `CGROUP_EGRESS` uses second bit of the result to signal congestion.
    const RESULT_CONGESTION_BIT: u64 = 2;

    // Max value that can be returned from a `CGROUP_EGRESS` program.
    const EGRESS_MAX_RESULT: u64 = Self::RESULT_PASS_BIT | Self::RESULT_CONGESTION_BIT;

    // Max value that can be returned from a `CGROUP_INGRESS` program.
    const INGRESS_MAX_RESULT: u64 = Self::RESULT_PASS_BIT;

    pub fn new(
        program: ValidVerifiedProgram,
        map_cache: &Arc<EbpfMapCache>,
    ) -> Result<Self, EbpfError> {
        // TODO(https://fxbug.dev/370043219): Currently we assume that the code has been verified.
        // This is safe because `fuchsia.posix.filter.SocketControl` is routed only to Starnix,
        // but that may change in the future. We need a better mechanism for permissions & BPF
        // verification.
        let (program, maps) =
            parse_verified_program_fidl(program, map_cache, CGROUP_SKB_ARGS.clone())?;

        let program = ebpf::link_program(&program, maps).map_err(|e| {
            error!("Failed to link eBPF program: {:?}", e);
            EbpfError::LinkFailed
        })?;

        Ok(Self { program })
    }

    fn run(&self, mut sk_buff: CgroupSkBuff<'_>) -> u64 {
        trace_duration!(
            c"ebpf::cgroup_skb::run",
            "len" => sk_buff.sk_buff.len,
            "protocol" => sk_buff.sk_buff.protocol
        );

        let mut run_context = EbpfRunContext::default();
        self.program.run_with_1_argument(&mut run_context, &mut sk_buff)
    }
}

type MapId = zx::Koid;

#[derive(Clone)]
pub(crate) struct CachedMapRefInner {
    map: PinnedMap,
    id: MapId,
    cache: Weak<EbpfMapCache>,
}

impl Drop for CachedMapRefInner {
    fn drop(&mut self) {
        // If this is the last reference to the map beside the reference owned
        // by the cache itself, then remove it from the cache.
        if let Some(cache) = self.cache.upgrade() {
            cache.last_reference_dropped(self.id)
        }
    }
}

/// A reference to a map stored in `EbpfMapCache`. The referenced map is
/// deleted from the cache when the last reference to that map is dropped.
pub struct CachedMapRef {
    inner: Arc<CachedMapRefInner>,
}

impl MapReference for CachedMapRef {
    fn schema(&self) -> &MapSchema {
        self.inner.map.schema()
    }

    fn as_bpf_value(&self) -> BpfValue {
        self.inner.map.as_bpf_value()
    }

    fn get_data_ptr(&self) -> Option<BpfValue> {
        self.inner.map.get_data_ptr()
    }
}

/// `EbpfMapCache` maintains list of all eBPF maps programs loaded in this
/// process. This allows to initialize to memory-map the corresponding VMOs
/// only once per process.
#[derive(Default)]
pub(crate) struct EbpfMapCache {
    maps: Mutex<HashMap<MapId, Weak<CachedMapRefInner>>>,
}

impl EbpfMapCache {
    fn init_map(self: &Arc<Self>, fidl_map: febpf::Map) -> Result<CachedMapRef, MapError> {
        // Maps are identified by the KOID of the underlying VMO.
        let id = fidl_map
            .vmo
            .as_ref()
            .ok_or(MapError::InvalidParam)?
            .koid()
            .map_err(|_: zx::Status| MapError::InvalidVmo)?;

        let mut maps = self.maps.lock();
        let entry = maps.entry(id);
        let inner = match &entry {
            hash_map::Entry::Occupied(occupied) => {
                // The upgraded `Arc` may be `None` if the map is being
                // dropped concurrently by another thread. In that case it
                // will be initialized again below.
                occupied.get().upgrade()
            }

            hash_map::Entry::Vacant(_) => None,
        };

        let inner = match inner {
            Some(inner) => inner,
            None => {
                let map = Map::new_shared(fidl_map)?;
                let inner = Arc::new(CachedMapRefInner { map, id, cache: Arc::downgrade(self) });
                let _: hash_map::OccupiedEntry<'_, _, _> =
                    entry.insert_entry(Arc::downgrade(&inner));
                inner
            }
        };

        Ok(CachedMapRef { inner })
    }

    fn init_maps(
        self: &Arc<Self>,
        fidl_maps: Vec<febpf::Map>,
    ) -> Result<Vec<CachedMapRef>, MapError> {
        let mut result = Vec::with_capacity(fidl_maps.len());
        for map in fidl_maps {
            result.push(self.init_map(map)?);
        }
        Ok(result)
    }

    fn last_reference_dropped(&self, id: MapId) {
        match self.maps.lock().entry(id) {
            hash_map::Entry::Occupied(occupied) => {
                // Remove the entry only if it's no longer valid since the
                // entry might have been replaced in `init_map()`.
                if occupied.get().upgrade().is_none() {
                    let _: Weak<CachedMapRefInner> = occupied.remove();
                }
            }
            hash_map::Entry::Vacant(_) => {
                // Nothing to do since the entry is not in the cache. This case
                // may be reached when the map is inserted and deleted
                // concurrently by another thread.
            }
        }
    }
}

#[derive(Default)]
struct EbpfManagerState {
    root_cgroup_egress: Option<CgroupSkbProgram>,
    root_cgroup_ingress: Option<CgroupSkbProgram>,
}

/// Holds state of eBPF programs attached in this process.
#[derive(Default)]
pub(crate) struct EbpfManager {
    state: RwLock<EbpfManagerState>,
    maps_cache: Arc<EbpfMapCache>,
}

impl EbpfManager {
    pub fn maps_cache(&self) -> &Arc<EbpfMapCache> {
        &self.maps_cache
    }

    pub fn set_egress_hook(
        &self,
        program: Option<CgroupSkbProgram>,
        allow_replace: bool,
    ) -> Result<(), AttachmentError> {
        let mut state = self.state.write();
        if !allow_replace && state.root_cgroup_egress.is_some() && program.is_some() {
            return Err(AttachmentError::DuplicateAttachment);
        }
        state.root_cgroup_egress = program;
        Ok(())
    }

    pub fn set_ingress_hook(
        &self,
        program: Option<CgroupSkbProgram>,
        allow_replace: bool,
    ) -> Result<(), AttachmentError> {
        let mut state = self.state.write();
        if !allow_replace && state.root_cgroup_ingress.is_some() && program.is_some() {
            return Err(AttachmentError::DuplicateAttachment);
        }
        state.root_cgroup_ingress = program;
        Ok(())
    }
}

pub(crate) trait DeviceIfIndex {
    fn get_ifindex(&self) -> u32;
}

impl DeviceIfIndex for DeviceId<BindingsCtx> {
    fn get_ifindex(&self) -> u32 {
        self.bindings_id().id.get().try_into().unwrap_or(0)
    }
}

// Size of the buffer used for stack buffer when serializing packet headers for
// egress and ingress filters. 128 bytes is enough to fit headers for most
// packets.
const PACKET_BUF_STACK_SIZE: usize = 128;

impl<D: DeviceIfIndex> SocketOpsFilter<D> for &EbpfManager {
    fn on_egress<I: FilterIpExt, P: FilterIpPacket<I>>(
        &self,
        packet: &P,
        device: &D,
        socket_info: SocketInfo,
        marks: &Marks,
    ) -> SocketEgressFilterResult {
        let state = self.state.read();
        let Some(prog) = state.root_cgroup_egress.as_ref() else {
            return SocketEgressFilterResult::Pass { congestion: false };
        };

        trace_duration!("ebpf::egress");

        let bpf_sock = BpfSock::new(socket_info, marks);
        let mut data_buffer = SmallVec::new();
        let sk_buff = SkBuff::from_ip_packet(
            packet,
            device.get_ifindex(),
            marks,
            &mut data_buffer,
            bpf_sock.as_ref(),
        );

        let result = prog.run(sk_buff);
        if result > CgroupSkbProgram::EGRESS_MAX_RESULT {
            // TODO(https://fxbug.dev/413490751): Change this to panic once
            // result validation is implemented in the verifier.
            error!("eBPF program returned invalid result: {}", result);
            return SocketEgressFilterResult::Pass { congestion: false };
        }

        let congestion = result & CgroupSkbProgram::RESULT_CONGESTION_BIT > 0;
        if result & CgroupSkbProgram::RESULT_PASS_BIT > 0 {
            SocketEgressFilterResult::Pass { congestion }
        } else {
            SocketEgressFilterResult::Drop { congestion }
        }
    }

    fn on_ingress(
        &self,
        ip_version: IpVersion,
        packet: FragmentedByteSlice<'_, &[u8]>,
        header_len: usize,
        device: &D,
        socket_info: SocketInfo,
        marks: &Marks,
    ) -> SocketIngressFilterResult {
        let state = self.state.read();
        let Some(prog) = state.root_cgroup_ingress.as_ref() else {
            return SocketIngressFilterResult::Accept;
        };

        trace_duration!("ebpf::ingress");

        let bpf_sock = BpfSock::new(socket_info, marks);
        let ethertype = EtherType::from_ip_version(ip_version);
        let mark = get_linux_packet_mark(marks);

        // TODO(https://fxbug.dev/424212358): Implement lazy packet serialization.
        let mut data = SmallVec::<[u8; PACKET_BUF_STACK_SIZE]>::with_capacity(header_len);

        // Copy `header_len` bytes from the `packet`.
        for fragment in packet.iter_fragments() {
            let bytes_to_copy = std::cmp::min(fragment.len(), header_len - data.len());
            data.extend_from_slice(&fragment[..bytes_to_copy]);
            if data.len() >= header_len {
                break;
            }
        }

        let skb = SkBuff::new(
            Some(ethertype.into()),
            packet.len(),
            device.get_ifindex(),
            mark,
            &data[..],
            /*ip_offset=*/ 0,
            /*default_offset=*/ 0,
            bpf_sock.as_ref(),
        );

        let result = prog.run(skb);

        if result > CgroupSkbProgram::INGRESS_MAX_RESULT {
            // TODO(https://fxbug.dev/413490751): Change this to panic once
            // result validation is implemented in the verifier.
            error!("eBPF program returned invalid result: {}", result);
            return SocketIngressFilterResult::Accept;
        }

        if result & CgroupSkbProgram::RESULT_PASS_BIT > 0 {
            SocketIngressFilterResult::Accept
        } else {
            SocketIngressFilterResult::Drop
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ebpf::{MapFlags, Packet};
    use ebpf_api::{AttachType, ProgramType, SKF_AD_MAX};
    use fidl_fuchsia_posix_socket_packet as fppacket;
    use ip_test_macro::ip_test;
    use net_types::Witness;
    use netstack3_core::NetworkSerializationContext;
    use netstack3_core::ip::Mark;
    use netstack3_core::socket::SocketCookie;
    use netstack3_core::sync::ResourceTokenValue;
    use netstack3_core::testutil::{self, FakeDeviceId, TestIpExt};
    use packet::{InnerPacketBuilder, NestablePacketBuilder, PacketConstraints, Serializer};
    use packet_formats::ip::{IpPacketBuilder, IpProto};
    use packet_formats::udp::UdpPacketBuilder;
    use std::num::NonZeroU16;
    use test_case::test_case;

    struct TestData;
    impl TestData {
        const PROTO: u16 = 0x08AB;
        const BUFFER: &'static [u8] = &[
            0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, // Dest MAC
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, // Source MAC
            0x08, 0xAB, // EtherType
            0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x3A, 0x4B, // Packet body
        ];
        const BODY_POSITION: usize = 14;

        /// Creates an SkBuff with the values specified above.
        fn packet(kind: fppacket::Kind) -> SocketFilterSkBuff<'static> {
            let default_offset = match kind {
                fppacket::Kind::Link => 0,
                fppacket::Kind::Network => Self::BODY_POSITION,
            };

            SkBuff::new(
                Some(Self::PROTO),
                Self::BUFFER.len(),
                /*ifindex=*/ 2,
                /*mark=*/ 0,
                Self::BUFFER,
                Self::BODY_POSITION,
                default_offset,
                None,
            )
        }
    }

    fn packet_load(packet: &SocketFilterSkBuff<'_>, offset: i32, width: DataWidth) -> Option<u64> {
        packet.load(offset, width).map(|v| v.as_u64())
    }

    // Test loading Ethernet header at the specified base offset.
    fn test_ll_header_load(packet: &SocketFilterSkBuff<'_>, base: i32) {
        assert_eq!(packet_load(packet, base, DataWidth::U8), Some(0x06));
        assert_eq!(packet_load(packet, base, DataWidth::U16), Some(0x0607));
        assert_eq!(packet_load(packet, base, DataWidth::U32), Some(0x06070809));
        assert_eq!(packet_load(packet, base, DataWidth::U64), Some(0x060708090A0B0001));

        // Loads past the Ethernet header load the packet body.
        assert_eq!(packet_load(packet, base + 8, DataWidth::U8), Some(0x02));
        assert_eq!(packet_load(packet, base + 8, DataWidth::U16), Some(0x0203));
        assert_eq!(packet_load(packet, base + 8, DataWidth::U32), Some(0x02030405));
        assert_eq!(packet_load(packet, base + 8, DataWidth::U64), Some(0x0203040508AB2122));
    }

    // Test loading packet body at the specified base offset.
    fn test_packet_body_load(packet: &SocketFilterSkBuff<'_>, base: i32) {
        assert_eq!(packet_load(packet, base, DataWidth::U64), Some(0x212223242526273A));
        assert_eq!(packet_load(packet, base, DataWidth::U8), Some(0x21));
        assert_eq!(packet_load(packet, base, DataWidth::U16), Some(0x2122));
        assert_eq!(packet_load(packet, base, DataWidth::U32), Some(0x21222324));
        assert_eq!(packet_load(packet, base, DataWidth::U64), Some(0x212223242526273A));

        assert_eq!(packet_load(packet, base + 6, DataWidth::U8), Some(0x27));
        assert_eq!(packet_load(packet, base + 6, DataWidth::U16), Some(0x273A));
        assert_eq!(packet_load(packet, base + 6, DataWidth::U32), None);
        assert_eq!(packet_load(packet, base + 6, DataWidth::U64), None);

        assert_eq!(packet_load(packet, base + 9, DataWidth::U8), None);
        assert_eq!(packet_load(packet, base + 9, DataWidth::U16), None);
        assert_eq!(packet_load(packet, base + 9, DataWidth::U32), None);
        assert_eq!(packet_load(packet, base + 9, DataWidth::U64), None);
    }

    #[test]
    fn network_level_packet() {
        let packet = TestData::packet(fppacket::Kind::Network);

        test_packet_body_load(&packet, 0);

        assert_eq!(packet_load(&packet, i32::MAX, DataWidth::U8), None);
        assert_eq!(packet_load(&packet, i32::MAX, DataWidth::U16), None);
        assert_eq!(packet_load(&packet, i32::MAX, DataWidth::U32), None);
        assert_eq!(packet_load(&packet, i32::MAX, DataWidth::U64), None);
    }

    #[test]
    fn link_level_packet() {
        let packet = TestData::packet(fppacket::Kind::Link);

        test_ll_header_load(&packet, 0);
        test_packet_body_load(&packet, TestData::BODY_POSITION.try_into().unwrap());
    }

    #[test]
    fn negative_offsets() {
        let packet = TestData::packet(fppacket::Kind::Link);
        // Loads from SKF_AD_OFF + SKF_AD_PROTOCOL load EtherType, ignoring data width.
        assert_eq!(
            packet_load(&packet, SKF_AD_OFF + SKF_AD_PROTOCOL, DataWidth::U8),
            Some(TestData::PROTO as u64)
        );
        assert_eq!(
            packet_load(&packet, SKF_AD_OFF + SKF_AD_PROTOCOL, DataWidth::U16),
            Some(TestData::PROTO as u64)
        );
        assert_eq!(
            packet_load(&packet, SKF_AD_OFF + SKF_AD_PROTOCOL, DataWidth::U32),
            Some(TestData::PROTO as u64)
        );

        // SKF_AD_MAX is the max offset that can be used with SKF_AD_OFF.
        assert_eq!(packet_load(&packet, SKF_AD_OFF + SKF_AD_MAX, DataWidth::U16), None);
        assert_eq!(packet_load(&packet, SKF_AD_OFF + SKF_AD_MAX + 1, DataWidth::U16), None);

        // SKF_LL_OFF can be used to load the packet starting from the LL header.
        test_ll_header_load(&packet, SKF_LL_OFF);
        test_packet_body_load(
            &packet,
            SKF_LL_OFF + i32::try_from(TestData::BODY_POSITION).unwrap(),
        );

        // Loads with `offset = SKF_NET_OFF+n` load the packet starting from the
        // packet body (Network-level header).
        test_packet_body_load(&packet, SKF_NET_OFF);

        // Loads below `SKF_LL_OFF` should always fail.
        assert_eq!(packet_load(&packet, SKF_LL_OFF - 1, DataWidth::U16), None);
        assert_eq!(packet_load(&packet, SKF_LL_OFF - 8, DataWidth::U16), None);
        assert_eq!(packet_load(&packet, i32::MIN, DataWidth::U16), None);
    }

    #[test]
    fn maps_cache() {
        let schema = MapSchema {
            map_type: ebpf_api::BPF_MAP_TYPE_HASH,
            key_size: 1,
            value_size: 2,
            max_entries: 10,
            flags: MapFlags::empty(),
        };

        let cache = Arc::new(EbpfMapCache::default());

        let num_cached = || cache.maps.lock().len();
        assert_eq!(num_cached(), 0);

        // Create a map and insert it to the cache.
        let map1 = Map::new(schema, "test").unwrap();
        let fidl_map = map1.share().unwrap();
        let cache_ref1 = cache.init_map(fidl_map).unwrap();

        let num_cached = || cache.maps.lock().len();
        assert_eq!(num_cached(), 1);

        // Import second map.
        let map2 = Map::new(schema, "test").unwrap();
        let fidl_map = map2.share().unwrap();
        let cache_ref2 = cache.init_map(fidl_map).unwrap();

        let num_cached = || cache.maps.lock().len();
        assert_eq!(num_cached(), 2);

        // Import the first map again. The cached entry should be reused.
        let fidl_map = map1.share().unwrap();
        let cache_ref1_dup = cache.init_map(fidl_map).unwrap();
        assert_eq!(num_cached(), 2);

        // Map should be imported only once, so `as_bpf_value()` will return
        // the same value for both refs.
        assert_eq!(cache_ref1.as_bpf_value().as_u64(), cache_ref1_dup.as_bpf_value().as_u64());

        // But the `BpfValue` is different when the maps are different.
        assert_ne!(cache_ref1.as_bpf_value().as_u64(), cache_ref2.as_bpf_value().as_u64());

        // Maps are removed from the cache when the references are dropped.
        std::mem::drop(cache_ref2);
        assert_eq!(num_cached(), 1);

        std::mem::drop(cache_ref1);
        assert_eq!(num_cached(), 1);

        std::mem::drop(cache_ref1_dup);
        assert_eq!(num_cached(), 0);
    }

    const TEST_IFINDEX: u32 = 2;

    impl DeviceIfIndex for FakeDeviceId {
        fn get_ifindex(&self) -> u32 {
            TEST_IFINDEX
        }
    }

    #[ip_test(I)]
    #[test_case(AttachType::CgroupInetEgress; "Egress")]
    #[test_case(AttachType::CgroupInetIngress; "Ingress")]
    fn run_skb_prog<I: TestIpExt + FilterIpExt>(attach_type: AttachType) {
        let manager = EbpfManager::default();
        let test_program =
            ebpf_test_util::TestProgramDefinition::load(ProgramType::CgroupSkb).instantiate();
        let program = test_program.get_fidl_program();
        let program = CgroupSkbProgram::new(program.try_into().unwrap(), manager.maps_cache())
            .expect("Failed to initialize a program");

        match attach_type {
            AttachType::CgroupInetEgress => {
                manager.set_egress_hook(Some(program), false).expect("Failed to set egress hook")
            }
            AttachType::CgroupInetIngress => {
                manager.set_ingress_hook(Some(program), false).expect("Failed to set ingress hook")
            }
            attach_type => unreachable!("Unexpected attach_type: {:?}", attach_type),
        }

        const SRC_PORT: NonZeroU16 = NonZeroU16::new(1234).unwrap();
        const DST_PORT: NonZeroU16 = NonZeroU16::new(5678).unwrap();

        let data = b"PACKET";
        let mut udp_packet = UdpPacketBuilder::new(
            I::TEST_ADDRS.local_ip.get(),
            I::TEST_ADDRS.remote_ip.get(),
            Some(SRC_PORT),
            DST_PORT,
        )
        .wrap_body(data.into_serializer());

        const UID: u32 = 231;

        let socket_resource_token = ResourceTokenValue::default();
        let socket_cookie = SocketCookie::new(socket_resource_token.token());
        let socket_info = SocketInfo {
            proto: I::map_ip(
                (),
                |()| EitherIpProto::V4(Ipv4Proto::Proto(IpProto::Udp)),
                |()| EitherIpProto::V6(Ipv6Proto::Proto(IpProto::Udp)),
            ),
            cookie: socket_cookie,
        };

        let mut marks = Marks::default();
        *marks.get_mut(fnet::MARK_DOMAIN_SOCKET_UID.into_core()) = Mark(Some(UID));

        let device = FakeDeviceId;

        match attach_type {
            AttachType::CgroupInetEgress => {
                let packet = testutil::new_filter_egress_ip_packet::<I, _>(
                    I::TEST_ADDRS.local_ip.get(),
                    I::TEST_ADDRS.remote_ip.get(),
                    IpProto::Udp.into(),
                    &mut udp_packet,
                );
                let result = (&manager).on_egress(&packet, &device, socket_info, &marks);
                assert!(result == SocketEgressFilterResult::Pass { congestion: false });
            }
            AttachType::CgroupInetIngress => {
                let ip_packet = I::PacketBuilder::new(
                    I::TEST_ADDRS.remote_ip.get(),
                    I::TEST_ADDRS.local_ip.get(),
                    0,
                    IpProto::Udp.into(),
                )
                .wrap_body(udp_packet);
                let serialized = ip_packet
                    .serialize_new_buf(
                        &mut NetworkSerializationContext::default(),
                        PacketConstraints::UNCONSTRAINED,
                        packet::new_buf_vec,
                    )
                    .expect("Failed to serialize test packet")
                    .into_inner();
                let mut parts = [&serialized[..]];

                let result = (&manager).on_ingress(
                    I::VERSION,
                    FragmentedByteSlice::new(&mut parts),
                    serialized.len() - data.len(),
                    &device,
                    socket_info,
                    &marks,
                );

                assert!(result == SocketIngressFilterResult::Accept);
            }
            attach_type => unreachable!("Unexpected attach_type: {:?}", attach_type),
        }

        // Check the result.
        let result = test_program.read_test_result();
        assert_eq!(result.cookie, socket_resource_token.token().export_value());
        assert_eq!(result.uid, UID);
        assert_eq!(result.ifindex, TEST_IFINDEX);
        assert_eq!(result.ether_type, u32::from(u16::from(I::ETHER_TYPE)));
        assert_eq!(result.ip_proto, u8::from(IpProto::Udp));
    }
}
