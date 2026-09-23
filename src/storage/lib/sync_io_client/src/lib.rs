// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This library provides a synchronous wrapper around fuchsia.io.  It is usually better to use the
//! Rust standard library for this (which will use fdio and zxio).  The primary user of this library
//! is Starnix which uses this for performance and memory reasons.

use fidl::endpoints::SynchronousProxy;
use fidl_fuchsia_io as fio;
use fuchsia_sync::Mutex;
use smallvec::SmallVec;
use std::ops::{ControlFlow, Range};
use syncio::{AllocateMode, zxio_fsverity_descriptor_t, zxio_node_attributes_t};
use zerocopy::FromBytes;

/// Converts FIDL attributes to `zxio_node_attributes_t`.
///
/// NOTE: This does not work for _all_ attributes e.g. fsverity options and root hash.
fn zxio_attr_from_fidl(
    mutable: &fio::MutableNodeAttributes,
    immutable: &fio::ImmutableNodeAttributes,
) -> zxio_node_attributes_t {
    let mut out_attr = zxio_node_attributes_t::default();
    if let Some(protocols) = immutable.protocols {
        out_attr.protocols = protocols.bits();
        out_attr.has.protocols = true;
    }
    if let Some(abilities) = immutable.abilities {
        out_attr.abilities = abilities.bits();
        out_attr.has.abilities = true;
    }
    if let Some(id) = immutable.id {
        out_attr.id = id;
        out_attr.has.id = true;
    }
    if let Some(content_size) = immutable.content_size {
        out_attr.content_size = content_size;
        out_attr.has.content_size = true;
    }
    if let Some(storage_size) = immutable.storage_size {
        out_attr.storage_size = storage_size;
        out_attr.has.storage_size = true;
    }
    if let Some(link_count) = immutable.link_count {
        out_attr.link_count = link_count;
        out_attr.has.link_count = true;
    }
    if let Some(creation_time) = mutable.creation_time {
        out_attr.creation_time = creation_time;
        out_attr.has.creation_time = true;
    }
    if let Some(modification_time) = mutable.modification_time {
        out_attr.modification_time = modification_time;
        out_attr.has.modification_time = true;
    }
    if let Some(access_time) = mutable.access_time {
        out_attr.access_time = access_time;
        out_attr.has.access_time = true;
    }
    if let Some(mode) = mutable.mode {
        out_attr.mode = mode;
        out_attr.has.mode = true;
    }
    if let Some(uid) = mutable.uid {
        out_attr.uid = uid;
        out_attr.has.uid = true;
    }
    if let Some(gid) = mutable.gid {
        out_attr.gid = gid;
        out_attr.has.gid = true;
    }
    if let Some(rdev) = mutable.rdev {
        out_attr.rdev = rdev;
        out_attr.has.rdev = true;
    }
    if let Some(change_time) = immutable.change_time {
        out_attr.change_time = change_time;
        out_attr.has.change_time = true;
    }
    if let Some(casefold) = mutable.casefold {
        out_attr.casefold = casefold;
        out_attr.has.casefold = true;
    }
    if let Some(verity_enabled) = immutable.verity_enabled {
        out_attr.fsverity_enabled = verity_enabled;
        out_attr.has.fsverity_enabled = true;
    }
    if let Some(wrapping_key_id) = mutable.wrapping_key_id {
        out_attr.wrapping_key_id = wrapping_key_id;
        out_attr.has.wrapping_key_id = true;
    }
    out_attr
}

/// This trait is used so that clients can create their own wrappers around types exposed here.
pub trait Factory {
    type Result;

    fn create_node(self, io: RemoteIo, info: fio::NodeInfo) -> Self::Result;
    fn create_directory(self, io: RemoteIo, info: fio::DirectoryInfo) -> Self::Result;
    fn create_file(self, io: RemoteIo, info: fio::FileInfo) -> Self::Result;
    fn create_symlink(self, io: RemoteIo, info: fio::SymlinkInfo) -> Self::Result;
}

/// Waits for the fuchsia.io `OnRepresentation` event and then uses the factory to create an an
/// appropriate object.  This returns attributes (as requested by the corresponding `open` call)
/// that are present in the `OnRepresentation` event.
///
/// NOTE: The attributes returned are not comprehensive.  Check `zxio_attr_from_fidl` above for
/// supported attributes.
pub fn create_with_on_representation<F: Factory>(
    proxy: fio::NodeSynchronousProxy,
    factory: F,
) -> Result<F::Result, zx::Status> {
    match proxy.wait_for_event(zx::MonotonicInstant::INFINITE) {
        Ok(fio::NodeEvent::OnRepresentation { payload }) => match payload {
            fio::Representation::Node(info) => Ok(factory.create_node(RemoteIo::new(proxy), info)),
            fio::Representation::Directory(info) => {
                Ok(factory.create_directory(RemoteIo::new(proxy), info))
            }
            fio::Representation::File(mut info) => {
                let io = RemoteIo {
                    proxy,
                    stream: info
                        .stream
                        .take()
                        .map(zx::Stream::from)
                        .unwrap_or_else(|| zx::NullableHandle::invalid().into()),
                };
                Ok(factory.create_file(io, info))
            }
            fio::Representation::Symlink(info) => {
                Ok(factory.create_symlink(RemoteIo::new(proxy), info))
            }
            _ => Err(zx::Status::NOT_SUPPORTED),
        },
        Err(fidl::Error::ClientChannelClosed { epitaph, .. }) => match epitaph.into() {
            Err(status) => Err(status),
            Ok(()) => Err(zx::Status::PEER_CLOSED),
        },
        _ => Err(zx::Status::IO),
    }
}

/// Wraps a proxy and optional stream and provides wrappers around most fuchsia.io methods.
///
/// NOTE: The caller must take care to call appropriate methods for the underlying type.  Calling
/// the wrong methods (e.g. calling file methods on a directory) will result in the connection being
/// closed.
pub struct RemoteIo {
    proxy: fio::NodeSynchronousProxy,
    // NOTE: This can be invalid if the remote end did not return a stream in which case
    // file I/O will use FIDL (slow).
    stream: zx::Stream,
}

impl RemoteIo {
    pub fn new(proxy: fio::NodeSynchronousProxy) -> Self {
        Self { proxy, stream: zx::NullableHandle::invalid().into() }
    }

    pub fn with_stream(proxy: fio::NodeSynchronousProxy, stream: zx::Stream) -> Self {
        Self { proxy, stream }
    }

    pub fn into_proxy(self) -> fio::NodeSynchronousProxy {
        self.proxy
    }

    fn cast_proxy<T: From<zx::Channel> + Into<zx::NullableHandle>>(&self) -> zx::Unowned<'_, T> {
        zx::Unowned::new(self.proxy.as_channel())
    }

    /// Returns attributes in fuchsia.io's FIDL representation.
    pub fn attr_get(
        &self,
        query: fio::NodeAttributesQuery,
    ) -> Result<(fio::MutableNodeAttributes, fio::ImmutableNodeAttributes), zx::Status> {
        self.proxy
            .get_attributes(query, zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Returns attributes mapped to `zxio_node_attributes_t`
    ///
    /// NOTE: Not all attributes are supported.  See `zxio_attr_from_fidl` above for supported
    /// attributes.
    pub fn attr_get_zxio(
        &self,
        query: fio::NodeAttributesQuery,
    ) -> Result<zxio_node_attributes_t, zx::Status> {
        self.attr_get(query).map(|(m, i)| zxio_attr_from_fidl(&m, &i))
    }

    /// Sets attributes.
    pub fn attr_set(&self, attributes: fio::MutableNodeAttributes) -> Result<(), zx::Status> {
        self.proxy
            .update_attributes(&attributes, zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Wraps fuchsia.io/Directory's Open.
    pub fn open<F: Factory>(
        &self,
        path: &str,
        flags: fio::Flags,
        create_attributes: Option<fio::MutableNodeAttributes>,
        query: fio::NodeAttributesQuery,
        factory: F,
    ) -> Result<F::Result, zx::Status> {
        let (client_end, server_end) = zx::Channel::create();
        let dir_proxy = self.cast_proxy::<fio::DirectorySynchronousProxy>();
        dir_proxy
            .open(
                path,
                flags | fio::Flags::FLAG_SEND_REPRESENTATION,
                &fio::Options {
                    attributes: (!query.is_empty()).then_some(query),
                    create_attributes,
                    ..Default::default()
                },
                server_end,
            )
            .map_err(|_| zx::Status::IO)?;
        create_with_on_representation(client_end.into(), factory)
    }

    /// Opens all nodes iteratively along the relative sub-paths given in `paths`.
    /// Each item in `paths` is opened from the node returned by opening the previous item in
    /// `paths`.
    ///
    /// `factory_fn` is used to create the `factory` argument to `create_with_on_representation`.
    ///
    /// The return vector can be smaller than the initial `paths` vector, and execution will
    /// always stop with the first error appending an `Err(status)` into the results.
    ///
    /// NOTE: To prevent opening and writing through non-directory nodes, this function adds
    /// `fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::PROTOCOL_SYMLINK` to intermediate components.
    pub fn open_pipelined<F: Factory>(
        &self,
        paths: &[&str],
        flags: fio::Flags,
        query: fio::NodeAttributesQuery,
        mut factory_fn: impl FnMut() -> F,
    ) -> impl Iterator<Item = Result<F::Result, zx::Status>> {
        let mut proxy_to_result = move |proxy: fio::NodeSynchronousProxy| {
            create_with_on_representation(proxy, factory_fn())
        };

        let mut proxies = SmallVec::<[fio::NodeSynchronousProxy; 8]>::new();
        let (client_end, server_end) = zx::Channel::create();
        proxies.push(fio::NodeSynchronousProxy::new(client_end));
        let mut next_server_end = Some(server_end);

        for (i, path) in paths.iter().enumerate().rev() {
            let server_end = next_server_end.take().unwrap();
            let channel = if i == 0 {
                self.proxy.as_channel()
            } else {
                let (client_end, server_end) = zx::Channel::create();
                next_server_end = Some(server_end);
                proxies.push(fio::NodeSynchronousProxy::new(client_end));
                proxies.last().unwrap().as_channel()
            };
            let dir_proxy = zx::Unowned::<fio::DirectorySynchronousProxy>::new(channel);

            let mut open_flags = flags | fio::Flags::FLAG_SEND_REPRESENTATION;
            if i < paths.len() - 1 {
                open_flags |= fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::PROTOCOL_SYMLINK;
            }

            if let Err(_) = dir_proxy.open(
                path,
                open_flags,
                &fio::Options {
                    attributes: (!query.is_empty()).then_some(query),
                    ..Default::default()
                },
                server_end,
            ) {
                // This should only be possible for the call from self. Other channels cannot be
                // closed as they have not been sent yet.
                debug_assert!(i == 0);
                // Nothing to do. The first proxy just got disconnected. `proxy_to_result` will
                // return an error.
            }
        }

        let mut proxies = proxies.into_iter().rev();
        std::iter::successors(proxies.next().map(&mut proxy_to_result), move |prev| match prev {
            Err(_) => None,
            Ok(_) => proxies.next().map(&mut proxy_to_result),
        })
    }

    /// Returns `(data, eof)`, where `eof` is true if we encountered the end of the file.  If `eof`
    /// is false, then it is still possible that a subsequent read would read no more i.e. the end
    /// of the file _might_ have been reached.  This might return fewer bytes than `max`.
    pub fn read_partial(&self, offset: u64, max: usize) -> Result<(Vec<u8>, bool), zx::Status> {
        if self.stream.is_invalid() {
            let file_proxy = self.cast_proxy::<fio::FileSynchronousProxy>();
            let max = std::cmp::min(max as u64, fio::MAX_TRANSFER_SIZE);
            let data = file_proxy
                .read_at(max, offset, zx::MonotonicInstant::INFINITE)
                .map_err(|_| zx::Status::IO)?
                .map_err(zx::Status::err_from_raw)?;
            let eof = (data.len() as u64) < max;
            Ok((data, eof))
        } else {
            // Use an intermediate buffer.
            let bytes =
                self.stream.read_at_to_vec(zx::StreamReadOptions::empty(), offset as u64, max)?;
            let eof = bytes.len() < max;
            Ok((bytes, eof))
        }
    }

    /// Attempts to read `len` bytes and will only return fewer if it encounters the end of the
    /// file, or an error.  `callback` will be called for each chunk.  If any bytes are successfully
    /// passed to `callback`, `read` will return the total number of bytes successfully written and
    /// any error encountered will be discarded.
    pub fn read<E>(
        &self,
        offset: u64,
        len: usize,
        mut callback: impl FnMut(Vec<u8>) -> Result<usize, E>,
        map_err: impl FnOnce(zx::Status) -> E,
    ) -> Result<usize, E> {
        let mut total = 0;
        while total < len {
            match self.read_partial(offset + total as u64, len - total) {
                Ok((data, eof)) => {
                    if data.is_empty() {
                        break;
                    }
                    let data_len = data.len();
                    let written = callback(data)?;
                    total += written;
                    if eof || written < data_len {
                        break;
                    }
                }
                Err(e) => {
                    if total > 0 {
                        break;
                    }
                    return Err(map_err(e));
                }
            }
        }
        Ok(total)
    }

    /// Writes `data` at `offset`.
    pub fn write(&self, offset: u64, data: &[u8]) -> Result<usize, zx::Status> {
        let file_proxy = self.cast_proxy::<fio::FileSynchronousProxy>();
        let mut total_written = 0;
        for chunk in data.chunks(fio::MAX_TRANSFER_SIZE as usize) {
            let result = file_proxy
                .write_at(chunk, offset + total_written as u64, zx::MonotonicInstant::INFINITE)
                .map_err(|_| zx::Status::IO)
                .and_then(|res| res.map_err(zx::Status::err_from_raw));
            match result {
                Ok(actual) => {
                    let actual = actual as usize;
                    total_written += actual;
                    if actual < chunk.len() {
                        return Ok(total_written);
                    }
                }
                Err(e) => {
                    if total_written > 0 {
                        break;
                    }
                    return Err(e);
                }
            }
        }
        Ok(total_written)
    }

    /// Returns true if vectored operations are supported.
    pub fn supports_vectored(&self) -> bool {
        // We only support readv and writev if we have a stream.
        !self.stream.is_invalid()
    }

    /// Reads into `iovecs` using a vectored read.  This is only supported with a valid stream.  See
    /// `supports_vectored` above.
    ///
    /// # Safety
    ///
    /// Same as `zx::Stream::readv`.
    pub unsafe fn readv(
        &self,
        offset: u64,
        iovecs: &mut [zx::sys::zx_iovec_t],
    ) -> Result<usize, zx::Status> {
        if self.stream.is_invalid() {
            return Err(zx::Status::NOT_SUPPORTED);
        }
        // SAFETY: See `zx::Stream::readv`.
        unsafe { self.stream.readv_at(zx::StreamReadOptions::empty(), offset as u64, iovecs) }
    }

    /// Writes from `iovecs` using vectored write.  This is only supported with a valid stream.  See
    /// `supports_vectored` above.
    pub fn writev(&self, offset: u64, iovecs: &[zx::sys::zx_iovec_t]) -> Result<usize, zx::Status> {
        if self.stream.is_invalid() {
            return Err(zx::Status::NOT_SUPPORTED);
        }
        self.stream.writev_at(zx::StreamWriteOptions::empty(), offset, &iovecs)
    }

    /// Wraps fuchsia.io/File's Truncate.
    pub fn truncate(&self, length: u64) -> Result<(), zx::Status> {
        self.cast_proxy::<fio::FileSynchronousProxy>()
            .resize(length, zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Returns a VMO backing the file.
    pub fn vmo_get(&self, flags: zx::VmarFlags) -> Result<zx::Vmo, zx::Status> {
        let mut fio_flags = fio::VmoFlags::empty();
        if flags.contains(zx::VmarFlags::PERM_READ) {
            fio_flags |= fio::VmoFlags::READ;
        }
        if flags.contains(zx::VmarFlags::PERM_WRITE) {
            fio_flags |= fio::VmoFlags::WRITE;
        }
        if flags.contains(zx::VmarFlags::PERM_EXECUTE) {
            fio_flags |= fio::VmoFlags::EXECUTE;
        }
        let file_proxy = self.cast_proxy::<fio::FileSynchronousProxy>();
        let vmo = file_proxy
            .get_backing_memory(fio_flags, zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)?;
        Ok(vmo)
    }

    /// Wraps fuchsia.io/Node's Sync.
    pub fn sync(&self) -> Result<(), zx::Status> {
        self.proxy
            .sync(zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Closes and updates access time asynchronously.
    pub fn close_and_update_access_time(self) {
        let _ = self.proxy.get_attributes(
            fio::NodeAttributesQuery::PENDING_ACCESS_TIME_UPDATE,
            zx::MonotonicInstant::INFINITE_PAST,
        );
    }

    /// Clones (in the fuchsia.unknown.Clonable sense) the underlying proxy.
    pub fn clone_proxy(&self) -> Result<fio::NodeSynchronousProxy, zx::Status> {
        let (client_end, server_end) = zx::Channel::create();
        self.proxy.clone(server_end.into()).map_err(|_| zx::Status::IO)?;
        Ok(client_end.into())
    }

    /// Wraps fuchsia.io/Node's LinkInto.
    pub fn link_into(&self, target_dir: &Self, name: &str) -> Result<(), zx::Status> {
        let target_dir_proxy = target_dir.cast_proxy::<fio::DirectorySynchronousProxy>();
        let (status, token) = target_dir_proxy
            .get_token(zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?;
        zx::Status::ok(status)?;
        let token = token.ok_or(zx::Status::NOT_SUPPORTED)?;

        // Linkable::LinkInto is a separate protocol.
        let linkable_proxy = self.cast_proxy::<fio::LinkableSynchronousProxy>();

        linkable_proxy
            .link_into(token.into(), name, zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Wraps fuchsia.io/Directory's Unlink.
    pub fn unlink(&self, name: &str, flags: fio::UnlinkFlags) -> Result<(), zx::Status> {
        let options = fio::UnlinkOptions { flags: Some(flags), ..Default::default() };
        let dir_proxy = self.cast_proxy::<fio::DirectorySynchronousProxy>();
        dir_proxy
            .unlink(name, &options, zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Wraps fuchsia.io/Directory's Rename.
    pub fn rename(
        &self,
        old_path: &str,
        new_directory: &Self,
        new_path: &str,
    ) -> Result<(), zx::Status> {
        let new_dir_proxy = new_directory.cast_proxy::<fio::DirectorySynchronousProxy>();
        let (status, token) =
            new_dir_proxy.get_token(zx::MonotonicInstant::INFINITE).map_err(|_| zx::Status::IO)?;
        zx::Status::ok(status)?;
        let token = token.ok_or(zx::Status::NOT_SUPPORTED)?;
        let dir_proxy = self.cast_proxy::<fio::DirectorySynchronousProxy>();
        dir_proxy
            .rename(old_path, token.into(), new_path, zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Wraps fuchsia.io/Directory's CreateSymlink.
    pub fn create_symlink(&self, name: &str, target: &[u8]) -> Result<RemoteIo, zx::Status> {
        let dir_proxy = self.cast_proxy::<fio::DirectorySynchronousProxy>();
        let (client_end, server_end) = zx::Channel::create();
        dir_proxy
            .create_symlink(name, target, Some(server_end.into()), zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)?;
        Ok(RemoteIo::new(client_end.into()))
    }

    /// Wraps fuchsia.io/File's EnableVerity.
    pub fn enable_verity(&self, descriptor: &zxio_fsverity_descriptor_t) -> Result<(), zx::Status> {
        let file_proxy = self.cast_proxy::<fio::FileSynchronousProxy>();
        let options = fio::VerificationOptions {
            hash_algorithm: Some(match descriptor.hash_algorithm {
                1 => fio::HashAlgorithm::Sha256,
                2 => fio::HashAlgorithm::Sha512,
                _ => return Err(zx::Status::INVALID_ARGS),
            }),
            salt: Some(descriptor.salt[..descriptor.salt_size as usize].to_vec()),
            ..Default::default()
        };
        file_proxy
            .enable_verity(&options, zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Wraps fuchsia.io/File's Allocate.
    pub fn allocate(&self, offset: u64, len: u64, mode: AllocateMode) -> Result<(), zx::Status> {
        let file_proxy = self.cast_proxy::<fio::FileSynchronousProxy>();
        let mut fio_mode = fio::AllocateMode::empty();
        if mode.contains(AllocateMode::KEEP_SIZE) {
            fio_mode |= fio::AllocateMode::KEEP_SIZE;
        }
        if mode.contains(AllocateMode::UNSHARE_RANGE) {
            fio_mode |= fio::AllocateMode::UNSHARE_RANGE;
        }
        if mode.contains(AllocateMode::PUNCH_HOLE) {
            fio_mode |= fio::AllocateMode::PUNCH_HOLE;
        }
        if mode.contains(AllocateMode::COLLAPSE_RANGE) {
            fio_mode |= fio::AllocateMode::COLLAPSE_RANGE;
        }
        if mode.contains(AllocateMode::ZERO_RANGE) {
            fio_mode |= fio::AllocateMode::ZERO_RANGE;
        }
        if mode.contains(AllocateMode::INSERT_RANGE) {
            fio_mode |= fio::AllocateMode::INSERT_RANGE;
        }
        file_proxy
            .allocate(offset, len, fio_mode, zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Wraps fuchsia.io/Node's GetExtendedAttribute.
    pub fn xattr_get(&self, name: &[u8]) -> Result<Vec<u8>, zx::Status> {
        let name_str = std::str::from_utf8(name.as_ref()).map_err(|_| zx::Status::INVALID_ARGS)?;
        let result = self
            .proxy
            .get_extended_attribute(name_str.as_bytes(), zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)?;
        match result {
            fio::ExtendedAttributeValue::Bytes(bytes) => Ok(bytes),
            fio::ExtendedAttributeValue::Buffer(vmo) => {
                let size = vmo.get_content_size().map_err(|_| zx::Status::IO)?;
                let mut bytes = vec![0u8; size as usize];
                vmo.read(&mut bytes, 0).map_err(|_| zx::Status::IO)?;
                Ok(bytes)
            }
            _ => Err(zx::Status::NOT_SUPPORTED),
        }
    }

    /// Wraps fuchsia.io/Node's SetExtendedAttribute.
    pub fn xattr_set(
        &self,
        name: &[u8],
        value: &[u8],
        mode: syncio::XattrSetMode,
    ) -> Result<(), zx::Status> {
        let val = fio::ExtendedAttributeValue::Bytes(value.to_vec());
        let fidl_mode = match mode {
            syncio::XattrSetMode::Set => fio::SetExtendedAttributeMode::Set,
            syncio::XattrSetMode::Create => fio::SetExtendedAttributeMode::Create,
            syncio::XattrSetMode::Replace => fio::SetExtendedAttributeMode::Replace,
        };
        self.proxy
            .set_extended_attribute(name, val, fidl_mode, zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Wraps fuchsia.io/Node's RenoveExtendedAttribute.
    pub fn xattr_remove(&self, name: &[u8]) -> Result<(), zx::Status> {
        let name_str = std::str::from_utf8(name.as_ref()).map_err(|_| zx::Status::INVALID_ARGS)?;
        self.proxy
            .remove_extended_attribute(name_str.as_bytes(), zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Wraps fuchsia.io/Node's ListExtendedAttributes.
    pub fn xattr_list(&self) -> Result<Vec<Vec<u8>>, zx::Status> {
        let (client_end, server_end) = zx::Channel::create();
        self.proxy.list_extended_attributes(server_end.into()).map_err(|_| zx::Status::IO)?;
        let iterator = fio::ExtendedAttributeIteratorSynchronousProxy::new(client_end);
        let mut all_attrs = vec![];
        loop {
            let (attributes, last) = iterator
                .get_next(zx::MonotonicInstant::INFINITE)
                .map_err(|_| zx::Status::IO)?
                .map_err(zx::Status::err_from_raw)?;
            all_attrs.extend(attributes);
            if last {
                break;
            }
        }
        Ok(all_attrs)
    }
}

/// RemoteDirectory supports iteration of directories and things that you can do to directories via
/// a file descriptor.  Other opterations, such as creating children, can be done via `RemoteIo`.
/// Iteration is not safe to be done concurrently because there is a seek pointer; `readdir` will
/// resume from the seek position.
pub struct RemoteDirectory {
    proxy: fio::DirectorySynchronousProxy,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Buffer contains the last batch of entries read from the remote end.
    buffer: Vec<u8>,

    /// Position in the buffer for the next entry.
    offset: usize,

    /// If the last attempt to write to the sink failed, this contains the entry that is pending to
    /// be added. This is also used to synthesize dot-dot.
    pending_entry: Entry,

    /// The current iterator position in the directory.
    current_index: u64,
}

impl State {
    fn name(&self, range: Range<usize>) -> &[u8] {
        &self.buffer[range]
    }

    /// Returns the next dir entry. If no more entries are found, returns None.  Returns an error if
    /// the iterator fails.
    fn next(&mut self, proxy: &fio::DirectorySynchronousProxy) -> Result<Entry, zx::Status> {
        let mut next_dirent = || -> Result<Entry, zx::Status> {
            if self.offset >= self.buffer.len() {
                match proxy.read_dirents(fio::MAX_BUF, zx::MonotonicInstant::INFINITE) {
                    Ok((status, dirents)) => {
                        zx::Status::ok(status)?;
                        if dirents.is_empty() {
                            return Ok(Entry::None);
                        }
                        self.buffer = dirents;
                        self.offset = 0;
                    }
                    Err(_) => return Err(zx::Status::IO),
                }
            }

            #[repr(C, packed)]
            #[derive(FromBytes)]
            struct DirectoryEntry {
                ino: u64,
                name_len: u8,
                entry_type: u8,
            }

            let Some((ino, name, entry_type)) =
                DirectoryEntry::read_from_prefix(&self.buffer[self.offset..]).ok().and_then(
                    |(DirectoryEntry { ino, name_len, entry_type }, remainder)| {
                        let name_len = name_len as usize;
                        let name_start = self.offset + std::mem::size_of::<DirectoryEntry>();
                        (remainder.len() >= name_len).then_some((
                            ino,
                            name_start..name_start + name_len,
                            entry_type,
                        ))
                    },
                )
            else {
                // Truncated entry.
                return Ok(Entry::None);
            };

            self.offset = name.end;

            Ok(Entry::Some {
                ino,
                entry_type: fio::DirentType::from_primitive(entry_type).ok_or(zx::Status::IO)?,
                name,
            })
        };

        let mut next = self.pending_entry.take();
        if let Entry::None = next {
            next = next_dirent()?;
        }
        // We only want to synthesize .. if . exists because the . and .. entries get removed if the
        // directory is unlinked, so if the remote filesystem has removed ., we know to omit the
        // .. entry.
        match &next {
            Entry::Some { name, .. } if self.name(name.clone()) == b"." => {
                self.pending_entry = Entry::DotDot;
            }
            _ => {}
        }
        self.current_index += 1;
        Ok(next)
    }

    fn rewind(&mut self, proxy: &fio::DirectorySynchronousProxy) -> Result<(), zx::Status> {
        self.pending_entry = Entry::None;
        let status = proxy.rewind(zx::MonotonicInstant::INFINITE).map_err(|_| zx::Status::IO)?;
        zx::Status::ok(status)?;
        self.buffer.clear();
        self.offset = 0;
        self.current_index = 0;
        Ok(())
    }
}

#[derive(Default)]
enum Entry {
    // Indicates no more entries.
    #[default]
    None,

    Some {
        ino: u64,
        entry_type: fio::DirentType,
        name: Range<usize>,
    },

    // Indicates dot-dot should be synthesized.
    DotDot,
}

impl Entry {
    fn take(&mut self) -> Entry {
        std::mem::replace(self, Entry::None)
    }
}

impl RemoteDirectory {
    pub fn new(proxy: fio::DirectorySynchronousProxy) -> Self {
        Self { proxy, state: Mutex::default() }
    }

    /// Seeks to `new_index` in the directory.
    pub fn seek(&self, new_index: u64) -> Result<u64, zx::Status> {
        let mut state = self.state.lock();

        if new_index < state.current_index {
            // Our iterator only goes forward, so reset it here.  Note: we *must* rewind it rather
            // than just create a new iterator because the remote end maintains the offset.
            state.rewind(&self.proxy)?;
            state.current_index = 0;
        }

        // Advance the iterator to catch up with the offset.
        for i in state.current_index..new_index {
            match state.next(&self.proxy) {
                Ok(Entry::Some { .. } | Entry::DotDot) => {}
                Ok(Entry::None) => break, // No more entries.
                Err(_) => {
                    // In order to keep the offset and the iterator in sync, set the new offset
                    // to be as far as we could get.
                    // Note that failing the seek here would also cause the iterator and the
                    // offset to not be in sync, because the iterator has already moved from
                    // where it was.
                    return Ok(i);
                }
            }
        }

        Ok(new_index)
    }

    /// Returns `None` if there are no more entries to be read.  `sink` can choose to return
    /// `ControlFlow::Break(_)` in which case the entry will be returned the next time `readdir` is
    /// called.
    pub fn readdir<B, S: FnMut(u64, fio::DirentType, &[u8]) -> ControlFlow<B, ()>>(
        &self,
        mut sink: S,
    ) -> Result<Option<B>, zx::Status> {
        let mut state = self.state.lock();
        loop {
            let entry = state.next(&self.proxy)?;
            if let ControlFlow::Break(b) = match &entry {
                Entry::Some { ino, entry_type, name } => {
                    sink(*ino, *entry_type, state.name(name.clone()))
                }
                Entry::DotDot => sink(0, fio::DirentType::Directory, b".."),
                Entry::None => break,
            } {
                state.pending_entry = entry;
                return Ok(Some(b));
            }
        }
        Ok(None)
    }

    /// Wraps fuchsia.io/Node's Sync.
    pub fn sync(&self) -> Result<(), zx::Status> {
        self.proxy
            .sync(zx::MonotonicInstant::INFINITE)
            .map_err(|_| zx::Status::IO)?
            .map_err(zx::Status::err_from_raw)
    }

    /// Clones (in the fuchsia.unknown.Clonable sense) the underlying proxy.
    pub fn clone_proxy(&self) -> Result<fio::DirectorySynchronousProxy, zx::Status> {
        let (client_end, server_end) = zx::Channel::create();
        self.proxy.clone(server_end.into()).map_err(|_| zx::Status::IO)?;
        Ok(client_end.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::RequestStream;
    use fuchsia_async as fasync;
    use futures::StreamExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[fuchsia::test]
    async fn test_read_chunking() {
        let (client, mut stream) = fidl::endpoints::create_request_stream::<fio::FileMarker>();
        let content = vec![0xAB; (fio::MAX_TRANSFER_SIZE + 100) as usize];
        let content_clone = content.clone();

        let _server_task = fasync::Task::spawn(async move {
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fio::FileRequest::ReadAt { count, offset, responder } => {
                        let start = offset as usize;
                        let end = std::cmp::min(start + count as usize, content_clone.len());
                        let data = if start < content_clone.len() {
                            &content_clone[start..end]
                        } else {
                            &[]
                        };
                        responder.send(Ok(data)).unwrap();
                    }
                    _ => panic!("Unexpected request: {:?}", request),
                }
            }
        });

        let io = RemoteIo::new(client.into_channel().into());
        fasync::unblock(move || {
            let mut data = Vec::new();
            let actual = io
                .read(
                    0,
                    content.len(),
                    |chunk| -> Result<usize, zx::Status> {
                        let len = chunk.len();
                        data.extend(chunk);
                        Ok(len)
                    },
                    |status| status,
                )
                .unwrap();
            assert_eq!(actual, content.len());
            assert_eq!(data, content);
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_read_error_after_data() {
        let (client, mut stream) = fidl::endpoints::create_request_stream::<fio::FileMarker>();
        let chunk_size = 100;
        let content = vec![0xAA; chunk_size];
        let content_clone = content.clone();

        let _server_task = fasync::Task::spawn(async move {
            let mut request_count = 0;
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fio::FileRequest::ReadAt { count: _, offset: _, responder } => {
                        request_count += 1;
                        if request_count == 1 {
                            responder.send(Ok(&content_clone)).unwrap();
                        } else {
                            responder.send(Err(zx::sys::ZX_ERR_IO)).unwrap();
                        }
                    }
                    _ => panic!("Unexpected request: {:?}", request),
                }
            }
        });

        let io = RemoteIo::new(client.into_channel().into());
        fasync::unblock(move || {
            let mut data = Vec::new();
            // Ask for more than chunk_size to ensure a second request is made.
            let actual = io
                .read(
                    0,
                    chunk_size * 2,
                    |chunk| -> Result<usize, zx::Status> {
                        let len = chunk.len();
                        data.extend(chunk);
                        Ok(len)
                    },
                    |status| status,
                )
                .expect("read should succeed even if later chunks fail");
            assert_eq!(actual, chunk_size);
            assert_eq!(data.len(), chunk_size);
            assert_eq!(data, content);
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_write_chunking() {
        let (client, mut stream) = fidl::endpoints::create_request_stream::<fio::FileMarker>();
        let content = vec![0xCD; (fio::MAX_TRANSFER_SIZE + 100) as usize];
        let content2 = content.clone();

        let server_task = fasync::Task::spawn(async move {
            let mut written = vec![0; content2.len()];
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fio::FileRequest::WriteAt { offset, data, responder, .. } => {
                        let offset = offset as usize;
                        written[offset..offset + data.len()].copy_from_slice(&data);
                        responder.send(Ok(data.len() as u64)).unwrap();
                    }
                    _ => panic!("Unexpected request: {:?}", request),
                }
            }
            assert_eq!(written, content2);
        });

        let io = RemoteIo::new(client.into_channel().into());
        fasync::unblock(move || {
            let written = io.write(0, &content).expect("write failed");
            assert_eq!(written, content.len());
        })
        .await;

        server_task.await;
    }

    #[fuchsia::test]
    async fn test_write_error_after_data() {
        let (client, mut stream) = fidl::endpoints::create_request_stream::<fio::FileMarker>();
        let chunk_size = fio::MAX_TRANSFER_SIZE as usize;
        let content = vec![0xEE; chunk_size * 2];

        let _server_task = fasync::Task::spawn(async move {
            let mut request_count = 0;
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fio::FileRequest::WriteAt { offset: _, data, responder, .. } => {
                        request_count += 1;
                        if request_count == 1 {
                            responder.send(Ok(data.len() as u64)).unwrap();
                        } else {
                            responder.send(Err(zx::sys::ZX_ERR_IO)).unwrap();
                        }
                    }
                    _ => panic!("Unexpected request: {:?}", request),
                }
            }
        });

        let io = RemoteIo::new(client.into_channel().into());
        fasync::unblock(move || {
            let written = io.write(0, &content).expect("write should succeed partial");
            assert_eq!(written, chunk_size);
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_large_directory() {
        let (client, mut stream) = fidl::endpoints::create_request_stream::<fio::DirectoryMarker>();
        let num_entries = 2000;

        let task = fasync::Task::spawn(async move {
            let mut sent_count = 0;
            let mut num_requests = 0;
            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fio::DirectoryRequest::ReadDirents { max_bytes, responder } => {
                        num_requests += 1;
                        let mut buffer = Vec::new();
                        while sent_count < num_entries {
                            let name = if sent_count == 0 {
                                ".".to_string()
                            } else {
                                format!("file_{}", sent_count - 1)
                            };
                            let name_bytes = name.as_bytes();
                            let entry_size = 10 + name_bytes.len();
                            if buffer.len() + entry_size > max_bytes as usize {
                                break;
                            }
                            buffer.extend_from_slice(&(sent_count as u64 + 1).to_le_bytes());
                            buffer.push(name_bytes.len() as u8);
                            let entry_type = if sent_count == 0 {
                                fio::DirentType::Directory
                            } else {
                                fio::DirentType::File
                            };
                            buffer.push(entry_type.into_primitive());
                            buffer.extend_from_slice(name_bytes);
                            sent_count += 1;
                        }
                        let _ = responder.send(0, &buffer);
                    }
                    fio::DirectoryRequest::Rewind { responder } => {
                        sent_count = 0;
                        let _ = responder.send(0);
                    }
                    fio::DirectoryRequest::Close { responder } => {
                        let _ = responder.send(Ok(()));
                    }
                    _ => {}
                }
            }
            assert!(num_requests > 0);
        });

        let dir = RemoteDirectory::new(client.into_channel().into());
        let count = Arc::new(AtomicU64::new(0));
        let count2 = count.clone();
        fasync::unblock(move || {
            dir.readdir::<(), _>(|_ino, _type, _name| {
                count.fetch_add(1, Ordering::Relaxed);
                ControlFlow::Continue(())
            })
            .unwrap();
        })
        .await;
        // Expect num_entries + 1 (for synthesized "..")
        assert_eq!(count2.load(Ordering::Relaxed), num_entries + 1);
        task.await;
    }

    #[fuchsia::test]
    async fn test_seek_backwards() {
        let (client, mut stream) = fidl::endpoints::create_request_stream::<fio::DirectoryMarker>();
        let _server_task = fasync::Task::spawn(async move {
            let entries = vec![
                (1, fio::DirentType::Directory, "."),
                (2, fio::DirentType::File, "file_0"),
                (3, fio::DirentType::File, "file_1"),
                (4, fio::DirentType::File, "file_2"),
            ];
            let mut current_entry = 0;

            while let Some(Ok(request)) = stream.next().await {
                match request {
                    fio::DirectoryRequest::ReadDirents { max_bytes, responder } => {
                        let mut buffer = Vec::new();
                        while current_entry < entries.len() {
                            let (ino, type_, name) = entries[current_entry];
                            let name_bytes = name.as_bytes();
                            let entry_size = 10 + name_bytes.len();
                            if buffer.len() + entry_size > max_bytes as usize {
                                break;
                            }
                            buffer.extend_from_slice(&(ino as u64).to_le_bytes());
                            buffer.push(name_bytes.len() as u8);
                            buffer.push(type_.into_primitive());
                            buffer.extend_from_slice(name_bytes);
                            current_entry += 1;
                        }
                        responder.send(0, &buffer).unwrap();
                    }
                    fio::DirectoryRequest::Rewind { responder } => {
                        current_entry = 0;
                        responder.send(0).unwrap();
                    }
                    _ => {}
                }
            }
        });

        let dir = RemoteDirectory::new(client.into_channel().into());
        fasync::unblock(move || {
            let mut names = Vec::new();
            // Read 3 entries: ".", "..", "file_0".
            dir.readdir::<(), _>(|_ino, _type, name| {
                names.push(name.to_vec());
                if names.len() == 3 { ControlFlow::Break(()) } else { ControlFlow::Continue(()) }
            })
            .unwrap();

            assert_eq!(names[0], b".");
            assert_eq!(names[1], b"..");
            assert_eq!(names[2], b"file_0");

            // Seek to 1. This triggers rewind() internally because 1 < current_index (3).
            // Index 1 corresponds to "..".
            dir.seek(1).unwrap();

            let mut names_after_seek = Vec::new();
            // Read 2 entries: "..", "file_0".
            dir.readdir::<(), _>(|_ino, _type, name| {
                names_after_seek.push(name.to_vec());
                if names_after_seek.len() == 2 {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            })
            .unwrap();

            assert_eq!(names_after_seek[0], b"..");
            assert_eq!(names_after_seek[1], b"file_0");
        })
        .await;
    }

    struct DummyFactory;

    impl Factory for DummyFactory {
        type Result = RemoteIo;
        fn create_node(self, io: RemoteIo, _info: fio::NodeInfo) -> Self::Result {
            io
        }
        fn create_directory(self, io: RemoteIo, _info: fio::DirectoryInfo) -> Self::Result {
            io
        }
        fn create_file(self, io: RemoteIo, _info: fio::FileInfo) -> Self::Result {
            io
        }
        fn create_symlink(self, io: RemoteIo, _info: fio::SymlinkInfo) -> Self::Result {
            io
        }
    }

    #[fuchsia::test]
    async fn test_open_pipelined() {
        let (client, stream) = fidl::endpoints::create_request_stream::<fio::DirectoryMarker>();

        fn serve_mock_directory(
            mut stream: fio::DirectoryRequestStream,
            mut expected_paths: Vec<String>,
        ) {
            fasync::Task::spawn(async move {
                if let Some(Ok(request)) = stream.next().await {
                    match request {
                        fio::DirectoryRequest::Open { path, flags, options: _, object, .. } => {
                            if !expected_paths.is_empty() {
                                let expected = expected_paths.remove(0);
                                assert_eq!(path, expected);
                                if expected == "path1" {
                                    assert!(flags.contains(fio::Flags::PROTOCOL_DIRECTORY));
                                } else if expected == "path2" {
                                    assert!(!flags.contains(fio::Flags::PROTOCOL_DIRECTORY));
                                }
                            }
                            let server_end =
                                fidl::endpoints::ServerEnd::<fio::DirectoryMarker>::new(object);
                            let dir_stream = server_end.into_stream();
                            let control_handle = dir_stream.control_handle();
                            let representation =
                                fio::Representation::Directory(fio::DirectoryInfo::default());
                            control_handle.send_on_representation(representation).unwrap();

                            serve_mock_directory(dir_stream, expected_paths);
                        }
                        _ => {}
                    }
                }
            })
            .detach();
        }

        serve_mock_directory(stream, vec!["path1".to_string(), "path2".to_string()]);

        let io = RemoteIo::new(client.into_channel().into());
        fasync::unblock(move || {
            let results = io
                .open_pipelined(
                    &["path1", "path2"],
                    fio::Flags::empty(),
                    fio::NodeAttributesQuery::empty(),
                    || DummyFactory,
                )
                .collect::<Vec<_>>();
            assert_eq!(results.len(), 2);
            assert!(results.iter().all(|r| r.is_ok()));
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_open_pipelined_not_found() {
        let (client, stream) = fidl::endpoints::create_request_stream::<fio::DirectoryMarker>();

        fn serve_mock_directory_not_found(mut stream: fio::DirectoryRequestStream) {
            fasync::Task::spawn(async move {
                if let Some(Ok(request)) = stream.next().await {
                    match request {
                        fio::DirectoryRequest::Open {
                            path, flags: _, options: _, object, ..
                        } => {
                            let server_end =
                                fidl::endpoints::ServerEnd::<fio::DirectoryMarker>::new(object);
                            let dir_stream = server_end.into_stream();
                            let control_handle = dir_stream.control_handle();

                            if path == "not_found" {
                                control_handle.shutdown_with_epitaph(zx::Status::NOT_FOUND);
                            } else {
                                let representation =
                                    fio::Representation::Directory(fio::DirectoryInfo::default());
                                control_handle.send_on_representation(representation).unwrap();
                                serve_mock_directory_not_found(dir_stream);
                            }
                        }
                        _ => {}
                    }
                }
            })
            .detach();
        }

        serve_mock_directory_not_found(stream);

        let io = RemoteIo::new(client.into_channel().into());
        fasync::unblock(move || {
            let results = io
                .open_pipelined(
                    &["path1", "not_found", "path3"],
                    fio::Flags::empty(),
                    fio::NodeAttributesQuery::empty(),
                    || DummyFactory,
                )
                .collect::<Vec<_>>();
            assert_eq!(results.len(), 2);
            assert!(results[0].is_ok());
            assert_eq!(results[1].as_ref().err(), Some(&zx::Status::NOT_FOUND));
        })
        .await;
    }

    #[fuchsia::test]
    async fn test_open_pipelined_peer_closed() {
        let (client, stream) = fidl::endpoints::create_request_stream::<fio::DirectoryMarker>();

        fn serve_mock_directory_close_early(mut stream: fio::DirectoryRequestStream) {
            fasync::Task::spawn(async move {
                if let Some(Ok(request)) = stream.next().await {
                    match request {
                        fio::DirectoryRequest::Open { object, .. } => {
                            // We just drop the object (the server_end of the channel), closing it.
                            drop(object);
                        }
                        _ => {}
                    }
                }
            })
            .detach();
        }

        serve_mock_directory_close_early(stream);

        let io = RemoteIo::new(client.into_channel().into());
        fasync::unblock(move || {
            let results = io
                .open_pipelined(
                    &["path1", "path2"],
                    fio::Flags::empty(),
                    fio::NodeAttributesQuery::empty(),
                    || DummyFactory,
                )
                .collect::<Vec<_>>();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].as_ref().err(), Some(&zx::Status::PEER_CLOSED));
        })
        .await;
    }
}
