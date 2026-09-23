// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::zxio::{
    ZXIO_NODE_PROTOCOL_DIRECTORY, ZXIO_NODE_PROTOCOL_FILE, zxio_dirent_iterator_next,
    zxio_dirent_iterator_t,
};
use bitflags::bitflags;
use bstr::BString;
use fidl::encoding::const_assert_eq;
use fidl::endpoints::SynchronousProxy;
use fidl_fuchsia_io as fio;
use pin_weak::sync::PinWeak;
use std::cell::OnceCell;
use std::ffi::CStr;
use std::marker::PhantomData;
use std::mem::{MaybeUninit, size_of, size_of_val};
use std::num::TryFromIntError;
use std::os::raw::{c_char, c_int, c_uint, c_void};
use std::pin::Pin;
use std::sync::Arc;
use zerocopy::{FromBytes, Immutable, IntoBytes, TryFromBytes};
use zxio::{
    ZXIO_SELINUX_CONTEXT_STATE_DATA, ZXIO_SHUTDOWN_OPTIONS_READ, ZXIO_SHUTDOWN_OPTIONS_WRITE,
    msghdr, sockaddr, sockaddr_storage, socklen_t, zx_handle_t, zx_status_t, zxio_object_type_t,
    zxio_seek_origin_t, zxio_socket_mark_t, zxio_storage_t,
};

pub mod zxio;

pub use zxio::{
    zxio_dirent_t, zxio_fsverity_descriptor, zxio_fsverity_descriptor_t,
    zxio_node_attr_zxio_node_attr_has_t as zxio_node_attr_has_t, zxio_node_attributes_t,
    zxio_signals_t,
};

// The inner mod is required because bitflags cannot pass the attribute through to the single
// variant, and attributes cannot be applied to macro invocations.
mod inner_signals {
    // Part of the code for the NONE case that's produced by the macro triggers the lint, but as a
    // whole, the produced code is still correct.
    #![allow(clippy::bad_bit_mask)] // TODO(b/303500202) Remove once addressed in bitflags.
    use super::{bitflags, zxio_signals_t};

    bitflags! {
        // These values should match the values in sdk/lib/zxio/include/lib/zxio/types.h
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct ZxioSignals : zxio_signals_t {
            const NONE            =      0;
            const READABLE        = 1 << 0;
            const WRITABLE        = 1 << 1;
            const READ_DISABLED   = 1 << 2;
            const WRITE_DISABLED  = 1 << 3;
            const READ_THRESHOLD  = 1 << 4;
            const WRITE_THRESHOLD = 1 << 5;
            const OUT_OF_BAND     = 1 << 6;
            const ERROR           = 1 << 7;
            const PEER_CLOSED     = 1 << 8;
        }
    }
}

pub use inner_signals::ZxioSignals;

bitflags! {
    /// The flags for shutting down sockets.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct ZxioShutdownFlags: u32 {
        /// Further transmissions will be disallowed.
        const WRITE = 1 << 0;

        /// Further receptions will be disallowed.
        const READ = 1 << 1;
    }
}

const_assert_eq!(ZxioShutdownFlags::WRITE.bits(), ZXIO_SHUTDOWN_OPTIONS_WRITE);
const_assert_eq!(ZxioShutdownFlags::READ.bits(), ZXIO_SHUTDOWN_OPTIONS_READ);

pub enum SeekOrigin {
    Start,
    Current,
    End,
}

impl From<SeekOrigin> for zxio_seek_origin_t {
    fn from(origin: SeekOrigin) -> Self {
        match origin {
            SeekOrigin::Start => zxio::ZXIO_SEEK_ORIGIN_START,
            SeekOrigin::Current => zxio::ZXIO_SEEK_ORIGIN_CURRENT,
            SeekOrigin::End => zxio::ZXIO_SEEK_ORIGIN_END,
        }
    }
}

// TODO: We need a more comprehensive error strategy.
// Our dependencies create elaborate error objects, but Starnix would prefer
// this library produce zx::Status errors for easier conversion to Errno.

#[derive(Default, Debug)]
pub struct ZxioDirent {
    pub protocols: Option<zxio::zxio_node_protocols_t>,
    pub abilities: Option<zxio::zxio_abilities_t>,
    pub id: Option<zxio::zxio_id_t>,
    pub name: BString,
}

pub struct DirentIterator<'a> {
    iterator: Box<zxio_dirent_iterator_t>,

    // zxio_dirent_iterator_t holds pointers to the underlying directory, so we must keep it alive
    // until we've destroyed it.
    _directory: PhantomData<&'a Zxio>,

    /// Whether the iterator has reached the end of dir entries.
    /// This is necessary because the zxio API returns only once the error code
    /// indicating the iterator has reached the end, where subsequent calls may
    /// return other error codes.
    finished: bool,
}

impl DirentIterator<'_> {
    /// Rewind the iterator to the beginning.
    pub fn rewind(&mut self) -> Result<(), zx::Status> {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_dirent_iterator_rewind(&mut *self.iterator) };
        zx::ok(status)?;
        self.finished = false;
        Ok(())
    }
}

/// It is important that all methods here are &mut self, to require the client
/// to obtain exclusive access to the object, externally locking it.
impl Iterator for DirentIterator<'_> {
    type Item = Result<ZxioDirent, zx::Status>;

    /// Returns the next dir entry for this iterator.
    fn next(&mut self) -> Option<Result<ZxioDirent, zx::Status>> {
        if self.finished {
            return None;
        }
        let mut entry = zxio_dirent_t::default();
        let mut name_buffer = Vec::with_capacity(fio::MAX_NAME_LENGTH as usize);
        // The FFI interface expects a pointer to c_char which is i8 on x86_64.
        // The Rust str and OsStr types expect raw character data to be stored in a buffer u8 values.
        // The types are equivalent for all practical purposes and Rust permits casting between the types,
        // so we insert a type cast here in the FFI bindings.
        entry.name = name_buffer.as_mut_ptr() as *mut c_char;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio_dirent_iterator_next(&mut *self.iterator.as_mut(), &mut entry) };
        let result = match zx::ok(status) {
            Ok(()) => {
                let result = ZxioDirent::from(entry, name_buffer);
                Ok(result)
            }
            Err(zx::Status::NOT_FOUND) => {
                self.finished = true;
                return None;
            }
            Err(e) => Err(e),
        };
        return Some(result);
    }
}

impl Drop for DirentIterator<'_> {
    fn drop(&mut self) {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            zxio::zxio_dirent_iterator_destroy(&mut *self.iterator.as_mut());
        }
    }
}

#[allow(clippy::undocumented_unsafe_blocks, reason = "Force documented unsafe blocks in Starnix")]
unsafe impl Send for DirentIterator<'_> {}
#[allow(clippy::undocumented_unsafe_blocks, reason = "Force documented unsafe blocks in Starnix")]
unsafe impl Sync for DirentIterator<'_> {}

impl ZxioDirent {
    fn from(dirent: zxio_dirent_t, name_buffer: Vec<u8>) -> ZxioDirent {
        let protocols = if dirent.has.protocols { Some(dirent.protocols) } else { None };
        let abilities = if dirent.has.abilities { Some(dirent.abilities) } else { None };
        let id = if dirent.has.id { Some(dirent.id) } else { None };
        let mut name = name_buffer;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            name.set_len(dirent.name_length as usize)
        };
        ZxioDirent { protocols, abilities, id, name: name.into() }
    }

    pub fn is_dir(&self) -> bool {
        self.protocols.map(|p| p & ZXIO_NODE_PROTOCOL_DIRECTORY > 0).unwrap_or(false)
    }

    pub fn is_file(&self) -> bool {
        self.protocols.map(|p| p & ZXIO_NODE_PROTOCOL_FILE > 0).unwrap_or(false)
    }
}

pub struct ZxioErrorCode(i16);
impl ZxioErrorCode {
    pub fn raw(&self) -> i16 {
        self.0
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ControlMessage {
    IpTos(u8),
    IpTtl(u8),
    IpRecvOrigDstAddr([u8; size_of::<zxio::sockaddr_in>()]),
    Ipv6Tclass(u8),
    Ipv6HopLimit(u8),
    IpPacketInfo {
        iface: c_int,
        local_addr: [u8; size_of::<zxio::in_addr>()],
        header_destination_addr: [u8; size_of::<zxio::in_addr>()],
    },
    Ipv6PacketInfo {
        iface: u32,
        local_addr: [u8; size_of::<zxio::in6_addr>()],
    },
    Timestamp {
        sec: i64,
        usec: i64,
    },
    TimestampNs {
        sec: i64,
        nsec: i64,
    },
}

const fn align_cmsg_size(len: usize) -> usize {
    (len + size_of::<usize>() - 1) & !(size_of::<usize>() - 1)
}

const CMSG_HEADER_SIZE: usize = align_cmsg_size(size_of::<zxio::cmsghdr>());

// Size of the buffer to allocate in recvmsg() for cmsgs buffer. We need a buffer that can fit
// Ipv6Tclass, Ipv6HopLimit and Ipv6HopLimit messages.
const MAX_CMSGS_BUFFER: usize =
    CMSG_HEADER_SIZE * 3 + align_cmsg_size(1) * 2 + align_cmsg_size(size_of::<zxio::in6_pktinfo>());

impl ControlMessage {
    pub fn get_data_size(&self) -> usize {
        match self {
            ControlMessage::IpTos(_) => 1,
            ControlMessage::IpTtl(_) => size_of::<c_int>(),
            ControlMessage::IpRecvOrigDstAddr(addr) => size_of_val(&addr),
            ControlMessage::Ipv6Tclass(_) => size_of::<c_int>(),
            ControlMessage::Ipv6HopLimit(_) => size_of::<c_int>(),
            ControlMessage::IpPacketInfo { .. } => size_of::<zxio::in_pktinfo>(),
            ControlMessage::Ipv6PacketInfo { .. } => size_of::<zxio::in6_pktinfo>(),
            ControlMessage::Timestamp { .. } => size_of::<zxio::timeval>(),
            ControlMessage::TimestampNs { .. } => size_of::<zxio::timespec>(),
        }
    }

    // Serializes the data in the format expected by ZXIO.
    fn serialize<'a>(&'a self, out: &'a mut [u8]) -> usize {
        let data = &mut out[CMSG_HEADER_SIZE..];
        let (size, level, type_) = match self {
            ControlMessage::IpTos(v) => {
                v.write_to_prefix(data).unwrap();
                (1, zxio::SOL_IP, zxio::IP_TOS)
            }
            ControlMessage::IpTtl(v) => {
                (*v as c_int).write_to_prefix(data).unwrap();
                (size_of::<c_int>(), zxio::SOL_IP, zxio::IP_TTL)
            }
            ControlMessage::IpRecvOrigDstAddr(v) => {
                v.write_to_prefix(data).unwrap();
                (size_of_val(&v), zxio::SOL_IP, zxio::IP_RECVORIGDSTADDR)
            }
            ControlMessage::Ipv6Tclass(v) => {
                (*v as c_int).write_to_prefix(data).unwrap();
                (size_of::<c_int>(), zxio::SOL_IPV6, zxio::IPV6_TCLASS)
            }
            ControlMessage::Ipv6HopLimit(v) => {
                (*v as c_int).write_to_prefix(data).unwrap();
                (size_of::<c_int>(), zxio::SOL_IPV6, zxio::IPV6_HOPLIMIT)
            }
            ControlMessage::IpPacketInfo { iface, local_addr, header_destination_addr } => {
                let pktinfo = zxio::in_pktinfo {
                    ipi_ifindex: *iface,
                    ipi_spec_dst: zxio::in_addr { s_addr: u32::from_ne_bytes(*local_addr) },
                    ipi_addr: zxio::in_addr {
                        s_addr: u32::from_ne_bytes(*header_destination_addr),
                    },
                };
                pktinfo.write_to_prefix(data).unwrap();
                (size_of_val(&pktinfo), zxio::SOL_IP, zxio::IP_PKTINFO)
            }
            ControlMessage::Ipv6PacketInfo { iface, local_addr } => {
                let pktinfo = zxio::in6_pktinfo {
                    ipi6_addr: zxio::in6_addr {
                        __in6_union: zxio::in6_addr__bindgen_ty_1 { __s6_addr: *local_addr },
                    },
                    ipi6_ifindex: *iface,
                };
                pktinfo.write_to_prefix(data).unwrap();
                (size_of_val(&pktinfo), zxio::SOL_IPV6, zxio::IPV6_PKTINFO)
            }
            ControlMessage::Timestamp { sec, usec } => {
                let timeval = zxio::timeval { tv_sec: *sec, tv_usec: *usec };
                timeval.write_to_prefix(data).unwrap();
                (size_of_val(&timeval), zxio::SOL_SOCKET, zxio::SO_TIMESTAMP)
            }
            ControlMessage::TimestampNs { sec, nsec } => {
                let timespec = zxio::timespec { tv_sec: *sec, tv_nsec: *nsec };
                timespec.write_to_prefix(data).unwrap();
                (size_of_val(&timespec), zxio::SOL_SOCKET, zxio::SO_TIMESTAMPNS)
            }
        };
        let total_size = CMSG_HEADER_SIZE + size;
        let header = zxio::cmsghdr {
            cmsg_len: total_size as c_uint,
            cmsg_level: level as i32,
            cmsg_type: type_ as i32,
        };
        header.write_to_prefix(&mut out[..]).unwrap();

        total_size
    }
}

fn serialize_control_messages(messages: &[ControlMessage]) -> Vec<u8> {
    let size = messages
        .iter()
        .fold(0, |sum, x| sum + CMSG_HEADER_SIZE + align_cmsg_size(x.get_data_size()));
    let mut buffer = vec![0u8; size];
    let mut pos = 0;
    for msg in messages {
        pos += align_cmsg_size(msg.serialize(&mut buffer[pos..]));
    }
    assert_eq!(pos, buffer.len());
    buffer
}

fn parse_control_messages(data: &[u8]) -> Vec<ControlMessage> {
    let mut result = vec![];
    let mut pos = 0;
    loop {
        if pos >= data.len() {
            return result;
        }
        let header_data = &data[pos..];
        let header = match zxio::cmsghdr::read_from_prefix(header_data) {
            Ok((h, _)) if h.cmsg_len as usize > CMSG_HEADER_SIZE => h,
            _ => return result,
        };

        let msg_data = &data[pos + CMSG_HEADER_SIZE..pos + header.cmsg_len as usize];
        let msg = match (header.cmsg_level as u32, header.cmsg_type as u32) {
            (zxio::SOL_IP, zxio::IP_TOS) => {
                ControlMessage::IpTos(u8::read_from_prefix(msg_data).unwrap().0)
            }
            (zxio::SOL_IP, zxio::IP_TTL) => {
                ControlMessage::IpTtl(c_int::read_from_prefix(msg_data).unwrap().0 as u8)
            }
            (zxio::SOL_IP, zxio::IP_RECVORIGDSTADDR) => ControlMessage::IpRecvOrigDstAddr(
                <[u8; size_of::<zxio::sockaddr_in>()]>::read_from_prefix(msg_data).unwrap().0,
            ),
            (zxio::SOL_IP, zxio::IP_PKTINFO) => {
                let pkt_info = zxio::in_pktinfo::read_from_prefix(msg_data).unwrap().0;
                ControlMessage::IpPacketInfo {
                    iface: pkt_info.ipi_ifindex,
                    local_addr: pkt_info.ipi_spec_dst.s_addr.to_ne_bytes(),
                    header_destination_addr: pkt_info.ipi_addr.s_addr.to_ne_bytes(),
                }
            }
            (zxio::SOL_IPV6, zxio::IPV6_TCLASS) => {
                ControlMessage::Ipv6Tclass(c_int::read_from_prefix(msg_data).unwrap().0 as u8)
            }
            (zxio::SOL_IPV6, zxio::IPV6_HOPLIMIT) => {
                ControlMessage::Ipv6HopLimit(c_int::read_from_prefix(msg_data).unwrap().0 as u8)
            }
            (zxio::SOL_IPV6, zxio::IPV6_PKTINFO) => {
                let pkt_info = zxio::in6_pktinfo::read_from_prefix(msg_data).unwrap().0;
                #[allow(
                    clippy::undocumented_unsafe_blocks,
                    reason = "Force documented unsafe blocks in Starnix"
                )]
                ControlMessage::Ipv6PacketInfo {
                    local_addr: unsafe { pkt_info.ipi6_addr.__in6_union.__s6_addr },
                    iface: pkt_info.ipi6_ifindex,
                }
            }
            (zxio::SOL_SOCKET, zxio::SO_TIMESTAMP) => {
                let timeval = zxio::timeval::read_from_prefix(msg_data).unwrap().0;
                ControlMessage::Timestamp { sec: timeval.tv_sec, usec: timeval.tv_usec }
            }
            (zxio::SOL_SOCKET, zxio::SO_TIMESTAMPNS) => {
                let timespec = zxio::timespec::read_from_prefix(msg_data).unwrap().0;
                ControlMessage::TimestampNs { sec: timespec.tv_sec, nsec: timespec.tv_nsec }
            }
            _ => panic!(
                "ZXIO produced unexpected cmsg level={}, type={}",
                header.cmsg_level, header.cmsg_type
            ),
        };
        result.push(msg);

        pos += align_cmsg_size(header.cmsg_len as usize);
    }
}

pub struct RecvMessageInfo {
    pub address: Vec<u8>,
    pub bytes_read: usize,
    pub message_length: usize,
    pub control_messages: Vec<ControlMessage>,
    pub flags: i32,
}

/// Holder type for the resulting context, ensuring that it
pub struct SelinuxContextAttr<'a> {
    buf: &'a mut MaybeUninit<[u8; fio::MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN as usize]>,
    size: OnceCell<usize>,
}

impl<'a> SelinuxContextAttr<'a> {
    /// Creates a holder type for managing the buffer init, where the buffer is backed by the
    /// provided `buf`.
    pub fn new(
        buf: &'a mut MaybeUninit<[u8; fio::MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN as usize]>,
    ) -> Self {
        Self { buf, size: OnceCell::new() }
    }

    /// The number of bytes initialized.
    fn init(&mut self, size: usize) {
        let res = self.size.set(size);
        debug_assert!(res.is_ok());
    }

    /// If a context string was recorded, this returns a slice to it.
    pub fn get(&self) -> Option<&[u8]> {
        let size = self.size.get()?;
        // SAFETY: The OnceCell contains the number of bytes initialized.
        Some(unsafe { self.buf.assume_init_ref()[..*size].as_ref() })
    }
}

/// Options for Open3.
#[derive(Default)]
pub struct ZxioOpenOptions<'a, 'b> {
    attributes: Option<&'a mut zxio_node_attributes_t>,

    /// If an object is to be created, attributes that should be stored with the object at creation
    /// time. Not all servers support all attributes.
    create_attributes: Option<zxio::zxio_node_attr>,

    /// If requesting the SELinux context as part of open, this manages the lifetime.
    selinux_context_read: Option<&'a mut SelinuxContextAttr<'b>>,
}

impl<'a, 'b> ZxioOpenOptions<'a, 'b> {
    /// Consumes the `create_attributes`` but the `attributes` is passed as a mutable ref, since the
    /// retrieved attributes will be written back into it. If any pointer fields are non-null, this
    /// will fail assertions.
    pub fn new(
        attributes: Option<&'a mut zxio_node_attributes_t>,
        create_attributes: Option<zxio::zxio_node_attr>,
    ) -> Self {
        if let Some(attrs) = &attributes {
            validate_pointer_fields(attrs);
        }
        if let Some(attrs) = &create_attributes {
            validate_pointer_fields(attrs);
        }
        Self { attributes, create_attributes, selinux_context_read: None }
    }

    /// Attaches the provided selinux context buffer to the `create_attributes`
    pub fn with_selinux_context_write(mut self, context: &'a [u8]) -> Result<Self, zx::Status> {
        if context.len() > fio::MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN as usize {
            return Err(zx::Status::INVALID_ARGS);
        }
        // If this value increases we'll have to rethink the below conversions.
        const_assert_eq!(fio::MAX_SELINUX_CONTEXT_ATTRIBUTE_LEN, 256);
        {
            let create_attributes = self.create_attributes.get_or_insert_with(Default::default);
            create_attributes.selinux_context_length = context.len() as u16;
            create_attributes.selinux_context_state = ZXIO_SELINUX_CONTEXT_STATE_DATA;
            // SAFETY: In this context the pointer will only be read from, but the type is a
            // mutable pointer.
            create_attributes.selinux_context = context.as_ptr() as *mut u8;
            create_attributes.has.selinux_context = true;
        }
        Ok(self)
    }

    /// Attaches the provided selinux context buffer to receive the context into. This call will
    /// fail if no attributes query was attached, since the success of the fetch cannot be verified.
    pub fn with_selinux_context_read(
        mut self,
        context: &'a mut SelinuxContextAttr<'b>,
    ) -> Result<Self, zx::Status> {
        if let Some(attributes_query) = &mut self.attributes {
            attributes_query.selinux_context = context.buf.as_mut_ptr().cast::<u8>();
            attributes_query.has.selinux_context = true;
            self.selinux_context_read = Some(context);
        } else {
            // If the value attributes query wasn't passed in we can't populate it, and the caller
            // can't get a response back to see if it worked anyways. Fail the request.
            return Err(zx::Status::INVALID_ARGS);
        }
        Ok(self)
    }

    /// Called inside the open method to confirm that some bytes were recorded into the buffer.
    fn init_context_from_read(&mut self) {
        if let (Some(attributes), Some(context)) =
            (&self.attributes, &mut self.selinux_context_read)
        {
            if attributes.selinux_context_state == ZXIO_SELINUX_CONTEXT_STATE_DATA {
                context.init(attributes.selinux_context_length as usize);
            }
        }
    }
}

/// Describes the mode of operation when setting an extended attribute.
#[derive(Copy, Clone, Debug)]
pub enum XattrSetMode {
    /// Create the extended attribute if it doesn't exist, replace the value if it does.
    Set = 1,
    /// Create the extended attribute if it doesn't exist, failing if it does.
    Create = 2,
    /// Replace the value of the extended attribute, failing if it doesn't exist.
    Replace = 3,
}

bitflags! {
    /// Describes the mode of operation when allocating disk space using Allocate.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct AllocateMode: u32 {
        const KEEP_SIZE = 1 << 0;
        const UNSHARE_RANGE = 1 << 1;
        const PUNCH_HOLE = 1 << 2;
        const COLLAPSE_RANGE = 1 << 3;
        const ZERO_RANGE = 1 << 4;
        const INSERT_RANGE = 1 << 5;
    }
}

const_assert_eq!(AllocateMode::KEEP_SIZE.bits(), zxio::ZXIO_ALLOCATE_KEEP_SIZE);
const_assert_eq!(AllocateMode::UNSHARE_RANGE.bits(), zxio::ZXIO_ALLOCATE_UNSHARE_RANGE);
const_assert_eq!(AllocateMode::PUNCH_HOLE.bits(), zxio::ZXIO_ALLOCATE_PUNCH_HOLE);
const_assert_eq!(AllocateMode::COLLAPSE_RANGE.bits(), zxio::ZXIO_ALLOCATE_COLLAPSE_RANGE);
const_assert_eq!(AllocateMode::ZERO_RANGE.bits(), zxio::ZXIO_ALLOCATE_ZERO_RANGE);
const_assert_eq!(AllocateMode::INSERT_RANGE.bits(), zxio::ZXIO_ALLOCATE_INSERT_RANGE);

// `ZxioStorage` is marked as `PhantomPinned` in order to prevent unsafe moves
// of the `zxio_storage_t`, because it may store self-referential types defined
// in zxio.
#[derive(Default)]
struct ZxioStorage {
    storage: zxio::zxio_storage_t,
    _pin: std::marker::PhantomPinned,
}

/// A handle to a zxio object.
///
/// Note: the underlying storage backing the object is pinned on the heap
/// because it can contain self referential data.
pub struct Zxio {
    inner: Pin<Arc<ZxioStorage>>,
}

impl Default for Zxio {
    fn default() -> Self {
        Self { inner: Arc::pin(ZxioStorage::default()) }
    }
}

impl std::fmt::Debug for Zxio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Zxio").finish()
    }
}

pub struct ZxioWeak(PinWeak<ZxioStorage>);

impl ZxioWeak {
    pub fn upgrade(&self) -> Option<Zxio> {
        Some(Zxio { inner: self.0.upgrade()? })
    }
}

/// A trait that provides functionality to connect a channel to a FIDL service.
//
// TODO(https://github.com/rust-lang/rust/issues/44291): allow clients to pass
// in a more general function pointer (`fn(&str, zx::Channel) -> zx::Status`)
// rather than having to implement this trait.
pub trait ServiceConnector {
    /// Returns a channel to the service named by `service_name`.
    fn connect(service_name: &str) -> Result<&'static zx::Channel, zx::Status>;
}

/// Sets `provider_handle` to a handle to the service named by `service_name`.
///
/// This function is intended to be passed to zxio_socket().
///
/// SAFETY: Dereferences the raw pointers `service_name` and `provider_handle`.
unsafe extern "C" fn service_connector<S: ServiceConnector>(
    service_name: *const c_char,
    provider_handle: *mut zx_handle_t,
) -> zx_status_t {
    zx::Status::result_into_raw((|| {
        #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
        let service_name = unsafe { CStr::from_ptr(service_name) }
            .to_str()
            .map_err(|std::str::Utf8Error { .. }| zx::Status::INVALID_ARGS)?;

        S::connect(service_name).map(|channel| {
            #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
            unsafe {
                *provider_handle = channel.raw_handle()
            };
        })
    })())
}

/// Sets `out_storage` as the zxio_storage of `out_context`.
///
/// This function is intended to be passed to zxio_socket().
///
/// SAFETY: Dereferences the raw pointer `out_storage`.
unsafe extern "C" fn storage_allocator(
    _type: zxio_object_type_t,
    out_storage: *mut *mut zxio_storage_t,
    out_context: *mut *mut c_void,
) -> zx_status_t {
    let zxio_ptr_ptr = out_context as *mut *mut zxio_storage_t;
    zx::Status::result_into_raw((|| {
        #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
        if let Some(zxio_ptr) = unsafe { zxio_ptr_ptr.as_mut() } {
            #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
            if let Some(zxio) = unsafe { zxio_ptr.as_mut() } {
                #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
                unsafe {
                    *out_storage = zxio
                };
                return Ok(());
            }
        }
        Err(zx::Status::NO_MEMORY)
    })())
}

/// Ensures that no pointer fields have been set. Used to enforce usage of code paths that can
/// ensure safety. Without this, anything passing a `zxio_node_attributes_t` from the caller to
/// zxio should be marked unsafe.
fn validate_pointer_fields(attrs: &zxio_node_attributes_t) {
    // If you're reading this, and the safe path you want doesn't exist, then add one!
    assert!(
        attrs.fsverity_root_hash.is_null(),
        "Passed in a pointer for the fsverity_root_hash that could not assure the lifetime."
    );
    assert!(
        attrs.selinux_context.is_null(),
        "Passed in a pointer for the selinux_context that could not assure the lifetime."
    );
}

/// To be called before letting any `zxio_node_attributes_t` get passed back to the caller, ensuring
/// that any pointers used in the call are cleaned up.
fn clean_pointer_fields(attrs: &mut zxio_node_attributes_t) {
    attrs.fsverity_root_hash = std::ptr::null_mut();
    attrs.selinux_context = std::ptr::null_mut();
}

pub const ZXIO_ROOT_HASH_LENGTH: usize = 64;

/// A transparent wrapper around the bindgen type.
#[repr(transparent)]
#[derive(IntoBytes, TryFromBytes, Immutable)]
pub struct ZxioSocketMark(zxio_socket_mark_t);

impl ZxioSocketMark {
    fn new(domain: u8, value: u32) -> Self {
        ZxioSocketMark(zxio_socket_mark_t { is_present: true, domain, value, ..Default::default() })
    }

    /// Creates a new socket mark representing the SO_MARK domain.
    pub fn so_mark(mark: u32) -> Self {
        Self::new(fidl_fuchsia_net::MARK_DOMAIN_SO_MARK as u8, mark)
    }

    /// Creates a new socket mark representing the uid domain.
    pub fn uid(uid: u32) -> Self {
        Self::new(fidl_fuchsia_net::MARK_DOMAIN_SOCKET_UID as u8, uid)
    }
}

/// A transparent wrapper around the zxio_wake_group_token_t bindgen type.
#[repr(transparent)]
pub struct ZxioWakeGroupToken(zx_handle_t);

impl ZxioWakeGroupToken {
    /// Creates a wake group token from the provided handle to an event.
    pub fn new(token: Option<zx::Event>) -> Self {
        ZxioWakeGroupToken(token.map(zx::Event::into_raw).unwrap_or(zx::sys::ZX_HANDLE_INVALID))
    }
}

/// Socket creation options that can be used.
pub struct ZxioSocketCreationOptions<'a> {
    pub marks: &'a mut [ZxioSocketMark],
    pub wake_group: ZxioWakeGroupToken,
}

impl Zxio {
    pub fn new_socket<S: ServiceConnector>(
        domain: c_int,
        socket_type: c_int,
        protocol: c_int,
        ZxioSocketCreationOptions { marks, wake_group }: ZxioSocketCreationOptions<'_>,
    ) -> Result<Result<Self, ZxioErrorCode>, zx::Status> {
        let zxio = Zxio::default();
        let mut out_context = zxio.as_storage_ptr() as *mut c_void;
        let mut out_code = 0;

        let ZxioWakeGroupToken(wake_group) = wake_group;
        let creation_opts = zxio::zxio_socket_creation_options {
            num_marks: marks.len(),
            marks: marks.as_mut_ptr() as *mut _,
            wake_group,
            ..Default::default()
        };

        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_socket_with_options(
                Some(service_connector::<S>),
                domain,
                socket_type,
                protocol,
                creation_opts,
                Some(storage_allocator),
                &mut out_context as *mut *mut c_void,
                &mut out_code,
            )
        };
        zx::ok(status)?;
        match out_code {
            0 => Ok(Ok(zxio)),
            _ => Ok(Err(ZxioErrorCode(out_code))),
        }
    }

    fn as_ptr(&self) -> *mut zxio::zxio_t {
        &self.inner.storage.io as *const zxio::zxio_t as *mut zxio::zxio_t
    }

    fn as_storage_ptr(&self) -> *mut zxio::zxio_storage_t {
        &self.inner.storage as *const zxio::zxio_storage_t as *mut zxio::zxio_storage_t
    }

    pub fn create(handle: zx::NullableHandle) -> Result<Zxio, zx::Status> {
        let zxio = Zxio::default();
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_create(handle.into_raw(), zxio.as_storage_ptr()) };
        zx::ok(status)?;
        Ok(zxio)
    }

    pub fn release(self) -> Result<zx::NullableHandle, zx::Status> {
        let mut handle = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_release(self.as_ptr(), &mut handle) };
        zx::ok(status)?;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            Ok(zx::NullableHandle::from_raw(handle))
        }
    }

    pub fn open(
        &self,
        path: &str,
        flags: fio::Flags,
        mut options: ZxioOpenOptions<'_, '_>,
    ) -> Result<Self, zx::Status> {
        let zxio = Zxio::default();

        let mut zxio_open_options = zxio::zxio_open_options::default();
        zxio_open_options.inout_attr = match &mut options.attributes {
            Some(a) => (*a) as *mut zxio_node_attributes_t,
            None => std::ptr::null_mut(),
        };
        zxio_open_options.create_attr = match &options.create_attributes {
            Some(a) => a as *const zxio_node_attributes_t,
            None => std::ptr::null_mut(),
        };

        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_open(
                self.as_ptr(),
                path.as_ptr() as *const c_char,
                path.len(),
                flags.bits(),
                &zxio_open_options,
                zxio.as_storage_ptr(),
            )
        };
        options.init_context_from_read();
        if let Some(attributes) = options.attributes {
            clean_pointer_fields(attributes);
        }
        zx::ok(status)?;
        Ok(zxio)
    }

    pub fn create_with_on_representation(
        handle: zx::NullableHandle,
        attributes: Option<&mut zxio_node_attributes_t>,
    ) -> Result<Zxio, zx::Status> {
        if let Some(attr) = &attributes {
            validate_pointer_fields(attr);
        }
        let zxio = Zxio::default();
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_create_with_on_representation(
                handle.into_raw(),
                attributes.map(|a| a as *mut _).unwrap_or(std::ptr::null_mut()),
                zxio.as_storage_ptr(),
            )
        };
        zx::ok(status)?;
        Ok(zxio)
    }

    /// Opens a limited node connection (similar to O_PATH).
    pub fn open_node(
        &self,
        path: &str,
        flags: fio::Flags,
        attributes: Option<&mut zxio_node_attributes_t>,
    ) -> Result<Self, zx::Status> {
        let zxio = Zxio::default();

        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_open(
                self.as_ptr(),
                path.as_ptr() as *const c_char,
                path.len(),
                // `PROTOCOL_NODE` takes precedence over over protocols. If other protocols are
                // specified as well, it will be used to validate the target node type.
                (flags | fio::Flags::PROTOCOL_NODE).bits(),
                &zxio::zxio_open_options {
                    inout_attr: attributes.map(|a| a as *mut _).unwrap_or(std::ptr::null_mut()),
                    ..Default::default()
                },
                zxio.as_storage_ptr(),
            )
        };
        zx::ok(status)?;
        Ok(zxio)
    }

    pub fn unlink(&self, name: &str, flags: fio::UnlinkFlags) -> Result<(), zx::Status> {
        let flags_bits = flags.bits().try_into().map_err(|_| zx::Status::INVALID_ARGS)?;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_unlink(self.as_ptr(), name.as_ptr() as *const c_char, name.len(), flags_bits)
        };
        zx::ok(status)
    }

    pub fn read(&self, data: &mut [u8]) -> Result<usize, zx::Status> {
        let flags = zxio::zxio_flags_t::default();
        let mut actual = 0usize;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_read(
                self.as_ptr(),
                data.as_ptr() as *mut c_void,
                data.len(),
                flags,
                &mut actual,
            )
        };
        zx::ok(status)?;
        Ok(actual)
    }

    /// Performs a vectorized read, returning the number of bytes read to `data`.
    ///
    /// # Safety
    ///
    /// The caller must check the returned `Result` to make sure the buffers
    /// provided are valid. The caller must provide pointers that are compatible
    /// with the backing implementation of zxio.
    ///
    /// This call allows writing to arbitrary memory locations. It is up to the
    /// caller to make sure that calling this method does not result in undefined
    /// behaviour.
    pub unsafe fn readv(&self, data: &[zxio::zx_iovec]) -> Result<usize, zx::Status> {
        let flags = zxio::zxio_flags_t::default();
        let mut actual = 0usize;
        #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
        let status = unsafe {
            zxio::zxio_readv(
                self.as_ptr(),
                data.as_ptr() as *const zxio::zx_iovec,
                data.len(),
                flags,
                &mut actual,
            )
        };
        zx::ok(status)?;
        Ok(actual)
    }

    pub fn deep_clone(&self) -> Result<Zxio, zx::Status> {
        Zxio::create(self.clone_handle()?)
    }

    pub fn clone_handle(&self) -> Result<zx::NullableHandle, zx::Status> {
        let mut handle = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_clone(self.as_ptr(), &mut handle) };
        zx::ok(status)?;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            Ok(zx::NullableHandle::from_raw(handle))
        }
    }

    pub fn downgrade(&self) -> ZxioWeak {
        ZxioWeak(PinWeak::downgrade(self.inner.clone()))
    }

    pub fn read_at(&self, offset: u64, data: &mut [u8]) -> Result<usize, zx::Status> {
        let flags = zxio::zxio_flags_t::default();
        let mut actual = 0usize;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_read_at(
                self.as_ptr(),
                offset,
                data.as_ptr() as *mut c_void,
                data.len(),
                flags,
                &mut actual,
            )
        };
        zx::ok(status)?;
        Ok(actual)
    }

    /// Performs a vectorized read at an offset, returning the number of bytes
    /// read to `data`.
    ///
    /// # Safety
    ///
    /// The caller must check the returned `Result` to make sure the buffers
    /// provided are valid. The caller must provide pointers that are compatible
    /// with the backing implementation of zxio.
    ///
    /// This call allows writing to arbitrary memory locations. It is up to the
    /// caller to make sure that calling this method does not result in undefined
    /// behaviour.
    pub unsafe fn readv_at(
        &self,
        offset: u64,
        data: &[zxio::zx_iovec],
    ) -> Result<usize, zx::Status> {
        let flags = zxio::zxio_flags_t::default();
        let mut actual = 0usize;
        #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
        let status = unsafe {
            zxio::zxio_readv_at(
                self.as_ptr(),
                offset,
                data.as_ptr() as *const zxio::zx_iovec,
                data.len(),
                flags,
                &mut actual,
            )
        };
        zx::ok(status)?;
        Ok(actual)
    }

    pub fn write(&self, data: &[u8]) -> Result<usize, zx::Status> {
        let flags = zxio::zxio_flags_t::default();
        let mut actual = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_write(
                self.as_ptr(),
                data.as_ptr() as *const c_void,
                data.len(),
                flags,
                &mut actual,
            )
        };
        zx::ok(status)?;
        Ok(actual)
    }

    /// Performs a vectorized write, returning the number of bytes written from
    /// `data`.
    ///
    /// # Safety
    ///
    /// The caller must check the returned `Result` to make sure the buffers
    /// provided are valid. The caller must provide pointers that are compatible
    /// with the backing implementation of zxio.
    ///
    /// This call allows reading from arbitrary memory locations. It is up to the
    /// caller to make sure that calling this method does not result in undefined
    /// behaviour.
    pub unsafe fn writev(&self, data: &[zxio::zx_iovec]) -> Result<usize, zx::Status> {
        let flags = zxio::zxio_flags_t::default();
        let mut actual = 0;
        #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
        let status = unsafe {
            zxio::zxio_writev(
                self.as_ptr(),
                data.as_ptr() as *const zxio::zx_iovec,
                data.len(),
                flags,
                &mut actual,
            )
        };
        zx::ok(status)?;
        Ok(actual)
    }

    pub fn write_at(&self, offset: u64, data: &[u8]) -> Result<usize, zx::Status> {
        let flags = zxio::zxio_flags_t::default();
        let mut actual = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_write_at(
                self.as_ptr(),
                offset,
                data.as_ptr() as *const c_void,
                data.len(),
                flags,
                &mut actual,
            )
        };
        zx::ok(status)?;
        Ok(actual)
    }

    /// Performs a vectorized write at an offset, returning the number of bytes
    /// written from `data`.
    ///
    /// # Safety
    ///
    /// The caller must check the returned `Result` to make sure the buffers
    /// provided are valid. The caller must provide pointers that are compatible
    /// with the backing implementation of zxio.
    ///
    /// This call allows reading from arbitrary memory locations. It is up to the
    /// caller to make sure that calling this method does not result in undefined
    /// behaviour.
    pub unsafe fn writev_at(
        &self,
        offset: u64,
        data: &[zxio::zx_iovec],
    ) -> Result<usize, zx::Status> {
        let flags = zxio::zxio_flags_t::default();
        let mut actual = 0;
        #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
        let status = unsafe {
            zxio::zxio_writev_at(
                self.as_ptr(),
                offset,
                data.as_ptr() as *const zxio::zx_iovec,
                data.len(),
                flags,
                &mut actual,
            )
        };
        zx::ok(status)?;
        Ok(actual)
    }

    pub fn truncate(&self, length: u64) -> Result<(), zx::Status> {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_truncate(self.as_ptr(), length) };
        zx::ok(status)?;
        Ok(())
    }

    pub fn seek(&self, seek_origin: SeekOrigin, offset: i64) -> Result<usize, zx::Status> {
        let mut result = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status =
            unsafe { zxio::zxio_seek(self.as_ptr(), seek_origin.into(), offset, &mut result) };
        zx::ok(status)?;
        Ok(result)
    }

    pub fn vmo_get(&self, flags: zx::VmarFlags) -> Result<zx::Vmo, zx::Status> {
        let mut vmo = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_vmo_get(self.as_ptr(), flags.bits(), &mut vmo) };
        zx::ok(status)?;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let handle = unsafe { zx::NullableHandle::from_raw(vmo) };
        Ok(zx::Vmo::from(handle))
    }

    fn node_attributes_from_query(
        &self,
        query: zxio_node_attr_has_t,
        fsverity_root_hash: Option<&mut [u8; ZXIO_ROOT_HASH_LENGTH]>,
    ) -> zxio_node_attributes_t {
        if let Some(fsverity_root_hash) = fsverity_root_hash {
            zxio_node_attributes_t {
                has: query,
                fsverity_root_hash: fsverity_root_hash as *mut u8,
                ..Default::default()
            }
        } else {
            zxio_node_attributes_t { has: query, ..Default::default() }
        }
    }

    pub fn attr_get(
        &self,
        query: zxio_node_attr_has_t,
    ) -> Result<zxio_node_attributes_t, zx::Status> {
        let mut attributes = self.node_attributes_from_query(query, None);
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_attr_get(self.as_ptr(), &mut attributes) };
        zx::ok(status)?;
        Ok(attributes)
    }

    // Call this function if access time needs to be updated before closing the node. For the case
    // where the underlying filesystem is unable to manage access time updates by itself (e.g.
    // because it cannot differentiate between a file read and write operation), Starnix is
    // responsible for informing the underlying filesystem that the node has been accessed and is
    // pending an access time update.
    pub fn close_and_update_access_time(self) -> Result<(), zx::Status> {
        let mut out_handle = zx::sys::ZX_HANDLE_INVALID;
        // SAFETY: This is okay as we have exclusive access to this Zxio object.
        let status = unsafe { zxio::zxio_release(self.as_ptr(), &mut out_handle) };
        zx::ok(status)?;
        let proxy = fio::NodeSynchronousProxy::from_channel(
            // SAFETY: `out_handle` extracted from `zxio_release` should be a valid handle.
            unsafe { zx::NullableHandle::from_raw(out_handle) }.into(),
        );

        // Don't wait for response from `get_attributes` (by setting deadline to `INFINITE_PAST`).
        // We don't need to know any queried attributes, and ignore if this request fails.
        // Expect this to fail with a TIMED_OUT error. Timeouts are generally fatal and clients do
        // not expect to continue communications on a channel that is timing out.
        let _ = proxy.get_attributes(
            fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE,
            zx::MonotonicInstant::INFINITE_PAST,
        );

        // The handle will be dropped when the proxy is dropped. `zxio_close` is called when self
        // is dropped.
        Ok(())
    }

    /// Assumes that the caller has set `query.fsverity_root_hash` to true.
    pub fn attr_get_with_root_hash(
        &self,
        query: zxio_node_attr_has_t,
        fsverity_root_hash: &mut [u8; ZXIO_ROOT_HASH_LENGTH],
    ) -> Result<zxio_node_attributes_t, zx::Status> {
        let mut attributes = self.node_attributes_from_query(query, Some(fsverity_root_hash));
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_attr_get(self.as_ptr(), &mut attributes) };
        clean_pointer_fields(&mut attributes);
        zx::ok(status)?;
        Ok(attributes)
    }

    pub fn attr_set(&self, attributes: &zxio_node_attributes_t) -> Result<(), zx::Status> {
        validate_pointer_fields(attributes);
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_attr_set(self.as_ptr(), attributes) };
        zx::ok(status)?;
        Ok(())
    }

    pub fn enable_verity(&self, descriptor: &zxio_fsverity_descriptor_t) -> Result<(), zx::Status> {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_enable_verity(self.as_ptr(), descriptor) };
        zx::ok(status)?;
        Ok(())
    }

    pub fn rename(
        &self,
        old_path: &str,
        new_directory: &Zxio,
        new_path: &str,
    ) -> Result<(), zx::Status> {
        let mut handle = zx::sys::ZX_HANDLE_INVALID;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_token_get(new_directory.as_ptr(), &mut handle) };
        zx::ok(status)?;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_rename(
                self.as_ptr(),
                old_path.as_ptr() as *const c_char,
                old_path.len(),
                handle,
                new_path.as_ptr() as *const c_char,
                new_path.len(),
            )
        };
        zx::ok(status)?;
        Ok(())
    }

    pub fn wait_begin(
        &self,
        zxio_signals: zxio_signals_t,
    ) -> (zx::Unowned<'_, zx::NullableHandle>, zx::Signals) {
        let mut handle = zx::sys::ZX_HANDLE_INVALID;
        let mut zx_signals = zx::sys::ZX_SIGNAL_NONE;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            zxio::zxio_wait_begin(self.as_ptr(), zxio_signals, &mut handle, &mut zx_signals)
        };
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let handle = unsafe { zx::Unowned::<zx::NullableHandle>::from_raw_handle(handle) };
        let signals = zx::Signals::from_bits_truncate(zx_signals);
        (handle, signals)
    }

    pub fn wait_end(&self, signals: zx::Signals) -> zxio_signals_t {
        let mut zxio_signals = ZxioSignals::NONE.bits();
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            zxio::zxio_wait_end(self.as_ptr(), signals.bits(), &mut zxio_signals);
        }
        zxio_signals
    }

    pub fn create_dirent_iterator(&self) -> Result<DirentIterator<'_>, zx::Status> {
        let mut zxio_iterator = Box::default();
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_dirent_iterator_init(&mut *zxio_iterator, self.as_ptr()) };
        zx::ok(status)?;
        let iterator =
            DirentIterator { iterator: zxio_iterator, _directory: PhantomData, finished: false };
        Ok(iterator)
    }

    pub fn connect(&self, addr: &[u8]) -> Result<Result<(), ZxioErrorCode>, zx::Status> {
        let mut out_code = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_connect(
                self.as_ptr(),
                addr.as_ptr() as *const sockaddr,
                addr.len() as socklen_t,
                &mut out_code,
            )
        };
        zx::ok(status)?;
        match out_code {
            0 => Ok(Ok(())),
            _ => Ok(Err(ZxioErrorCode(out_code))),
        }
    }

    pub fn bind(&self, addr: &[u8]) -> Result<Result<(), ZxioErrorCode>, zx::Status> {
        let mut out_code = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_bind(
                self.as_ptr(),
                addr.as_ptr() as *const sockaddr,
                addr.len() as socklen_t,
                &mut out_code,
            )
        };
        zx::ok(status)?;
        match out_code {
            0 => Ok(Ok(())),
            _ => Ok(Err(ZxioErrorCode(out_code))),
        }
    }

    pub fn listen(&self, backlog: i32) -> Result<Result<(), ZxioErrorCode>, zx::Status> {
        let mut out_code = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_listen(self.as_ptr(), backlog as c_int, &mut out_code) };
        zx::ok(status)?;
        match out_code {
            0 => Ok(Ok(())),
            _ => Ok(Err(ZxioErrorCode(out_code))),
        }
    }

    pub fn accept(&self) -> Result<Result<Zxio, ZxioErrorCode>, zx::Status> {
        let mut addrlen = std::mem::size_of::<sockaddr_storage>() as socklen_t;
        let mut addr = vec![0u8; addrlen as usize];
        let zxio = Zxio::default();
        let mut out_code = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_accept(
                self.as_ptr(),
                addr.as_mut_ptr() as *mut sockaddr,
                &mut addrlen,
                zxio.as_storage_ptr(),
                &mut out_code,
            )
        };
        zx::ok(status)?;
        match out_code {
            0 => Ok(Ok(zxio)),
            _ => Ok(Err(ZxioErrorCode(out_code))),
        }
    }

    pub fn getsockname(&self) -> Result<Result<Vec<u8>, ZxioErrorCode>, zx::Status> {
        let mut addrlen = std::mem::size_of::<sockaddr_storage>() as socklen_t;
        let mut addr = vec![0u8; addrlen as usize];
        let mut out_code = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_getsockname(
                self.as_ptr(),
                addr.as_mut_ptr() as *mut sockaddr,
                &mut addrlen,
                &mut out_code,
            )
        };
        zx::ok(status)?;
        match out_code {
            0 => Ok(Ok(addr[..addrlen as usize].to_vec())),
            _ => Ok(Err(ZxioErrorCode(out_code))),
        }
    }

    pub fn getpeername(&self) -> Result<Result<Vec<u8>, ZxioErrorCode>, zx::Status> {
        let mut addrlen = std::mem::size_of::<sockaddr_storage>() as socklen_t;
        let mut addr = vec![0u8; addrlen as usize];
        let mut out_code = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_getpeername(
                self.as_ptr(),
                addr.as_mut_ptr() as *mut sockaddr,
                &mut addrlen,
                &mut out_code,
            )
        };
        zx::ok(status)?;
        match out_code {
            0 => Ok(Ok(addr[..addrlen as usize].to_vec())),
            _ => Ok(Err(ZxioErrorCode(out_code))),
        }
    }

    pub fn getsockopt_slice(
        &self,
        level: u32,
        optname: u32,
        optval: &mut [u8],
    ) -> Result<Result<socklen_t, ZxioErrorCode>, zx::Status> {
        let mut optlen = optval.len() as socklen_t;
        let mut out_code = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_getsockopt(
                self.as_ptr(),
                level as c_int,
                optname as c_int,
                optval.as_mut_ptr() as *mut c_void,
                &mut optlen,
                &mut out_code,
            )
        };
        zx::ok(status)?;
        match out_code {
            0 => Ok(Ok(optlen)),
            _ => Ok(Err(ZxioErrorCode(out_code))),
        }
    }

    pub fn getsockopt(
        &self,
        level: u32,
        optname: u32,
        optlen: socklen_t,
    ) -> Result<Result<Vec<u8>, ZxioErrorCode>, zx::Status> {
        let mut optval = vec![0u8; optlen as usize];
        let result = self.getsockopt_slice(level, optname, &mut optval[..])?;
        Ok(result.map(|optlen| optval[..optlen as usize].to_vec()))
    }

    pub fn setsockopt(
        &self,
        level: i32,
        optname: i32,
        optval: &[u8],
        access_token: Option<zx::NullableHandle>,
    ) -> Result<Result<(), ZxioErrorCode>, zx::Status> {
        let mut out_code = 0;
        let access_token_handle =
            access_token.map(zx::NullableHandle::into_raw).unwrap_or(zx::sys::ZX_HANDLE_INVALID);

        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_setsockopt(
                self.as_ptr(),
                level,
                optname,
                optval.as_ptr() as *const c_void,
                optval.len() as socklen_t,
                access_token_handle,
                &mut out_code,
            )
        };
        zx::ok(status)?;
        match out_code {
            0 => Ok(Ok(())),
            _ => Ok(Err(ZxioErrorCode(out_code))),
        }
    }

    pub fn shutdown(
        &self,
        flags: ZxioShutdownFlags,
    ) -> Result<Result<(), ZxioErrorCode>, zx::Status> {
        let mut out_code = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_shutdown(self.as_ptr(), flags.bits(), &mut out_code) };
        zx::ok(status)?;
        match out_code {
            0 => Ok(Ok(())),
            _ => Ok(Err(ZxioErrorCode(out_code))),
        }
    }

    pub fn sendmsg(
        &self,
        addr: &mut [u8],
        buffer: &mut [zxio::iovec],
        cmsg: &[ControlMessage],
        flags: u32,
    ) -> Result<Result<usize, ZxioErrorCode>, zx::Status> {
        let mut msg = zxio::msghdr::default();
        msg.msg_name = match addr.len() {
            0 => std::ptr::null_mut() as *mut c_void,
            _ => addr.as_mut_ptr() as *mut c_void,
        };
        msg.msg_namelen = addr.len() as u32;

        msg.msg_iovlen =
            i32::try_from(buffer.len()).map_err(|_: TryFromIntError| zx::Status::INVALID_ARGS)?;
        msg.msg_iov = buffer.as_mut_ptr();

        let mut cmsg_buffer = serialize_control_messages(cmsg);
        msg.msg_control = cmsg_buffer.as_mut_ptr() as *mut c_void;
        msg.msg_controllen = cmsg_buffer.len() as u32;

        let mut out_code = 0;
        let mut out_actual = 0;

        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_sendmsg(self.as_ptr(), &msg, flags as c_int, &mut out_actual, &mut out_code)
        };

        zx::ok(status)?;
        match out_code {
            0 => Ok(Ok(out_actual)),
            _ => Ok(Err(ZxioErrorCode(out_code))),
        }
    }

    pub fn recvmsg(
        &self,
        buffer: &mut [zxio::iovec],
        flags: u32,
    ) -> Result<Result<RecvMessageInfo, ZxioErrorCode>, zx::Status> {
        let mut msg = msghdr::default();
        let mut addr = vec![0u8; std::mem::size_of::<sockaddr_storage>()];
        msg.msg_name = addr.as_mut_ptr() as *mut c_void;
        msg.msg_namelen = addr.len() as u32;

        let max_buffer_capacity = buffer.iter().map(|v| v.iov_len).sum();
        msg.msg_iovlen =
            i32::try_from(buffer.len()).map_err(|_: TryFromIntError| zx::Status::INVALID_ARGS)?;
        msg.msg_iov = buffer.as_mut_ptr();

        let mut cmsg_buffer = vec![0u8; MAX_CMSGS_BUFFER];
        msg.msg_control = cmsg_buffer.as_mut_ptr() as *mut c_void;
        msg.msg_controllen = cmsg_buffer.len() as u32;

        let mut out_code = 0;
        let mut out_actual = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_recvmsg(
                self.as_ptr(),
                &mut msg,
                flags as c_int,
                &mut out_actual,
                &mut out_code,
            )
        };
        zx::ok(status)?;

        if out_code != 0 {
            return Ok(Err(ZxioErrorCode(out_code)));
        }

        let control_messages = parse_control_messages(&cmsg_buffer[..msg.msg_controllen as usize]);
        Ok(Ok(RecvMessageInfo {
            address: addr[..msg.msg_namelen as usize].to_vec(),
            bytes_read: std::cmp::min(max_buffer_capacity, out_actual),
            message_length: out_actual,
            control_messages,
            flags: msg.msg_flags,
        }))
    }

    pub fn read_link(&self) -> Result<&[u8], zx::Status> {
        let mut target = std::ptr::null();
        let mut target_len = 0;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_read_link(self.as_ptr(), &mut target, &mut target_len) };
        zx::ok(status)?;
        // SAFETY: target will live as long as the underlying zxio object lives.
        unsafe { Ok(std::slice::from_raw_parts(target, target_len)) }
    }

    pub fn create_symlink(&self, name: &str, target: &[u8]) -> Result<Zxio, zx::Status> {
        let name = name.as_bytes();
        let zxio = Zxio::default();
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_create_symlink(
                self.as_ptr(),
                name.as_ptr() as *const c_char,
                name.len(),
                target.as_ptr(),
                target.len(),
                zxio.as_storage_ptr(),
            )
        };
        zx::ok(status)?;
        Ok(zxio)
    }

    pub fn xattr_list(&self) -> Result<Vec<Vec<u8>>, zx::Status> {
        unsafe extern "C" fn callback(context: *mut c_void, name: *const u8, name_len: usize) {
            #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
            let out_names = unsafe { &mut *(context as *mut Vec<Vec<u8>>) };
            #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
            let name_slice = unsafe { std::slice::from_raw_parts(name, name_len) };
            out_names.push(name_slice.to_vec());
        }
        let mut out_names = Vec::new();
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_xattr_list(
                self.as_ptr(),
                Some(callback),
                &mut out_names as *mut _ as *mut c_void,
            )
        };
        zx::ok(status)?;
        Ok(out_names)
    }

    pub fn xattr_get(&self, name: &[u8]) -> Result<Vec<u8>, zx::Status> {
        unsafe extern "C" fn callback(
            context: *mut c_void,
            data: zxio::zxio_xattr_data_t,
        ) -> zx_status_t {
            #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
            let out_value = unsafe { &mut *(context as *mut Vec<u8>) };
            if data.data.is_null() {
                #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
                let value_vmo = unsafe { zx::Unowned::<'_, zx::Vmo>::from_raw_handle(data.vmo) };
                match value_vmo.read_to_vec(0, data.len as u64) {
                    Ok(vec) => *out_value = vec,
                    Err(status) => return status.into_raw(),
                }
            } else {
                #[allow(clippy::undocumented_unsafe_blocks, reason = "2024 edition migration")]
                let value_slice =
                    unsafe { std::slice::from_raw_parts(data.data as *mut u8, data.len) };
                out_value.extend_from_slice(value_slice);
            }
            zx::sys::ZX_OK
        }
        let mut out_value = Vec::new();
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_xattr_get(
                self.as_ptr(),
                name.as_ptr(),
                name.len(),
                Some(callback),
                &mut out_value as *mut _ as *mut c_void,
            )
        };
        zx::ok(status)?;
        Ok(out_value)
    }

    pub fn xattr_set(
        &self,
        name: &[u8],
        value: &[u8],
        mode: XattrSetMode,
    ) -> Result<(), zx::Status> {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_xattr_set(
                self.as_ptr(),
                name.as_ptr(),
                name.len(),
                value.as_ptr(),
                value.len(),
                mode as u32,
            )
        };
        zx::ok(status)
    }

    pub fn xattr_remove(&self, name: &[u8]) -> Result<(), zx::Status> {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        zx::ok(unsafe { zxio::zxio_xattr_remove(self.as_ptr(), name.as_ptr(), name.len()) })
    }

    pub fn link_into(&self, target_dir: &Zxio, name: &str) -> Result<(), zx::Status> {
        let mut handle = zx::sys::ZX_HANDLE_INVALID;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        zx::ok(unsafe { zxio::zxio_token_get(target_dir.as_ptr(), &mut handle) })?;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        zx::ok(unsafe {
            zxio::zxio_link_into(self.as_ptr(), handle, name.as_ptr() as *const c_char, name.len())
        })
    }

    pub fn allocate(&self, offset: u64, len: u64, mode: AllocateMode) -> Result<(), zx::Status> {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_allocate(self.as_ptr(), offset, len, mode.bits()) };
        zx::ok(status)
    }

    pub fn sync(&self) -> Result<(), zx::Status> {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_sync(self.as_ptr()) };
        zx::ok(status)
    }

    pub fn close(&self) -> Result<(), zx::Status> {
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe { zxio::zxio_close(self.as_ptr()) };
        zx::ok(status)
    }

    pub fn get_read_buffer_available(&self) -> Result<usize, zx::Status> {
        let mut available = 0usize;
        // SAFETY: `self.zxio` is a valid pointer and is guaranteed to be valid for the duration
        // of the call.
        let status = unsafe { zxio::zxio_get_read_buffer_available(self.as_ptr(), &mut available) };
        zx::ok(status)?;
        Ok(available)
    }
}

impl Drop for ZxioStorage {
    fn drop(&mut self) {
        // `zxio_destroy` should be called once only when `ZxioStorage` is dropped, just before the
        // `zxio_storage_t` memory is deallocated. Operations on an upgraded `ZxioWeak`,
        // like WaitCanceler, will then always operate on a non-poisoned `zxio_t as long as the
        // `ZxioStorage` is alive.
        let zxio_ptr: *mut zxio::zxio_t = &mut self.storage.io;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            zxio::zxio_destroy(zxio_ptr);
        };
    }
}

impl Clone for Zxio {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

enum NodeKind {
    File,
    Directory,
    Symlink,
    Unknown,
}

impl From<fio::Representation> for NodeKind {
    fn from(representation: fio::Representation) -> Self {
        match representation {
            fio::Representation::File(_) => NodeKind::File,
            fio::Representation::Directory(_) => NodeKind::Directory,
            fio::Representation::Symlink(_) => NodeKind::Symlink,
            _ => NodeKind::Unknown,
        }
    }
}

/// A fuchsia.io.Node along with its NodeInfoDeprecated.
///
/// The NodeInfoDeprecated provides information about the concrete protocol spoken by the
/// node.
struct DescribedNode {
    node: fio::NodeSynchronousProxy,
    kind: NodeKind,
}

/// Open the given path in the given directory.
///
/// The semantics for the flags argument are defined by the
/// fuchsia.io/Directory.Open3 message.
///
/// This function adds FLAG_SEND_REPRESENTATION to the given flags and then blocks
/// until the directory describes the newly opened node.
///
/// Returns the opened Node, along with its NodeKind, or an error.
fn directory_open(
    directory: &fio::DirectorySynchronousProxy,
    path: &str,
    flags: fio::Flags,
    deadline: zx::MonotonicInstant,
) -> Result<DescribedNode, zx::Status> {
    let flags = flags | fio::Flags::FLAG_SEND_REPRESENTATION;

    let (client_end, server_end) = zx::Channel::create();
    directory.open(path, flags, &Default::default(), server_end).map_err(|_| zx::Status::IO)?;
    let node = fio::NodeSynchronousProxy::new(client_end);

    match node.wait_for_event(deadline).map_err(map_fidl_error)? {
        fio::NodeEvent::OnOpen_ { .. } => {
            panic!("Should never happen when sending FLAG_SEND_REPRESENTATION")
        }
        fio::NodeEvent::OnRepresentation { payload } => {
            Ok(DescribedNode { node, kind: payload.into() })
        }
        fio::NodeEvent::_UnknownEvent { .. } => Err(zx::Status::NOT_SUPPORTED),
    }
}

/// Open a VMO at the given path in the given directory.
///
/// The semantics for the vmo_flags argument are defined by the
/// fuchsia.io/File.GetBackingMemory message (i.e., VmoFlags::*).
///
/// If the node at the given path is not a VMO, then this function returns
/// a zx::Status::IO error.
pub fn directory_open_vmo(
    directory: &fio::DirectorySynchronousProxy,
    path: &str,
    vmo_flags: fio::VmoFlags,
    deadline: zx::MonotonicInstant,
) -> Result<zx::Vmo, zx::Status> {
    let mut flags = fio::Flags::empty();
    if vmo_flags.contains(fio::VmoFlags::READ) {
        flags |= fio::PERM_READABLE;
    }
    if vmo_flags.contains(fio::VmoFlags::WRITE) {
        flags |= fio::PERM_WRITABLE;
    }
    if vmo_flags.contains(fio::VmoFlags::EXECUTE) {
        flags |= fio::PERM_EXECUTABLE;
    }
    let description = directory_open(directory, path, flags, deadline)?;
    let file = match description.kind {
        NodeKind::File => fio::FileSynchronousProxy::new(description.node.into_channel()),
        _ => return Err(zx::Status::IO),
    };

    let vmo = file
        .get_backing_memory(vmo_flags, deadline)
        .map_err(map_fidl_error)?
        .map_err(zx::Status::err_from_raw)?;
    Ok(vmo)
}

/// Read the content of the file at the given path in the given directory.
///
/// If the node at the given path is not a file, then this function returns
/// a zx::Status::IO error.
pub fn directory_read_file(
    directory: &fio::DirectorySynchronousProxy,
    path: &str,
    deadline: zx::MonotonicInstant,
) -> Result<Vec<u8>, zx::Status> {
    let description = directory_open(directory, path, fio::PERM_READABLE, deadline)?;
    let file = match description.kind {
        NodeKind::File => fio::FileSynchronousProxy::new(description.node.into_channel()),
        _ => return Err(zx::Status::IO),
    };

    let mut result = Vec::new();
    loop {
        let mut data = file
            .read(fio::MAX_TRANSFER_SIZE, deadline)
            .map_err(map_fidl_error)?
            .map_err(zx::Status::err_from_raw)?;
        let finished = (data.len() as u64) < fio::MAX_TRANSFER_SIZE;
        result.append(&mut data);
        if finished {
            return Ok(result);
        }
    }
}

/// Create an anonymous temp file in the given directory.
pub fn directory_create_tmp_file(
    directory: &fio::DirectorySynchronousProxy,
    flags: fio::Flags,
    deadline: zx::MonotonicInstant,
) -> Result<fio::FileSynchronousProxy, zx::Status> {
    let description = directory_open(
        directory,
        ".",
        flags
            | fio::PERM_WRITABLE
            | fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY
            | fio::Flags::PROTOCOL_FILE,
        deadline,
    )?;
    let file = match description.kind {
        NodeKind::File => fio::FileSynchronousProxy::new(description.node.into_channel()),
        _ => return Err(zx::Status::NOT_FILE),
    };

    Ok(file)
}

/// Open the given path in the given directory without blocking.
///
/// A zx::Channel to the opened node is returned (or an error).
///
/// It is an error to supply the FLAG_SEND_REPRESENTATION flag in flags.
///
/// This function will "succeed" even if the given path does not exist in the
/// given directory because this function does not wait for the directory to
/// confirm that the path exists.
pub fn directory_open_async(
    directory: &fio::DirectorySynchronousProxy,
    path: &str,
    flags: fio::Flags,
) -> Result<fio::DirectorySynchronousProxy, zx::Status> {
    if flags.intersects(fio::Flags::FLAG_SEND_REPRESENTATION) {
        return Err(zx::Status::INVALID_ARGS);
    }

    let (proxy, server_end) = fidl::endpoints::create_sync_proxy::<fio::DirectoryMarker>();
    directory
        .open(path, flags, &Default::default(), server_end.into_channel())
        .map_err(|_| zx::Status::IO)?;
    Ok(proxy)
}

/// Open a directory at the given path in the given directory without blocking.
///
/// This function adds the PROTOCOL_DIRECTORY flag to ensure that the open operation completes only
/// if the given path is actually a directory, which means clients can start using the returned
/// DirectorySynchronousProxy immediately without waiting for the server to complete the operation.
///
/// This function will "succeed" even if the given path does not exist in the
/// given directory or if the path is not a directory because this function
/// does not wait for the directory to confirm that the path exists and is a
/// directory.
pub fn directory_open_directory_async(
    directory: &fio::DirectorySynchronousProxy,
    path: &str,
    flags: fio::Flags,
) -> Result<fio::DirectorySynchronousProxy, zx::Status> {
    let flags = flags | fio::Flags::PROTOCOL_DIRECTORY;
    let proxy = directory_open_async(directory, path, flags)?;
    Ok(proxy)
}

fn map_fidl_error(error: fidl::Error) -> zx::Status {
    match error {
        fidl::Error::ClientChannelClosed { epitaph, .. } => match epitaph.into() {
            Err(s) => s,
            Ok(()) => zx::Status::PEER_CLOSED,
        },
        _ => zx::Status::IO,
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use anyhow::Error;
    use fidl::endpoints::Proxy as _;
    use fidl_fuchsia_io as fio;
    use fuchsia_fs::directory;

    fn open_pkg() -> fio::DirectorySynchronousProxy {
        let pkg_proxy =
            directory::open_in_namespace("/pkg", fio::PERM_READABLE | fio::PERM_EXECUTABLE)
                .expect("failed to open /pkg");
        fio::DirectorySynchronousProxy::new(
            pkg_proxy
                .into_channel()
                .expect("failed to convert proxy into channel")
                .into_zx_channel(),
        )
    }

    #[fuchsia::test]
    async fn test_directory_open() -> Result<(), Error> {
        let pkg = open_pkg();
        let description = directory_open(
            &pkg,
            "bin/syncio_lib_test",
            fio::PERM_READABLE,
            zx::MonotonicInstant::INFINITE,
        )?;
        assert!(match description.kind {
            NodeKind::File => true,
            _ => false,
        });
        Ok(())
    }

    #[fuchsia::test]
    async fn test_directory_open_vmo() -> Result<(), Error> {
        let pkg = open_pkg();
        let vmo = directory_open_vmo(
            &pkg,
            "bin/syncio_lib_test",
            fio::VmoFlags::READ | fio::VmoFlags::EXECUTE,
            zx::MonotonicInstant::INFINITE,
        )?;
        assert!(!vmo.is_invalid());

        let info = vmo.basic_info()?;
        assert_eq!(zx::Rights::READ, info.rights & zx::Rights::READ);
        assert_eq!(zx::Rights::EXECUTE, info.rights & zx::Rights::EXECUTE);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_directory_read_file() -> Result<(), Error> {
        let pkg = open_pkg();
        let data =
            directory_read_file(&pkg, "bin/syncio_lib_test", zx::MonotonicInstant::INFINITE)?;

        assert!(!data.is_empty());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_directory_open_directory_async() -> Result<(), Error> {
        let pkg = open_pkg();
        let bin =
            directory_open_directory_async(&pkg, "bin", fio::PERM_READABLE | fio::PERM_EXECUTABLE)?;
        let vmo = directory_open_vmo(
            &bin,
            "syncio_lib_test",
            fio::VmoFlags::READ | fio::VmoFlags::EXECUTE,
            zx::MonotonicInstant::INFINITE,
        )?;
        assert!(!vmo.is_invalid());

        let info = vmo.basic_info()?;
        assert_eq!(zx::Rights::READ, info.rights & zx::Rights::READ);
        assert_eq!(zx::Rights::EXECUTE, info.rights & zx::Rights::EXECUTE);
        Ok(())
    }

    #[fuchsia::test]
    async fn test_directory_open_zxio_async() -> Result<(), Error> {
        let pkg_proxy =
            directory::open_in_namespace("/pkg", fio::PERM_READABLE | fio::PERM_EXECUTABLE)
                .expect("failed to open /pkg");
        let zx_channel = pkg_proxy
            .into_channel()
            .expect("failed to convert proxy into channel")
            .into_zx_channel();
        let storage = zxio::zxio_storage_t::default();
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let status = unsafe {
            zxio::zxio_create(
                zx_channel.into_raw(),
                &storage as *const zxio::zxio_storage_t as *mut zxio::zxio_storage_t,
            )
        };
        assert_eq!(status, zx::sys::ZX_OK);
        let io = &storage.io as *const zxio::zxio_t as *mut zxio::zxio_t;
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let close_status = unsafe { zxio::zxio_close(io) };
        assert_eq!(close_status, zx::sys::ZX_OK);
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            zxio::zxio_destroy(io);
        }
        Ok(())
    }

    #[fuchsia::test]
    async fn test_directory_enumerate() -> Result<(), Error> {
        let pkg_dir_handle =
            directory::open_in_namespace("/pkg", fio::PERM_READABLE | fio::PERM_EXECUTABLE)
                .expect("failed to open /pkg")
                .into_channel()
                .expect("could not unwrap channel")
                .into_zx_channel()
                .into();

        let io: Zxio = Zxio::create(pkg_dir_handle)?;
        let iter = io.create_dirent_iterator().expect("failed to create iterator");
        let expected_dir_names = vec![".", "bin", "lib", "meta"];
        let mut found_dir_names = iter
            .map(|e| {
                let dirent = e.expect("dirent");
                assert!(dirent.is_dir());
                std::str::from_utf8(&dirent.name).expect("name was not valid utf8").to_string()
            })
            .collect::<Vec<_>>();
        found_dir_names.sort();
        assert_eq!(expected_dir_names, found_dir_names);

        // Check all entry inside bin are either "." or a file
        let bin_io = io
            .open("bin", fio::PERM_READABLE | fio::PERM_EXECUTABLE, Default::default())
            .expect("open");
        for entry in bin_io.create_dirent_iterator().expect("failed to create iterator") {
            let dirent = entry.expect("dirent");
            if dirent.name == "." {
                assert!(dirent.is_dir());
            } else {
                assert!(dirent.is_file());
            }
        }

        Ok(())
    }

    #[fuchsia::test]
    fn test_storage_allocator() {
        let mut out_storage = zxio_storage_t::default();
        let mut out_storage_ptr = &mut out_storage as *mut zxio_storage_t;

        let mut out_context = Zxio::default();
        let mut out_context_ptr = &mut out_context as *mut Zxio;

        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let out = unsafe {
            storage_allocator(
                0 as zxio_object_type_t,
                &mut out_storage_ptr as *mut *mut zxio_storage_t,
                &mut out_context_ptr as *mut *mut Zxio as *mut *mut c_void,
            )
        };
        assert_eq!(out, zx::sys::ZX_OK);
    }

    #[fuchsia::test]
    fn test_storage_allocator_bad_context() {
        let mut out_storage = zxio_storage_t::default();
        let mut out_storage_ptr = &mut out_storage as *mut zxio_storage_t;

        let out_context = std::ptr::null_mut();

        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        let out = unsafe {
            storage_allocator(
                0 as zxio_object_type_t,
                &mut out_storage_ptr as *mut *mut zxio_storage_t,
                out_context,
            )
        };
        assert_eq!(out, zx::sys::ZX_ERR_NO_MEMORY);
    }

    #[fuchsia::test]
    fn test_parse_ip_pktinfo_control_message() {
        let pktinfo = zxio::in_pktinfo {
            ipi_ifindex: 1,
            ipi_spec_dst: zxio::in_addr { s_addr: u32::from_ne_bytes([192, 0, 2, 1]) },
            ipi_addr: zxio::in_addr { s_addr: u32::from_ne_bytes([192, 0, 2, 2]) },
        };
        let total_size = CMSG_HEADER_SIZE + size_of_val(&pktinfo);
        let header = zxio::cmsghdr {
            cmsg_len: total_size as c_uint,
            cmsg_level: zxio::SOL_IP as i32,
            cmsg_type: zxio::IP_PKTINFO as i32,
        };
        let mut out = vec![0u8; total_size];
        header.write_to_prefix(&mut out[..]).unwrap();
        pktinfo.write_to_prefix(&mut out[CMSG_HEADER_SIZE..]).unwrap();
        let parsed = parse_control_messages(&out);
        assert_eq!(
            parsed,
            vec![ControlMessage::IpPacketInfo {
                iface: 1,
                local_addr: [192, 0, 2, 1],
                header_destination_addr: [192, 0, 2, 2],
            }]
        );
    }
}
