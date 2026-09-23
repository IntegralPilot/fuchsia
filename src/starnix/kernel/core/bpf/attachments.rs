// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://github.com/rust-lang/rust/issues/39371): remove
#![allow(non_upper_case_globals)]

use crate::bpf::context::EbpfRunContextImpl;
use crate::bpf::fs::{BpfHandle, get_bpf_object};
use crate::bpf::program::ProgramHandle;
use crate::mm::PAGE_SIZE;
use crate::security;
use crate::task::CurrentTask;
use crate::vfs::FdNumber;
use crate::vfs::socket::{
    SockOptValue, Socket, SocketBpfState, SocketDomain, SocketProtocol, SocketType,
    ZxioBackedSocket,
};
use ebpf::{BpfValue, EbpfProgram, EbpfProgramContext, EbpfPtr, ProgramArgument, Type};
use ebpf_api::{
    AttachType, BPF_SOCK_ADDR_TYPE, BPF_SOCK_TYPE, BpfSockContext, CgroupSockAddrProgramContext,
    CgroupSockOptProgramContext, CgroupSockProgramContext, CurrentTaskContext, Map, MapValueRef,
    MapsContext, PinnedMap, ProgramType, ReturnValueContext, SocketRef,
};
use fidl_fuchsia_net_filter as fnet_filter;
use fuchsia_component::client::connect_to_protocol_sync;
use linux_uapi::{bpf_sockopt, uaddr};
use starnix_logging::{log_error, log_warn, track_stub};
use starnix_sync::{EbpfStateLock, LockDepRwLock};
use starnix_syscalls::{SUCCESS, SyscallResult};
use starnix_uapi::auth::{CAP_NET_ADMIN, CAP_SYS_ADMIN, Capabilities};
use starnix_uapi::errors::{Errno, ErrnoCode, is_error_return_value};
use starnix_uapi::{
    CGROUP2_SUPER_MAGIC, bpf_attr__bindgen_ty_6, bpf_sock, bpf_sock_addr, errno, error, gid_t,
    pid_t, uid_t,
};
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, OnceLock};
use zerocopy::FromBytes;

pub type BpfAttachAttr = bpf_attr__bindgen_ty_6;

fn check_root_cgroup_fd(current_task: &CurrentTask, cgroup_fd: FdNumber) -> Result<(), Errno> {
    let file = current_task.files().get(cgroup_fd)?;

    // Check that `cgroup_fd` is from the CGROUP2 file system.
    let is_cgroup = file.node().fs().statfs(current_task)?.f_type == CGROUP2_SUPER_MAGIC as i64;
    if !is_cgroup {
        log_warn!("bpf_prog_attach(BPF_PROG_ATTACH) is called with an invalid cgroup2 FD.");
        return error!(EINVAL);
    }

    // Currently cgroup attachments are supported only for the root cgroup.
    // TODO(https://fxbug.dev//388077431) Allow attachments to any cgroup once cgroup
    // hierarchy is moved to starnix_core.
    let is_root = file
        .node()
        .fs()
        .maybe_root()
        .map(|root| Arc::ptr_eq(&root.node, file.node()))
        .unwrap_or(false);
    if !is_root {
        log_warn!("bpf_prog_attach(BPF_PROG_ATTACH) is supported only for root cgroup.");
        return error!(EINVAL);
    }

    Ok(())
}

pub fn bpf_prog_attach(
    current_task: &CurrentTask,
    attr: BpfAttachAttr,
) -> Result<SyscallResult, Errno> {
    // SAFETY: reading i32 field from a union is always safe.
    let bpf_fd = FdNumber::from_raw(attr.attach_bpf_fd as i32);
    let object = get_bpf_object(current_task, bpf_fd)?;
    if matches!(object, BpfHandle::ProgramStub(_)) {
        log_warn!("Stub program. Faking successful attach");
        return Ok(SUCCESS);
    }
    let program = object.as_program()?.clone();

    if !security::is_task_capable_noaudit(current_task, CAP_SYS_ADMIN) {
        let required_caps = get_capability_for_program(program.info.program_type)?;
        security::check_task_capable(current_task, required_caps)?;
    }

    let attach_type = AttachType::from(attr.attach_type);
    let program_type = program.info.program_type;
    if attach_type.get_program_type() != program_type {
        log_warn!(
            "bpf_prog_attach(BPF_PROG_ATTACH): program not compatible with attach_type \
                   attach_type: {attach_type:?}, program_type: {program_type:?}"
        );
        return error!(EINVAL);
    }

    if !attach_type.is_compatible_with_expected_attach_type(program.info.expected_attach_type) {
        log_warn!(
            "bpf_prog_attach(BPF_PROG_ATTACH): expected_attach_type didn't match attach_type \
                   expected_attach_type: {:?}, attach_type: {:?}",
            program.info.expected_attach_type,
            attach_type
        );
        return error!(EINVAL);
    }

    // SAFETY: reading i32 field from a union is always safe.
    let target_fd = unsafe { attr.__bindgen_anon_1.target_fd };
    let target_fd = FdNumber::from_raw(target_fd as i32);

    current_task.kernel().ebpf_state.attachments.attach_prog(
        current_task,
        attach_type,
        target_fd,
        program,
    )
}

pub fn bpf_prog_detach(
    current_task: &CurrentTask,
    attr: BpfAttachAttr,
) -> Result<SyscallResult, Errno> {
    let attach_type = AttachType::from(attr.attach_type);

    // SAFETY: reading i32 field from a union is always safe.
    let target_fd = unsafe { attr.__bindgen_anon_1.target_fd };
    let target_fd = FdNumber::from_raw(target_fd as i32);

    current_task.kernel().ebpf_state.attachments.detach_prog(current_task, attach_type, target_fd)
}

// Wrapper for `bpf_sock_addr` used to implement `ProgramArgument` trait.
#[repr(C)]
pub struct BpfSockAddr<'a> {
    sock_addr: bpf_sock_addr,

    bpf_sock: &'a BpfSock<'a>,
}

impl<'a> Deref for BpfSockAddr<'a> {
    type Target = bpf_sock_addr;
    fn deref(&self) -> &Self::Target {
        &self.sock_addr
    }
}

impl<'a> DerefMut for BpfSockAddr<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sock_addr
    }
}

impl<'a> ProgramArgument for &'_ mut BpfSockAddr<'a> {
    fn get_type() -> &'static Type {
        &*BPF_SOCK_ADDR_TYPE
    }
}

impl<'a, 'b> SocketRef for &'a mut BpfSockAddr<'a> {
    fn get_socket_cookie(&self) -> Option<u64> {
        self.bpf_sock.get_socket_cookie()
    }

    fn get_socket_uid(&self) -> Option<uid_t> {
        self.bpf_sock.get_socket_uid()
    }
}

// Context for eBPF programs of type BPF_PROG_TYPE_CGROUP_SOCKADDR.
struct SockAddrProgram(EbpfProgram<SockAddrProgram>);

impl EbpfProgramContext for SockAddrProgram {
    type RunContext<'a> = EbpfRunContextImpl<'a>;
    type Packet<'a> = ();
    type Arg1<'a> = &'a mut BpfSockAddr<'a>;
    type Arg2<'a> = ();
    type Arg3<'a> = ();
    type Arg4<'a> = ();
    type Arg5<'a> = ();

    type Map = PinnedMap;
}

ebpf_api::ebpf_program_context_type!(SockAddrProgram, CgroupSockAddrProgramContext);

#[derive(Debug, PartialEq, Eq)]
pub enum SockAddrProgramResult {
    Allow,
    Block,
}

impl SockAddrProgram {
    fn run<'a>(
        &self,
        current_task: &'a CurrentTask,
        addr: &'a mut BpfSockAddr<'a>,
        can_block: bool,
    ) -> SockAddrProgramResult {
        let mut run_context = EbpfRunContextImpl::new(current_task);
        match self.0.run_with_1_argument(&mut run_context, addr) {
            // UDP_RECVMSG programs are not allowed to block the packet.
            0 if can_block => SockAddrProgramResult::Block,
            1 => SockAddrProgramResult::Allow,
            result => {
                // TODO(https://fxbug.dev/413490751): Change this to panic once
                // result validation is implemented in the eBPF verifier.
                log_error!("eBPF program returned invalid result: {}", result);
                SockAddrProgramResult::Allow
            }
        }
    }
}

type AttachedSockAddrProgramCell = LockDepRwLock<Option<SockAddrProgram>, EbpfStateLock>;

// Wrapper for `bpf_sock` used to implement `ProgramArgument` trait.
#[repr(C)]
pub struct BpfSock<'a> {
    // Must be first field.
    value: bpf_sock,

    socket: Option<&'a ZxioBackedSocket>,
}

impl<'a> BpfSock<'a> {
    fn from_socket(socket: &'a Socket) -> Self {
        Self {
            value: bpf_sock {
                family: socket.domain.as_raw().into(),
                type_: socket.socket_type.as_raw(),
                protocol: socket.protocol.as_raw(),
                state: socket.bpf_state().into(),
                ..Default::default()
            },
            socket: socket.downcast_socket(),
        }
    }
}

impl<'a> Deref for BpfSock<'a> {
    type Target = bpf_sock;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl<'a> DerefMut for BpfSock<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.value
    }
}

impl<'a> ProgramArgument for &'_ BpfSock<'a> {
    fn get_type() -> &'static Type {
        &*BPF_SOCK_TYPE
    }
}

impl<'a> SocketRef for &'_ BpfSock<'a> {
    fn get_socket_cookie(&self) -> Option<u64> {
        self.socket.and_then(|socket| {
            socket
                .get_socket_cookie()
                .inspect_err(|errno| log_error!("Failed to get socket cookie: {:?}", errno))
                .ok()
        })
    }

    fn get_socket_uid(&self) -> Option<uid_t> {
        self.socket.map(|socket| socket.uid())
    }
}

// Context for eBPF programs of type BPF_PROG_TYPE_CGROUP_SOCK.
struct SockProgram(EbpfProgram<SockProgram>);

impl EbpfProgramContext for SockProgram {
    type RunContext<'a> = EbpfRunContextImpl<'a>;
    type Packet<'a> = ();
    type Arg1<'a> = &'a BpfSock<'a>;
    type Arg2<'a> = ();
    type Arg3<'a> = ();
    type Arg4<'a> = ();
    type Arg5<'a> = ();

    type Map = PinnedMap;
}

ebpf_api::ebpf_program_context_type!(SockProgram, CgroupSockProgramContext);

#[derive(Debug, PartialEq, Eq)]
pub enum SockProgramResult {
    Allow,
    Block,
}

impl SockProgram {
    fn run<'a>(&self, current_task: &'a CurrentTask, sock: &'a BpfSock<'a>) -> SockProgramResult {
        let mut run_context = EbpfRunContextImpl::new(current_task);
        if self.0.run_with_1_argument(&mut run_context, sock) == 0 {
            SockProgramResult::Block
        } else {
            SockProgramResult::Allow
        }
    }
}

type AttachedSockProgramCell = LockDepRwLock<Option<SockProgram>, EbpfStateLock>;

mod internal {
    use super::BpfSock;
    use ebpf::{BpfValue, EbpfPtr, ProgramArgument, Type};
    use ebpf_api::BPF_SOCKOPT_TYPE;
    use starnix_uapi::{bpf_sockopt, uaddr};
    use std::ops::Deref;
    use zerocopy::{FromBytes, IntoBytes};

    // Wrapper for `bpf_sockopt` that implements `ProgramArgument` trait.
    #[repr(C)]
    #[derive(IntoBytes, FromBytes)]
    pub struct BpfSockOpt(bpf_sockopt);

    impl ProgramArgument for &'_ mut BpfSockOpt {
        fn get_type() -> &'static Type {
            &*BPF_SOCKOPT_TYPE
        }
    }

    /// Wrapper for `bpf_sockopt` that keeps a buffer for the `optval`.
    pub struct BpfSockOptWithValue {
        sockopt: BpfSockOpt,

        // Buffer used to store the option value. A pointer to the buffer
        // contents is stored in `sockopt`. `Vec::as_mut_ptr()` guarantees that
        // the pointer remains valid only as long as the `Vec` is not modified,
        // so this field should not be updated directly. `take_value()` can be
        // used to extract the value when `BpfSockOpt` is no longer needed.
        value_buf: Vec<u8>,
    }

    impl BpfSockOptWithValue {
        pub fn new(
            level: u32,
            optname: u32,
            value_buf: Vec<u8>,
            optlen: u32,
            retval: i32,
            sock: *const BpfSock<'_>,
        ) -> Self {
            let mut sockopt = Self {
                sockopt: BpfSockOpt(bpf_sockopt {
                    level: level as i32,
                    optname: optname as i32,
                    optlen: optlen as i32,
                    retval: retval as i32,
                    ..Default::default()
                }),
                value_buf,
            };

            // SAFETY: Setting buffer bounds in unions is safe.
            unsafe {
                sockopt.sockopt.0.__bindgen_anon_2.optval =
                    uaddr { addr: sockopt.value_buf.as_mut_ptr() as u64 };
                sockopt.sockopt.0.__bindgen_anon_3.optval_end = uaddr {
                    addr: sockopt.value_buf.as_mut_ptr().add(sockopt.value_buf.len()) as u64,
                };
            }

            sockopt.sockopt.0.__bindgen_anon_1.sk =
                (uaddr { addr: BpfValue::from(sock).into() }).into();

            sockopt
        }

        pub fn as_ptr<'a>(&'a mut self) -> EbpfPtr<'a, BpfSockOpt> {
            EbpfPtr::from(&mut self.sockopt)
        }

        // Returns the value. Consumes `self` since it's not safe to use again
        // after the value buffer is moved.
        pub fn take_value(self) -> Vec<u8> {
            self.value_buf
        }
    }

    impl Deref for BpfSockOptWithValue {
        type Target = bpf_sockopt;
        fn deref(&self) -> &Self::Target {
            &self.sockopt.0
        }
    }
}

use internal::{BpfSockOpt, BpfSockOptWithValue};

// Context for eBPF programs of type BPF_PROG_TYPE_CGROUP_SOCKOPT.
struct SockOptProgram(EbpfProgram<SockOptProgram>);

// RunContext for eBPF programs of type BPF_PROG_TYPE_CGROUP_SOCKOPT.
pub struct SockOptEbpfRunContextImpl<'a> {
    ebpf_run_context: EbpfRunContextImpl<'a>,

    // Pointer to the BpfSockOpt passed to the program. Used for
    // `bpf_set_retval` and `bpf_get_retval`.
    sockopt: EbpfPtr<'a, BpfSockOpt>,
}

const BPF_SOCKOPT_RETVAL_OFFSET: usize = std::mem::offset_of!(bpf_sockopt, retval);

impl<'a> SockOptEbpfRunContextImpl<'a> {
    pub fn new(current_task: &'a CurrentTask, sockopt: EbpfPtr<'a, BpfSockOpt>) -> Self {
        Self { ebpf_run_context: EbpfRunContextImpl::new(current_task), sockopt }
    }
}

impl<'a> MapsContext<'a> for SockOptEbpfRunContextImpl<'a> {
    fn on_map_access(&mut self, map: &Map) {
        self.ebpf_run_context.on_map_access(map);
    }
    fn add_value_ref(&mut self, map_ref: MapValueRef<'a>) {
        self.ebpf_run_context.add_value_ref(map_ref);
    }
}

impl<'a> CurrentTaskContext for SockOptEbpfRunContextImpl<'a> {
    fn get_uid_gid(&self) -> (uid_t, gid_t) {
        self.ebpf_run_context.get_uid_gid()
    }
    fn get_tid_tgid(&self) -> (pid_t, pid_t) {
        self.ebpf_run_context.get_tid_tgid()
    }
}

impl<'a> ReturnValueContext for SockOptEbpfRunContextImpl<'a> {
    fn set_retval(&mut self, value: i32) -> i32 {
        let sockopt = self.sockopt.get_field::<i32, BPF_SOCKOPT_RETVAL_OFFSET>();
        sockopt.store_relaxed(value);
        0
    }
    fn get_retval(&self) -> i32 {
        let sockopt = self.sockopt.get_field::<i32, BPF_SOCKOPT_RETVAL_OFFSET>();
        sockopt.load_relaxed()
    }
}

impl<'a> BpfSockContext for SockOptEbpfRunContextImpl<'a> {
    type BpfSockRef = &'a BpfSock<'a>;
}

impl EbpfProgramContext for SockOptProgram {
    type RunContext<'a> = SockOptEbpfRunContextImpl<'a>;
    type Packet<'a> = ();
    type Arg1<'a> = EbpfPtr<'a, BpfSockOpt>;
    type Arg2<'a> = ();
    type Arg3<'a> = ();
    type Arg4<'a> = ();
    type Arg5<'a> = ();

    type Map = PinnedMap;
}

ebpf_api::ebpf_program_context_type!(SockOptProgram, CgroupSockOptProgramContext);

#[derive(Debug)]
pub enum SetSockOptProgramResult {
    /// Fail the syscall.
    Fail(Errno),

    /// Proceed with the specified option value.
    Allow(SockOptValue),

    /// Return to userspace without invoking the underlying implementation of
    /// setsockopt.
    Bypass,
}

impl SockOptProgram {
    fn run<'a>(&self, current_task: &'a CurrentTask, sockopt: &'a mut BpfSockOptWithValue) -> u64 {
        let sockopt_ptr = sockopt.as_ptr();
        let mut run_context = SockOptEbpfRunContextImpl::new(current_task, sockopt_ptr);
        self.0.run_with_1_argument(&mut run_context, sockopt_ptr)
    }
}

type AttachedSockOptProgramCell = LockDepRwLock<Option<SockOptProgram>, EbpfStateLock>;

#[derive(Default)]
pub struct CgroupEbpfProgramSet {
    inet4_bind: AttachedSockAddrProgramCell,
    inet6_bind: AttachedSockAddrProgramCell,
    inet4_connect: AttachedSockAddrProgramCell,
    inet6_connect: AttachedSockAddrProgramCell,
    udp4_sendmsg: AttachedSockAddrProgramCell,
    udp6_sendmsg: AttachedSockAddrProgramCell,
    udp4_recvmsg: AttachedSockAddrProgramCell,
    udp6_recvmsg: AttachedSockAddrProgramCell,
    sock_create: AttachedSockProgramCell,
    sock_release: AttachedSockProgramCell,
    set_sockopt: AttachedSockOptProgramCell,
    get_sockopt: AttachedSockOptProgramCell,
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum SockAddrOp {
    Bind,
    Connect,
    UdpSendMsg,
    UdpRecvMsg,
}

#[derive(Eq, PartialEq, Debug, Copy, Clone)]
pub enum SockOp {
    Create,
    Release,
}

impl CgroupEbpfProgramSet {
    fn get_sock_addr_program(
        &self,
        attach_type: AttachType,
    ) -> Result<&AttachedSockAddrProgramCell, Errno> {
        assert!(attach_type.is_cgroup());

        match attach_type {
            AttachType::CgroupInet4Bind => Ok(&self.inet4_bind),
            AttachType::CgroupInet6Bind => Ok(&self.inet6_bind),
            AttachType::CgroupInet4Connect => Ok(&self.inet4_connect),
            AttachType::CgroupInet6Connect => Ok(&self.inet6_connect),
            AttachType::CgroupUdp4Sendmsg => Ok(&self.udp4_sendmsg),
            AttachType::CgroupUdp6Sendmsg => Ok(&self.udp6_sendmsg),
            AttachType::CgroupUdp4Recvmsg => Ok(&self.udp4_recvmsg),
            AttachType::CgroupUdp6Recvmsg => Ok(&self.udp6_recvmsg),
            _ => error!(ENOTSUP),
        }
    }

    fn get_sock_program(&self, attach_type: AttachType) -> Result<&AttachedSockProgramCell, Errno> {
        assert!(attach_type.is_cgroup());

        match attach_type {
            AttachType::CgroupInetSockCreate => Ok(&self.sock_create),
            AttachType::CgroupInetSockRelease => Ok(&self.sock_release),
            _ => error!(ENOTSUP),
        }
    }

    fn get_sock_opt_program(
        &self,
        attach_type: AttachType,
    ) -> Result<&AttachedSockOptProgramCell, Errno> {
        assert!(attach_type.is_cgroup());

        match attach_type {
            AttachType::CgroupSetsockopt => Ok(&self.set_sockopt),
            AttachType::CgroupGetsockopt => Ok(&self.get_sockopt),
            _ => error!(ENOTSUP),
        }
    }

    // Executes eBPF program for the operation `op`. `socket_address` contains
    // socket address as a `sockaddr` struct.
    pub fn run_sock_addr_prog(
        &self,
        current_task: &CurrentTask,
        op: SockAddrOp,
        domain: SocketDomain,
        socket_type: SocketType,
        protocol: SocketProtocol,
        socket_address: &[u8],
        socket: &Socket,
    ) -> Result<SockAddrProgramResult, Errno> {
        let prog_cell = match (domain, op) {
            (SocketDomain::Inet, SockAddrOp::Bind) => &self.inet4_bind,
            (SocketDomain::Inet6, SockAddrOp::Bind) => &self.inet6_bind,
            (SocketDomain::Inet, SockAddrOp::Connect) => &self.inet4_connect,
            (SocketDomain::Inet6, SockAddrOp::Connect) => &self.inet6_connect,
            (SocketDomain::Inet, SockAddrOp::UdpSendMsg) => &self.udp4_sendmsg,
            (SocketDomain::Inet6, SockAddrOp::UdpSendMsg) => &self.udp6_sendmsg,
            (SocketDomain::Inet, SockAddrOp::UdpRecvMsg) => &self.udp4_recvmsg,
            (SocketDomain::Inet6, SockAddrOp::UdpRecvMsg) => &self.udp6_recvmsg,
            _ => return Ok(SockAddrProgramResult::Allow),
        };

        let prog_guard = prog_cell.read();
        let Some(prog) = prog_guard.as_ref() else {
            return Ok(SockAddrProgramResult::Allow);
        };

        let bpf_sock = BpfSock::from_socket(socket);

        let mut bpf_sockaddr = BpfSockAddr { sock_addr: Default::default(), bpf_sock: &bpf_sock };
        bpf_sockaddr.family = domain.as_raw().into();
        bpf_sockaddr.type_ = socket_type.as_raw();
        bpf_sockaddr.protocol = protocol.as_raw();

        let (sa_family, _) = u16::read_from_prefix(socket_address).map_err(|_| errno!(EINVAL))?;

        if domain.as_raw() != sa_family {
            return error!(EAFNOSUPPORT);
        }
        bpf_sockaddr.user_family = sa_family.into();

        match sa_family.into() {
            linux_uapi::AF_INET => {
                let (sockaddr, _) = linux_uapi::sockaddr_in::ref_from_prefix(socket_address)
                    .map_err(|_| errno!(EINVAL))?;
                bpf_sockaddr.user_port = sockaddr.sin_port.into();
                bpf_sockaddr.user_ip4 = sockaddr.sin_addr.s_addr;
            }
            linux_uapi::AF_INET6 => {
                let sockaddr = linux_uapi::sockaddr_in6::ref_from_prefix(socket_address)
                    .map_err(|_| errno!(EINVAL))?
                    .0;
                bpf_sockaddr.user_port = sockaddr.sin6_port.into();
                // SAFETY: reading an array of u32 from a union is safe.
                bpf_sockaddr.user_ip6 = unsafe { sockaddr.sin6_addr.in6_u.u6_addr32 };
            }
            _ => return error!(EAFNOSUPPORT),
        }

        bpf_sockaddr.__bindgen_anon_1.sk =
            (uaddr { addr: BpfValue::from(&bpf_sock).into() }).into();

        // UDP recvmsg programs are not allowed to filter packets.
        let can_block = op != SockAddrOp::UdpRecvMsg;
        Ok(prog.run(current_task, &mut bpf_sockaddr, can_block))
    }

    pub fn run_sock_prog(
        &self,
        current_task: &CurrentTask,
        op: SockOp,
        domain: SocketDomain,
        socket_type: SocketType,
        protocol: SocketProtocol,
        socket: &ZxioBackedSocket,
    ) -> SockProgramResult {
        let prog_cell = match op {
            SockOp::Create => &self.sock_create,
            SockOp::Release => &self.sock_release,
        };
        let prog_guard = prog_cell.read();
        let Some(prog) = prog_guard.as_ref() else {
            return SockProgramResult::Allow;
        };

        let bpf_sock = BpfSock {
            value: bpf_sock {
                family: domain.as_raw().into(),
                type_: socket_type.as_raw(),
                protocol: protocol.as_raw(),
                state: SocketBpfState::Close.into(),
                ..Default::default()
            },
            socket: Some(socket),
        };

        prog.run(current_task, &bpf_sock)
    }

    pub fn run_getsockopt_prog(
        &self,
        current_task: &CurrentTask,
        level: u32,
        optname: u32,
        value_buf: Vec<u8>,
        optlen: usize,
        error: Option<Errno>,
        socket: &Socket,
    ) -> Result<(Vec<u8>, usize), Errno> {
        let prog_guard = self.get_sockopt.read();
        let Some(prog) = prog_guard.as_ref() else {
            return error.map(|e| Err(e)).unwrap_or_else(|| Ok((value_buf, optlen)));
        };

        let bpf_sock = BpfSock::from_socket(socket);

        let retval = error.as_ref().map(|e| -(e.code.error_code() as i32)).unwrap_or(0);
        let mut bpf_sockopt = BpfSockOptWithValue::new(
            level,
            optname,
            value_buf.clone(),
            optlen as u32,
            retval,
            &bpf_sock,
        );

        // Run the program.
        let result = prog.run(current_task, &mut bpf_sockopt);

        let retval = bpf_sockopt.retval;

        let retval = match result {
            0 if is_error_return_value(retval) => retval,
            0 => -(linux_uapi::EPERM as i32),
            1 => retval,
            _ => {
                // TODO(https://fxbug.dev/413490751): Change this to panic once
                // result validation is implemented in the verifier.
                log_error!("eBPF getsockopt program returned invalid result: {}", result);
                retval
            }
        };

        if retval < 0 {
            return Err(Errno::new(ErrnoCode::from_error_code(-retval as i16)));
        }

        let new_optlen = bpf_sockopt.optlen;

        match usize::try_from(new_optlen) {
            // Fail if the program set an invalid `optlen`.
            Err(_) => error!(EFAULT),
            Ok(new_optlen) if new_optlen > value_buf.len() => error!(EFAULT),

            // If `optlen` is set to 0 then proceed with the original value.
            Ok(0) => Ok((value_buf, optlen)),

            Ok(new_optlen) => Ok((bpf_sockopt.take_value(), new_optlen)),
        }
    }

    pub fn run_setsockopt_prog(
        &self,
        current_task: &CurrentTask,
        level: u32,
        optname: u32,
        value: SockOptValue,
        socket: &Socket,
    ) -> SetSockOptProgramResult {
        let prog_guard = self.set_sockopt.read();
        let Some(prog) = prog_guard.as_ref() else {
            return SetSockOptProgramResult::Allow(value);
        };

        let page_size = *PAGE_SIZE as usize;

        // Read only the first page from the user-specified buffer in case it's
        // larger than that.
        let buffer = match value.read_bytes(current_task, page_size) {
            Ok(buffer) => buffer,
            Err(err) => return SetSockOptProgramResult::Fail(err),
        };

        let bpf_sock = BpfSock::from_socket(socket);

        let buffer_len = buffer.len();
        let optlen = value.len();
        let mut bpf_sockopt =
            BpfSockOptWithValue::new(level, optname, buffer, optlen as u32, 0, &bpf_sock);
        let result = prog.run(current_task, &mut bpf_sockopt);

        let retval = bpf_sockopt.retval;

        let retval = match result {
            0 if is_error_return_value(retval) => retval,
            0 => -(linux_uapi::EPERM as i32),
            1 => retval,
            _ => {
                // TODO(https://fxbug.dev/413490751): Change this to panic once
                // result validation is implemented in the verifier.
                log_error!("eBPF getsockopt program returned invalid result: {}", result);
                retval
            }
        };

        if retval < 0 {
            return SetSockOptProgramResult::Fail(Errno::new(ErrnoCode::from_error_code(
                -retval as i16,
            )));
        }

        match bpf_sockopt.optlen {
            // `setsockopt` programs can bypass the platform implementation by
            // setting `optlen` to -1.
            -1 => SetSockOptProgramResult::Bypass,

            // If the original value is larger than a page and the program
            // didn't change `optlen` then return the original value. This
            // allows to avoid `EFAULT` below with a no-op program.
            new_optlen if optlen > page_size && (new_optlen as usize) == optlen => {
                SetSockOptProgramResult::Allow(value)
            }

            // Fail if the program has set an invalid `optlen` (except for the
            // case handled above).
            optlen if optlen < 0 || (optlen as usize) > buffer_len => {
                SetSockOptProgramResult::Fail(errno!(EFAULT))
            }

            // If `optlen` is set to 0 then proceed with the original value.
            0 => SetSockOptProgramResult::Allow(value),

            // Return value from `bpf_sockbuf` - it may be different from the
            // original value.
            optlen => {
                let mut value = bpf_sockopt.take_value();
                value.resize(optlen as usize, 0);
                SetSockOptProgramResult::Allow(value.into())
            }
        }
    }
}

fn attach_type_to_netstack_hook(attach_type: AttachType) -> Option<fnet_filter::SocketHook> {
    let hook = match attach_type {
        AttachType::CgroupInetEgress => fnet_filter::SocketHook::Egress,
        AttachType::CgroupInetIngress => fnet_filter::SocketHook::Ingress,
        _ => return None,
    };
    Some(hook)
}

// Defined a location where eBPF programs can be attached.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum AttachLocation {
    // Attached in Starnix kernel.
    Kernel,

    // Attached in Netstack.
    Netstack,
}

impl TryFrom<AttachType> for AttachLocation {
    type Error = Errno;

    fn try_from(attach_type: AttachType) -> Result<Self, Self::Error> {
        match attach_type {
            AttachType::CgroupInet4Bind
            | AttachType::CgroupInet6Bind
            | AttachType::CgroupInet4Connect
            | AttachType::CgroupInet6Connect
            | AttachType::CgroupUdp4Sendmsg
            | AttachType::CgroupUdp6Sendmsg
            | AttachType::CgroupUdp4Recvmsg
            | AttachType::CgroupUdp6Recvmsg
            | AttachType::CgroupInetSockCreate
            | AttachType::CgroupInetSockRelease
            | AttachType::CgroupGetsockopt
            | AttachType::CgroupSetsockopt => Ok(AttachLocation::Kernel),

            AttachType::CgroupInetEgress | AttachType::CgroupInetIngress => {
                Ok(AttachLocation::Netstack)
            }

            AttachType::CgroupDevice
            | AttachType::CgroupInet4Getpeername
            | AttachType::CgroupInet4Getsockname
            | AttachType::CgroupInet4PostBind
            | AttachType::CgroupInet6Getpeername
            | AttachType::CgroupInet6Getsockname
            | AttachType::CgroupInet6PostBind
            | AttachType::CgroupSysctl
            | AttachType::CgroupUnixConnect
            | AttachType::CgroupUnixGetpeername
            | AttachType::CgroupUnixGetsockname
            | AttachType::CgroupUnixRecvmsg
            | AttachType::CgroupUnixSendmsg
            | AttachType::CgroupSockOps
            | AttachType::SkSkbStreamParser
            | AttachType::SkSkbStreamVerdict
            | AttachType::SkMsgVerdict
            | AttachType::LircMode2
            | AttachType::FlowDissector
            | AttachType::TraceRawTp
            | AttachType::TraceFentry
            | AttachType::TraceFexit
            | AttachType::ModifyReturn
            | AttachType::LsmMac
            | AttachType::TraceIter
            | AttachType::XdpDevmap
            | AttachType::XdpCpumap
            | AttachType::SkLookup
            | AttachType::Xdp
            | AttachType::SkSkbVerdict
            | AttachType::SkReuseportSelect
            | AttachType::SkReuseportSelectOrMigrate
            | AttachType::PerfEvent
            | AttachType::TraceKprobeMulti
            | AttachType::LsmCgroup
            | AttachType::StructOps
            | AttachType::Netfilter
            | AttachType::TcxIngress
            | AttachType::TcxEgress
            | AttachType::TraceUprobeMulti
            | AttachType::NetkitPrimary
            | AttachType::NetkitPeer
            | AttachType::TraceKprobeSession => {
                track_stub!(TODO("https://fxbug.dev/322873416"), "BPF_PROG_ATTACH", attach_type);
                error!(ENOTSUP)
            }

            AttachType::Unspecified | AttachType::Invalid(_) => {
                error!(EINVAL)
            }
        }
    }
}

fn get_capability_for_program(program_type: ProgramType) -> Result<Capabilities, Errno> {
    match program_type {
        ProgramType::CgroupSkb
        | ProgramType::CgroupSock
        | ProgramType::CgroupSockAddr
        | ProgramType::CgroupSockopt
        | ProgramType::CgroupSysctl => Ok(CAP_NET_ADMIN),

        // The following program types cannot be attached with
        // `bpf(BPF_PROG_ATTACH)` yet.
        ProgramType::CgroupDevice
        | ProgramType::Ext
        | ProgramType::FlowDissector
        | ProgramType::Kprobe
        | ProgramType::LircMode2
        | ProgramType::Lsm
        | ProgramType::LwtIn
        | ProgramType::LwtOut
        | ProgramType::LwtSeg6Local
        | ProgramType::LwtXmit
        | ProgramType::Netfilter
        | ProgramType::PerfEvent
        | ProgramType::RawTracepoint
        | ProgramType::RawTracepointWritable
        | ProgramType::SchedAct
        | ProgramType::SchedCls
        | ProgramType::SkLookup
        | ProgramType::SkMsg
        | ProgramType::SkReuseport
        | ProgramType::SkSkb
        | ProgramType::SocketFilter
        | ProgramType::SockOps
        | ProgramType::StructOps
        | ProgramType::Syscall
        | ProgramType::Tracepoint
        | ProgramType::Tracing
        | ProgramType::Unspec
        | ProgramType::Xdp
        | ProgramType::Fuse => error!(ENOTSUP),
    }
}

#[derive(Default)]
pub struct EbpfAttachments {
    root_cgroup: CgroupEbpfProgramSet,
    socket_control: OnceLock<fnet_filter::SocketControlSynchronousProxy>,
}

impl EbpfAttachments {
    pub fn root_cgroup(&self) -> &CgroupEbpfProgramSet {
        &self.root_cgroup
    }

    fn socket_control(&self) -> &fnet_filter::SocketControlSynchronousProxy {
        self.socket_control.get_or_init(|| {
            connect_to_protocol_sync::<fnet_filter::SocketControlMarker>()
                .expect("Failed to connect to fuchsia.net.filter.SocketControl.")
        })
    }

    fn attach_prog(
        &self,
        current_task: &CurrentTask,
        attach_type: AttachType,
        target_fd: FdNumber,
        program: ProgramHandle,
    ) -> Result<SyscallResult, Errno> {
        let location: AttachLocation = attach_type.try_into()?;
        let program_type = attach_type.get_program_type();
        match (location, program_type) {
            (AttachLocation::Kernel, ProgramType::CgroupSockAddr) => {
                check_root_cgroup_fd(current_task, target_fd)?;

                let linked_program = SockAddrProgram(program.link(attach_type.get_program_type())?);
                *self.root_cgroup.get_sock_addr_program(attach_type)?.write() =
                    Some(linked_program);

                Ok(SUCCESS)
            }

            (AttachLocation::Kernel, ProgramType::CgroupSock) => {
                check_root_cgroup_fd(current_task, target_fd)?;

                let linked_program = SockProgram(program.link(attach_type.get_program_type())?);
                *self.root_cgroup.get_sock_program(attach_type)?.write() = Some(linked_program);

                Ok(SUCCESS)
            }

            (AttachLocation::Kernel, ProgramType::CgroupSockopt) => {
                check_root_cgroup_fd(current_task, target_fd)?;

                let linked_program = SockOptProgram(program.link(attach_type.get_program_type())?);
                *self.root_cgroup.get_sock_opt_program(attach_type)?.write() = Some(linked_program);

                Ok(SUCCESS)
            }

            (AttachLocation::Kernel, _) => {
                unreachable!();
            }

            (AttachLocation::Netstack, _) => {
                check_root_cgroup_fd(current_task, target_fd)?;
                self.attach_prog_in_netstack(attach_type, program)
            }
        }
    }

    fn detach_prog(
        &self,
        current_task: &CurrentTask,
        attach_type: AttachType,
        target_fd: FdNumber,
    ) -> Result<SyscallResult, Errno> {
        let location = attach_type.try_into()?;
        let program_type = attach_type.get_program_type();
        match (location, program_type) {
            (AttachLocation::Kernel, ProgramType::CgroupSockAddr) => {
                check_root_cgroup_fd(current_task, target_fd)?;

                let mut prog_guard = self.root_cgroup.get_sock_addr_program(attach_type)?.write();
                if prog_guard.is_none() {
                    return error!(ENOENT);
                }

                *prog_guard = None;

                Ok(SUCCESS)
            }

            (AttachLocation::Kernel, ProgramType::CgroupSock) => {
                check_root_cgroup_fd(current_task, target_fd)?;

                let mut prog_guard = self.root_cgroup.get_sock_program(attach_type)?.write();
                if prog_guard.is_none() {
                    return error!(ENOENT);
                }

                *prog_guard = None;

                Ok(SUCCESS)
            }

            (AttachLocation::Kernel, ProgramType::CgroupSockopt) => {
                check_root_cgroup_fd(current_task, target_fd)?;

                let mut prog_guard = self.root_cgroup.get_sock_opt_program(attach_type)?.write();
                if prog_guard.is_none() {
                    return error!(ENOENT);
                }

                *prog_guard = None;

                Ok(SUCCESS)
            }

            (AttachLocation::Kernel, _) => {
                unreachable!();
            }

            (AttachLocation::Netstack, _) => {
                check_root_cgroup_fd(current_task, target_fd)?;
                self.detach_prog_in_netstack(attach_type)
            }
        }
    }

    fn attach_prog_in_netstack(
        &self,
        attach_type: AttachType,
        program: ProgramHandle,
    ) -> Result<SyscallResult, Errno> {
        let hook = attach_type_to_netstack_hook(attach_type).ok_or_else(|| errno!(ENOTSUP))?;
        let opts = fnet_filter::AttachEbpfProgramOptions {
            hook: Some(hook),
            program: Some((&**program).try_into()?),
            ..Default::default()
        };
        self.socket_control()
            .attach_ebpf_program(opts, zx::MonotonicInstant::INFINITE)
            .map_err(|e| {
                log_error!(
                    "failed to send fuchsia.net.filter/SocketControl.AttachEbpfProgram: {}",
                    e
                );
                errno!(EIO)
            })?
            .map_err(|e| {
                use fnet_filter::SocketControlAttachEbpfProgramError as Error;
                match e {
                    Error::NotSupported => errno!(ENOTSUP),
                    Error::LinkFailed => errno!(EINVAL),
                    Error::MapFailed => errno!(EIO),
                    Error::DuplicateAttachment => errno!(EEXIST),
                }
            })?;

        Ok(SUCCESS)
    }

    fn detach_prog_in_netstack(&self, attach_type: AttachType) -> Result<SyscallResult, Errno> {
        let hook = attach_type_to_netstack_hook(attach_type).ok_or_else(|| errno!(ENOTSUP))?;
        self.socket_control()
            .detach_ebpf_program(hook, zx::MonotonicInstant::INFINITE)
            .map_err(|e| {
                log_error!(
                    "failed to send fuchsia.net.filter/SocketControl.DetachEbpfProgram: {}",
                    e
                );
                errno!(EIO)
            })?
            .map_err(|e| {
                use fnet_filter::SocketControlDetachEbpfProgramError as Error;
                match e {
                    Error::NotFound => errno!(ENOENT),
                }
            })?;
        Ok(SUCCESS)
    }
}
