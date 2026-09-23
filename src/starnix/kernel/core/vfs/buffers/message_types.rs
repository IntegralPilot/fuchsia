// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::VmsplicePayload;
use crate::security;
use crate::task::CurrentTask;
use crate::vfs::buffers::{InputBuffer, InputBufferExt as _, OutputBuffer};
use crate::vfs::socket::syscalls::CMsgHdrPtr;
use crate::vfs::socket::{SocketAddress, SocketMessageFlags};
use crate::vfs::{FdFlags, FdNumber, FileHandle, FsString};
use byteorder::{ByteOrder, NativeEndian};
use starnix_uapi::auth::{CAP_SETGID, CAP_SETUID, CAP_SYS_ADMIN};
use starnix_uapi::errors::Errno;

use starnix_uapi::user_address::{ArchSpecific, MultiArchUserRef};
use starnix_uapi::{
    IP_PKTINFO, IP_RECVORIGDSTADDR, IP_TOS, IP_TTL, IPV6_HOPLIMIT, IPV6_PKTINFO, IPV6_TCLASS,
    SCM_CREDENTIALS, SCM_RIGHTS, SCM_SECURITY, SO_TIMESTAMP, SO_TIMESTAMPNS, SOL_IP, SOL_IPV6,
    SOL_SOCKET, c_int, errno, error, uapi,
};
use std::fmt::Debug;
use std::sync::Arc;
use zerocopy::{FromBytes, IntoBytes};

uapi::check_arch_independent_layout! {
    in_pktinfo {
        ipi_ifindex,
        ipi_spec_dst,
        ipi_addr,
    }
    in6_pktinfo {
        ipi6_addr,
        ipi6_ifindex,
    }
}

pub type UcredPtr = MultiArchUserRef<uapi::ucred, uapi::arch32::ucred>;

/// A `Message` represents a typed segment of bytes within a `MessageQueue`.
#[derive(Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub struct Message<D: MessageData = Vec<u8>> {
    /// The data contained in the message.
    pub data: D,

    /// The address from which the message was sent.
    pub address: Option<SocketAddress>,

    /// The ancillary data that is associated with this message.
    pub ancillary_data: Vec<AncillaryData>,
}

impl<D: MessageData> Message<D> {
    /// Creates a a new message with the provided message and ancillary data.
    pub fn new(
        data: D,
        address: Option<SocketAddress>,
        ancillary_data: Vec<AncillaryData>,
    ) -> Self {
        Message { data, address, ancillary_data }
    }

    /// Returns the length of the message in bytes.
    ///
    /// Note that ancillary data does not contribute to the length of the message.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn clone_at_most(&self, limit: usize) -> Self {
        Message {
            data: self.data.clone_at_most(limit),
            address: self.address.clone(),
            ancillary_data: self.ancillary_data.clone(),
        }
    }

    /// Shortens the message data, keeping the first `limit` elements and dropping the rest.
    ///
    /// If `limit` is greater or equal to the current data length, this has no effect.
    pub fn truncate(&mut self, limit: usize) {
        self.data.truncate(limit);
    }
}

impl<D: MessageData> From<D> for Message<D> {
    fn from(data: D) -> Self {
        Message { data, address: None, ancillary_data: Vec::new() }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ControlMsg {
    pub level: u32,
    pub r#type: u32,
    pub data: Vec<u8>,
}

impl ControlMsg {
    pub fn new(level: u32, r#type: u32, data: Vec<u8>) -> Self {
        Self { level, r#type, data }
    }

    fn truncate(&mut self, len: usize) {
        self.data.truncate(len)
    }

    fn into_bytes<A: ArchSpecific>(self, a: &A) -> Result<Vec<u8>, ()> {
        let ControlMsg { level, r#type, mut data } = self;
        let header_size = CMsgHdrPtr::size_of_object_for(a);
        let cmsg_len = (header_size + data.len()) as u64;
        let mut bytes = CMsgHdrPtr::into_bytes(
            a,
            uapi::cmsghdr { cmsg_len, cmsg_level: level, cmsg_type: r#type },
        )?;
        bytes.append(&mut data);
        return Ok(bytes);
    }
}

/// `AncillaryData` converts a `cmsghdr` into a representation suitable for passing around
/// inside of starnix. In AF_UNIX/SCM_RIGHTS, for example, the file descrpitors will be turned
/// into `FileHandle`s that can be sent to other tasks.
///
/// An `AncillaryData` instance can be converted back into a `cmsghdr`. At that point the contained
/// objects will be converted back to what can be stored in a `cmsghdr`. File handles, for example,
/// will be added to the reading task's files and the associated file descriptors will be stored in
///  the `cmsghdr`.
#[derive(Clone, Debug)]
#[cfg_attr(test, derive(PartialEq))]
pub enum AncillaryData {
    Unix(UnixControlData),

    // SOL_IP and SOL_IPV6.
    Ip(syncio::ControlMessage),
}

// Reads int `cmsg` value and tries to convert it to u8.
fn read_u8_value_from_int_cmsg(data: &[u8]) -> Option<u8> {
    u8::try_from(c_int::read_from_prefix(data).ok()?.0).ok()
}

pub struct AncillaryDataConvertedToBytes {
    pub bytes: Vec<u8>,
    /// Indicates the message was shortened due to insufficient buffer space,
    /// not due to permission checks
    pub truncated: bool,
}

impl AncillaryData {
    /// Creates a new `AncillaryData` instance representing the data in `message`.
    ///
    /// # Parameters
    /// - `current_task`: The current task. Used to interpret SCM_RIGHTS messages.
    /// - `message`: The message header to parse.
    pub fn from_cmsg(current_task: &CurrentTask, message: ControlMsg) -> Result<Self, Errno> {
        match (message.level, message.r#type) {
            (SOL_SOCKET, SCM_RIGHTS | SCM_CREDENTIALS) => {
                Ok(AncillaryData::Unix(UnixControlData::new(current_task, message)?))
            }
            (SOL_IP, IP_TOS) => Ok(AncillaryData::Ip(syncio::ControlMessage::IpTos(
                u8::read_from_prefix(&message.data[..]).map_err(|_| errno!(EINVAL))?.0,
            ))),
            (SOL_IP, IP_TTL) => Ok(AncillaryData::Ip(syncio::ControlMessage::IpTtl(
                read_u8_value_from_int_cmsg(&message.data).ok_or_else(|| errno!(EINVAL))?,
            ))),
            (SOL_IP, IP_RECVORIGDSTADDR) => {
                Ok(AncillaryData::Ip(syncio::ControlMessage::IpRecvOrigDstAddr(
                    <[u8; size_of::<uapi::sockaddr_in>()]>::read_from_prefix(&message.data[..])
                        .map_err(|_| errno!(EINVAL))?
                        .0,
                )))
            }
            (SOL_IP, IP_PKTINFO) => {
                let (pktinfo, _) = uapi::in_pktinfo::read_from_prefix(&message.data[..])
                    .map_err(|_| errno!(EINVAL))?;
                Ok(AncillaryData::Ip(syncio::ControlMessage::IpPacketInfo {
                    iface: pktinfo.ipi_ifindex,
                    local_addr: pktinfo.ipi_spec_dst.s_addr.to_ne_bytes(),
                    header_destination_addr: pktinfo.ipi_addr.s_addr.to_ne_bytes(),
                }))
            }
            (SOL_IPV6, IPV6_TCLASS) => Ok(AncillaryData::Ip(syncio::ControlMessage::Ipv6Tclass(
                read_u8_value_from_int_cmsg(&message.data).ok_or_else(|| errno!(EINVAL))?,
            ))),
            (SOL_IPV6, IPV6_HOPLIMIT) => {
                Ok(AncillaryData::Ip(syncio::ControlMessage::Ipv6HopLimit(
                    read_u8_value_from_int_cmsg(&message.data).ok_or_else(|| errno!(EINVAL))?,
                )))
            }
            (SOL_IPV6, IPV6_PKTINFO) => {
                let (pktinfo, _) = uapi::in6_pktinfo::read_from_prefix(&message.data[..])
                    .map_err(|_| errno!(EINVAL))?;
                Ok(AncillaryData::Ip(syncio::ControlMessage::Ipv6PacketInfo {
                    local_addr: pktinfo
                        .ipi6_addr
                        .as_bytes()
                        .try_into()
                        .ok()
                        .ok_or_else(|| errno!(EINVAL))?,
                    iface: pktinfo.ipi6_ifindex as u32,
                }))
            }
            (level, type_) => {
                error!(EINVAL, format!("invalid cmsg_level/type: 0x{:x}/0x{:x}", level, type_))
            }
        }
    }

    pub fn into_controlmsg(
        self,
        current_task: &CurrentTask,
        flags: SocketMessageFlags,
    ) -> Result<Option<ControlMsg>, Errno> {
        match self {
            AncillaryData::Unix(control) => control.into_controlmsg(current_task, flags),
            AncillaryData::Ip(syncio::ControlMessage::IpTos(value)) => {
                Ok(Some(ControlMsg::new(SOL_IP, IP_TOS, value.as_bytes().to_vec())))
            }
            AncillaryData::Ip(syncio::ControlMessage::IpTtl(value)) => {
                Ok(Some(ControlMsg::new(SOL_IP, IP_TTL, (value as c_int).as_bytes().to_vec())))
            }
            AncillaryData::Ip(syncio::ControlMessage::IpRecvOrigDstAddr(value)) => {
                Ok(Some(ControlMsg::new(SOL_IP, IP_RECVORIGDSTADDR, value.as_bytes().to_vec())))
            }
            AncillaryData::Ip(syncio::ControlMessage::Ipv6Tclass(value)) => Ok(Some(
                ControlMsg::new(SOL_IPV6, IPV6_TCLASS, (value as c_int).as_bytes().to_vec()),
            )),
            AncillaryData::Ip(syncio::ControlMessage::Ipv6HopLimit(value)) => Ok(Some(
                ControlMsg::new(SOL_IPV6, IPV6_HOPLIMIT, (value as c_int).as_bytes().to_vec()),
            )),
            AncillaryData::Ip(syncio::ControlMessage::IpPacketInfo {
                iface,
                local_addr,
                header_destination_addr,
            }) => {
                let pktinfo = uapi::in_pktinfo {
                    ipi_ifindex: iface,
                    ipi_spec_dst: uapi::in_addr { s_addr: u32::from_ne_bytes(local_addr) },
                    ipi_addr: uapi::in_addr { s_addr: u32::from_ne_bytes(header_destination_addr) },
                };
                Ok(Some(ControlMsg::new(SOL_IP, IP_PKTINFO, pktinfo.as_bytes().to_vec())))
            }
            AncillaryData::Ip(syncio::ControlMessage::Ipv6PacketInfo { iface, local_addr }) => {
                let pktinfo = uapi::in6_pktinfo {
                    ipi6_addr: uapi::in6_addr {
                        in6_u: uapi::in6_addr__bindgen_ty_1 { u6_addr8: local_addr },
                    },
                    ipi6_ifindex: iface as i32,
                };
                Ok(Some(ControlMsg::new(SOL_IPV6, IPV6_PKTINFO, pktinfo.as_bytes().to_vec())))
            }
            AncillaryData::Ip(syncio::ControlMessage::Timestamp { sec, usec }) => {
                let time = uapi::timeval { tv_sec: sec, tv_usec: usec };
                Ok(Some(ControlMsg::new(SOL_SOCKET, SO_TIMESTAMP, time.as_bytes().to_vec())))
            }
            AncillaryData::Ip(syncio::ControlMessage::TimestampNs { sec, nsec }) => {
                let time = uapi::timespec { tv_sec: sec, tv_nsec: nsec };
                Ok(Some(ControlMsg::new(SOL_SOCKET, SO_TIMESTAMPNS, time.as_bytes().to_vec())))
            }
        }
    }

    /// Returns the total size of all data in this message.
    pub fn total_size(&self, current_task: &CurrentTask) -> usize {
        match self {
            AncillaryData::Unix(control) => control.total_size(current_task),
            AncillaryData::Ip(msg) => msg.get_data_size(),
        }
    }

    /// Returns the minimum size that can fit some amount of this message's data.
    pub fn minimum_size(&self) -> usize {
        match self {
            AncillaryData::Unix(control) => control.minimum_size(),
            AncillaryData::Ip(msg) => msg.get_data_size(),
        }
    }

    /// Convert the message into bytes, truncating it if it exceeds the available space.
    pub fn into_bytes(
        self,
        current_task: &CurrentTask,
        flags: SocketMessageFlags,
        space_available: usize,
    ) -> Result<AncillaryDataConvertedToBytes, Errno> {
        let header_size = CMsgHdrPtr::size_of_object_for(current_task);
        let minimum_data_size = self.minimum_size();

        // If there is not enough space available to fit the header, return an empty vector
        // instead of a partial header.
        if space_available < header_size + minimum_data_size {
            return Ok(AncillaryDataConvertedToBytes { bytes: vec![], truncated: true });
        }

        let max_data_bytes = space_available - header_size;
        // Truncate file descriptors to the number that fit in the remaining buffer space
        // so that into_controlmsg() only installs fds the receiver can actually see
        let (limited, space_truncated) = match self {
            AncillaryData::Unix(UnixControlData::Rights(mut files)) => {
                let max_fds = max_data_bytes / std::mem::size_of::<FdNumber>();
                let truncated = files.len() > max_fds;
                files.truncate(max_fds);
                (AncillaryData::Unix(UnixControlData::Rights(files)), truncated)
            }
            other => (other, false),
        };
        let cmsg = limited.into_controlmsg(current_task, flags)?;
        if let Some(mut cmsg) = cmsg {
            let data_truncated = cmsg.data.len() > max_data_bytes;
            cmsg.truncate(max_data_bytes);
            let bytes = cmsg.into_bytes(current_task).map_err(|_| errno!(EINVAL))?;
            Ok(AncillaryDataConvertedToBytes {
                bytes,
                truncated: space_truncated || data_truncated,
            })
        } else {
            Ok(AncillaryDataConvertedToBytes { bytes: vec![], truncated: space_truncated })
        }
    }
}

/// A control message for a Unix domain socket.
#[derive(Clone, Debug)]
pub enum UnixControlData {
    /// "Send or receive a set of open file descriptors from another process. The data portion
    /// contains an integer array of the file descriptors."
    ///
    /// See https://man7.org/linux/man-pages/man7/unix.7.html.
    Rights(Vec<FileHandle>),

    /// "Send or receive UNIX credentials.  This can be used for authentication. The credentials are
    /// passed as a struct ucred ancillary message."
    ///
    /// See https://man7.org/linux/man-pages/man7/unix.7.html.
    Credentials(uapi::ucred),

    /// "Receive the SELinux security context (the security label) of the peer socket. The received
    /// ancillary data is a null-terminated string containing the security context."
    ///
    /// See https://man7.org/linux/man-pages/man7/unix.7.html.
    Security(FsString),
}

/// `UnixControlData` cannot derive `PartialEq` due to `Rights` containing file handles.
///
/// This implementation only compares the number of files, not the actual files. The equality
/// should only be used for testing.
#[cfg(test)]
impl PartialEq for UnixControlData {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (UnixControlData::Rights(self_files), UnixControlData::Rights(other_files)) => {
                self_files.len() == other_files.len()
            }
            (
                UnixControlData::Credentials(self_credentials),
                UnixControlData::Credentials(other_credentials),
            ) => self_credentials == other_credentials,
            (
                UnixControlData::Security(self_security),
                UnixControlData::Security(other_security),
            ) => self_security == other_security,
            _ => false,
        }
    }
}

impl UnixControlData {
    /// Creates a new `UnixControlData` instance for the provided `message_header`. This includes
    /// reading the associated data from the `task` (e.g., files from file descriptors).
    pub fn new(current_task: &CurrentTask, message: ControlMsg) -> Result<Self, Errno> {
        match message.r#type {
            SCM_RIGHTS => {
                // Compute the number of file descriptors that fit in the provided bytes.
                let bytes_per_file_descriptor = std::mem::size_of::<FdNumber>();
                let num_file_descriptors = message.data.len() / bytes_per_file_descriptor;

                // Get the files associated with the provided file descriptors.
                let files = (0..num_file_descriptors * bytes_per_file_descriptor)
                    .step_by(bytes_per_file_descriptor)
                    .map(|index| NativeEndian::read_i32(&message.data[index..]))
                    // O_PATH allowed for:
                    //
                    //   Passing the file descriptor to another process via a
                    //   UNIX domain socket (see SCM_RIGHTS in unix(7)).
                    //
                    // See https://man7.org/linux/man-pages/man2/open.2.html
                    .map(|fd| current_task.files().get_allowing_opath(FdNumber::from_raw(fd)))
                    .collect::<Result<Vec<FileHandle>, Errno>>()?;

                Ok(UnixControlData::Rights(files))
            }
            SCM_CREDENTIALS => {
                // A privileged process is allowed to specify
                // values that do not match its own. The sender must specify
                // its own process ID (unless it has the capability
                // CAP_SYS_ADMIN, in which case the PID of any existing
                // process may be specified), its real user ID, effective user
                // ID, or saved set-user-ID (unless it has CAP_SETUID), and
                // its real group ID, effective group ID, or saved set-group-
                // ID (unless it has CAP_SETGID).
                //
                // See https://man7.org/linux/man-pages/man7/unix.7.html

                if message.data.len() < UcredPtr::size_of_object_for(current_task) {
                    return error!(EINVAL);
                }

                let credentials = UcredPtr::read_from_prefix(current_task, &message.data)
                    .map_err(|_| errno!(EINVAL))?;

                let task_creds = current_task.current_creds();
                let actual_pid = current_task.get_pid();

                // Validate pid: must match sender's PID, or sender has CAP_SYS_ADMIN
                if credentials.pid != actual_pid {
                    security::check_task_capable(current_task, CAP_SYS_ADMIN)?;
                    current_task.get_task(credentials.pid)?.running_state()?;
                }
                // Validate uid: must match sender's uid/euid/suid, or sender has CAP_SETUID
                if credentials.uid != task_creds.uid
                    && credentials.uid != task_creds.euid
                    && credentials.uid != task_creds.saved_uid
                {
                    security::check_task_capable(current_task, CAP_SETUID)?;
                }
                // Validate gid: must match sender's gid/egid/sgid, or sender has CAP_SETGID
                if credentials.gid != task_creds.gid
                    && credentials.gid != task_creds.egid
                    && credentials.gid != task_creds.saved_gid
                {
                    security::check_task_capable(current_task, CAP_SETGID)?;
                }

                Ok(UnixControlData::Credentials(credentials))
            }
            SCM_SECURITY => Ok(UnixControlData::Security(message.data.into())),
            _ => error!(EINVAL),
        }
    }

    /// Returns a `UnixControlData` message that can be used when passcred is enabled but no
    /// credentials were sent.
    pub fn unknown_creds() -> Self {
        const NOBODY: u32 = 65534;
        let credentials = uapi::ucred { pid: 0, uid: NOBODY, gid: NOBODY };
        UnixControlData::Credentials(credentials)
    }

    /// Constructs a ControlMsg for this control data, with a destination of `task`.
    ///
    /// The provided `task` is used to create any required file descriptors, etc.
    pub fn into_controlmsg(
        self,
        current_task: &CurrentTask,
        flags: SocketMessageFlags,
    ) -> Result<Option<ControlMsg>, Errno> {
        let (msg_type, data) = match self {
            UnixControlData::Rights(files) => {
                let flags = if flags.contains(SocketMessageFlags::CMSG_CLOEXEC) {
                    FdFlags::CLOEXEC
                } else {
                    FdFlags::empty()
                };
                let mut fds = Vec::with_capacity(files.len());
                for file in files {
                    // Truncate the message as soon as a file descriptor is not allowed to be
                    // received.
                    if security::file_receive(current_task, &file).is_err() {
                        break;
                    }
                    let fd = current_task.add_file(file, flags)?;
                    fds.push(fd);
                }
                // If no file descriptors are left, the entirety of the control message should be
                // dropped.
                if fds.is_empty() {
                    return Ok(None);
                }
                (SCM_RIGHTS, fds.as_bytes().to_owned())
            }
            UnixControlData::Credentials(credentials) => (
                SCM_CREDENTIALS,
                UcredPtr::into_bytes(current_task, credentials).map_err(|_| errno!(EINVAL))?,
            ),
            UnixControlData::Security(string) => (SCM_SECURITY, string.as_bytes().to_owned()),
        };
        Ok(Some(ControlMsg::new(SOL_SOCKET, msg_type, data)))
    }

    /// Returns the total size of all data in this message.
    pub fn total_size(&self, current_task: &CurrentTask) -> usize {
        match self {
            UnixControlData::Rights(files) => files.len() * std::mem::size_of::<FdNumber>(),
            UnixControlData::Credentials(_credentials) => {
                UcredPtr::size_of_object_for(current_task)
            }
            UnixControlData::Security(string) => string.len(),
        }
    }

    /// Returns the minimum size that can fit some amount of this message's data. For example, the
    /// minimum size for an SCM_RIGHTS message is the size of a single FD. If the buffer is large
    /// enough for the minimum size but too small for the total size, the message is truncated and
    /// the MSG_CTRUNC flag is set.
    pub fn minimum_size(&self) -> usize {
        match self {
            UnixControlData::Rights(_files) => std::mem::size_of::<FdNumber>(),
            UnixControlData::Credentials(_credentials) => 0,
            UnixControlData::Security(string) => string.len(),
        }
    }
}

/// Stores an arbitrary sequence of bytes.
pub trait MessageData: Sized + Debug {
    /// Copies data from user memory into a new MessageData object.
    fn copy_from_user(data: &mut dyn InputBuffer, limit: usize) -> Result<Self, Errno>;

    /// Returns a pointer to this data.
    fn ptr(&self) -> Result<*const u8, Errno>;

    /// Calls the provided function with this data's bytes.
    fn with_bytes<O, F: FnMut(&[u8]) -> Result<O, Errno>>(&self, f: F) -> Result<O, Errno>;

    /// Returns the number of bytes in the message.
    fn len(&self) -> usize;

    /// Splits the message data at `index`.
    ///
    /// After this call returns, at most `at` bytes will be stored in this `MessageData`, and any
    /// remaining bytes will be moved to the returned `MessageData`.
    fn split_off(&mut self, index: usize) -> Option<Self>;

    /// Copies the message out to the output buffer.
    ///
    /// Returns the number of bytes that were read into the buffer.
    fn copy_to_user(&self, data: &mut dyn OutputBuffer) -> Result<usize, Errno>;

    /// Clones this data up to a limit.
    fn clone_at_most(&self, limit: usize) -> Self;

    /// Truncates this data to the specified limit.
    ///
    /// Does nothing if the limit is larger than the data's size.
    fn truncate(&mut self, limit: usize);
}

impl MessageData for Vec<u8> {
    fn copy_from_user(data: &mut dyn InputBuffer, limit: usize) -> Result<Self, Errno> {
        data.read_to_vec_exact(limit)
    }

    fn ptr(&self) -> Result<*const u8, Errno> {
        Ok(self.as_ptr())
    }

    fn with_bytes<O, F: FnMut(&[u8]) -> Result<O, Errno>>(&self, mut f: F) -> Result<O, Errno> {
        f(self)
    }

    fn len(&self) -> usize {
        Vec::<u8>::len(self)
    }

    fn split_off(&mut self, index: usize) -> Option<Self> {
        if index < self.len() {
            let bytes = self.split_off(index);
            Some(bytes)
        } else {
            None
        }
    }

    fn copy_to_user(&self, data: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        data.write(self.as_ref())
    }

    fn clone_at_most(&self, limit: usize) -> Self {
        self[0..std::cmp::min(self.len(), limit)].to_vec()
    }

    fn truncate(&mut self, limit: usize) {
        self.truncate(limit)
    }
}

#[derive(Debug)]
pub enum PipeMessageData {
    Owned(Vec<u8>),
    Vmspliced(Arc<VmsplicePayload>),
}

impl From<Vec<u8>> for PipeMessageData {
    fn from(v: Vec<u8>) -> Self {
        Self::Owned(v)
    }
}

impl MessageData for PipeMessageData {
    fn copy_from_user(data: &mut dyn InputBuffer, limit: usize) -> Result<Self, Errno> {
        MessageData::copy_from_user(data, limit).map(Self::Owned)
    }

    fn ptr(&self) -> Result<*const u8, Errno> {
        match self {
            Self::Owned(d) => MessageData::ptr(d),
            Self::Vmspliced(d) => MessageData::ptr(d),
        }
    }

    fn with_bytes<O, F: FnMut(&[u8]) -> Result<O, Errno>>(&self, f: F) -> Result<O, Errno> {
        match self {
            Self::Owned(d) => MessageData::with_bytes(d, f),
            Self::Vmspliced(d) => MessageData::with_bytes(d, f),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Owned(d) => MessageData::len(d),
            Self::Vmspliced(d) => MessageData::len(d),
        }
    }

    fn split_off(&mut self, index: usize) -> Option<Self> {
        match self {
            Self::Owned(d) => MessageData::split_off(d, index).map(Self::Owned),
            Self::Vmspliced(d) => MessageData::split_off(d, index).map(Self::Vmspliced),
        }
    }

    fn copy_to_user(&self, data: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        match self {
            Self::Owned(d) => MessageData::copy_to_user(d, data),
            Self::Vmspliced(d) => MessageData::copy_to_user(d, data),
        }
    }

    fn clone_at_most(&self, limit: usize) -> Self {
        match self {
            Self::Owned(d) => Self::Owned(MessageData::clone_at_most(d, limit)),
            Self::Vmspliced(d) => Self::Vmspliced(MessageData::clone_at_most(d, limit)),
        }
    }

    fn truncate(&mut self, limit: usize) {
        match self {
            Self::Owned(d) => MessageData::truncate(d, limit),
            Self::Vmspliced(d) => MessageData::truncate(d, limit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::spawn_kernel_with_selinux_and_run;
    use starnix_uapi::device_id::DeviceId;
    use starnix_uapi::file_mode::FileMode;
    use starnix_uapi::open_flags::OpenFlags;

    fn open_file(current_task: &CurrentTask, name: &str) -> FileHandle {
        current_task
            .fs()
            .root()
            .create_node(&current_task, name.into(), FileMode::IFREG, DeviceId::NONE)
            .expect("create_node")
            .open(current_task, OpenFlags::RDWR)
            .expect("open")
    }

    fn make_rights(current_task: &CurrentTask, n: usize) -> AncillaryData {
        let files: Vec<_> = (0..n).map(|i| open_file(current_task, &format!("file{i}"))).collect();
        AncillaryData::Unix(UnixControlData::Rights(files))
    }

    // AncillaryData to bytes conversion does not reflect permission denials in the
    // `truncated` member.
    #[::fuchsia::test]
    async fn into_bytes_permission_denial_no_truncation() {
        spawn_kernel_with_selinux_and_run(async |current_task, security_server| {
            // Create and open the files before loading the policy: the test policy
            // does not allow `kernel_t` to `open` `unlabeled_file_t` files, so they
            // must already be open before enforcement begins. The `file_receive`
            // check in `into_bytes()` below then runs against the enforcing policy
            // and denies the transfer.
            let data = make_rights(current_task, 2);

            let policy_bytes = include_bytes!(
                "../../../../lib/selinux/testdata/micro_policies/hooks_tests_policy"
            )
            .to_vec();
            security_server.set_enforcing(true);
            security_server.load_policy(policy_bytes).expect("policy load failed");
            security::selinuxfs_policy_loaded(current_task);

            let data_size = data.total_size(current_task);
            let header_size = CMsgHdrPtr::size_of_object_for(current_task);
            let space_available = header_size + data_size;
            let result = data
                .into_bytes(current_task, SocketMessageFlags::empty(), space_available)
                .expect("into_bytes");
            assert!(!result.truncated);
            assert!(result.bytes.is_empty());
        })
        .await;
    }
}
