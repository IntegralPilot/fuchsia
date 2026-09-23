// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! FIDL encoding and decoding.

// TODO(https://fxbug.dev/42069912): This file is too big. Split it into smaller files.

pub use static_assertions::const_assert_eq;

use crate::endpoints::ProtocolMarker;
use crate::handle::{HandleDisposition, HandleInfo, HandleOp, NullableHandle, ObjectType, Rights};
use crate::time::{Instant, Ticks, Timeline};
use crate::{Error, MethodType, Result};
use bitflags::bitflags;
pub use fidl_constants::*;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::{mem, ptr, str};

////////////////////////////////////////////////////////////////////////////////
// Traits
////////////////////////////////////////////////////////////////////////////////

/// Trait for a "Box" that wraps a handle when it's inside a client. Useful when
/// we need some infrastructure to make our underlying channels work the way the
/// client code expects.
pub trait ProxyChannelBox<D: ResourceDialect>: std::fmt::Debug + Send + Sync {
    /// Receives a message on the channel and registers this `Channel` as
    /// needing a read on receiving a `io::std::ErrorKind::WouldBlock`.
    #[allow(clippy::type_complexity)]
    fn recv_etc_from(
        &self,
        ctx: &mut std::task::Context<'_>,
        buf: &mut D::MessageBufEtc,
    ) -> std::task::Poll<Result<(), Option<<D::ProxyChannel as ProxyChannelFor<D>>::Error>>>;

    #[cfg(not(target_os = "fuchsia"))]
    /// Closed reason for proxies.
    fn closed_reason(&self) -> Option<String> {
        None
    }

    /// Get a reference to the boxed channel.
    fn as_channel(&self) -> &D::ProxyChannel;

    /// Write data to a Proxy channel
    fn write_etc(
        &self,
        bytes: &[u8],
        handles: &mut [<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition],
    ) -> Result<(), Option<<D::ProxyChannel as ProxyChannelFor<D>>::Error>>;

    /// Return whether a `ProxyChannel` is closed.
    fn is_closed(&self) -> bool;

    /// Unbox this channel
    fn unbox(self) -> D::ProxyChannel;
}

/// Message buffer used to hold a message in a particular dialect.
pub trait MessageBufFor<D: ResourceDialect>: std::fmt::Debug + Send + Sync {
    /// Create a new message buffer.
    fn new() -> Self;

    /// Discard any allocated-but-unused space in the byte portion of this buffer.
    fn shrink_bytes_to_fit(&mut self) {}

    /// Access the contents of this buffer as two vectors.
    fn split_mut(&mut self) -> (&mut Vec<u8>, &mut Vec<<D::Handle as HandleFor<D>>::HandleInfo>);
}

/// Channel used for proxies in a particular dialect.
pub trait ProxyChannelFor<D: ResourceDialect>:
    std::fmt::Debug + crate::epitaph::ChannelLike
{
    /// Box we put around a `ProxyChannel` when using it within a client.
    type Boxed: ProxyChannelBox<D>;

    /// Type of the errors we get from this proxy channel.
    type Error: Into<crate::TransportError>;

    /// Handle disposition used in this dialect.
    ///
    /// This is for sending handles, and includes the intended type/rights.
    type HandleDisposition: HandleDispositionFor<D>;

    /// Construct a new box around a proxy channel.
    fn boxed(self) -> Self::Boxed;

    /// Write data to a Proxy channel
    fn write_etc(
        &self,
        bytes: &[u8],
        handles: &mut [Self::HandleDisposition],
    ) -> Result<(), Option<Self::Error>>;
}

/// Handle disposition struct used for a particular dialect.
pub trait HandleDispositionFor<D: ResourceDialect>: std::fmt::Debug {
    /// Wrap a handle in a handle disposition.
    fn from_handle(
        handle: D::Handle,
        object_type: crate::ObjectType,
        rights: crate::Rights,
    ) -> Self;
}

/// Handle type used for a particular dialect.
pub trait HandleFor<D: ResourceDialect> {
    /// Handle info used in this dialect.
    ///
    /// This is used for receiving handles, and includes type/rights from the
    /// kernel.
    type HandleInfo: HandleInfoFor<D>;

    /// Produce an invalid version of `Handle` used as a place filler when
    /// we remove handles from an array.
    fn invalid() -> Self;

    /// Check whether a handle is invalid.
    fn is_invalid(&self) -> bool;
}

/// Handle info struct used for a particular dialect.
pub trait HandleInfoFor<D: ResourceDialect>: std::fmt::Debug {
    /// Verifies a `HandleInfo` has the type and rights we expect and
    /// extracts the `D::Handle` from it.
    fn consume(
        &mut self,
        expected_object_type: crate::ObjectType,
        expected_rights: crate::Rights,
    ) -> Result<D::Handle>;

    /// Destroy the given handle info, leaving it invalid.
    fn drop_in_place(&mut self);
}

/// Describes how a given transport encodes resources like handles.
pub trait ResourceDialect: 'static + Sized + Default + std::fmt::Debug + Copy + Clone {
    /// Handle type used in this dialect.
    type Handle: HandleFor<Self>;

    /// Message buffer type used in this dialect.
    type MessageBufEtc: MessageBufFor<Self>;

    /// Channel type used for proxies in this dialect.
    type ProxyChannel: ProxyChannelFor<Self>;

    /// Get a thread-local common instance of `TlsBuf`
    fn with_tls_buf<R>(f: impl FnOnce(&mut TlsBuf<Self>) -> R) -> R;
}

/// Indicates a type is encodable as a handle in a given resource dialect.
pub trait EncodableAsHandle: Into<<Self::Dialect as ResourceDialect>::Handle> {
    /// What resource dialect can encode this object as a handle.
    type Dialect: ResourceDialect<Handle: Into<Self>>;
}

/// A FIDL type marker.
///
/// This trait is only used for compile time dispatch. For example, we can
/// parameterize code on `T: TypeMarker`, but we would never write `value: T`.
/// In fact, `T` is often a zero-sized struct. From the user's perspective,
/// `T::Owned` is the FIDL type's "Rust type". For example, for the FIDL type
/// `string:10`, `T` is `BoundedString<10>` and `T::Owned` is `String`.
///
/// For primitive types and user-defined types, `Self` is actually the same as
/// `Self::Owned`. For all others (strings, arrays, vectors, handles, endpoints,
/// optionals, error results), `Self` is a zero-sized struct that uses generics
/// to represent FIDL type information such as the element type or constraints.
///
/// # Safety
///
/// * Implementations of `encode_is_copy` must only return true if it is safe to
///   transmute from `*const Self::Owned` to `*const u8` and read `inline_size`
///   bytes starting from that address.
///
/// * Implementations of `decode_is_copy` must only return true if it is safe to
///   transmute from `*mut Self::Owned` to `*mut u8` and write `inline_size`
///   bytes starting at that address.
pub unsafe trait TypeMarker: 'static + Sized {
    /// The owned Rust type which this FIDL type decodes into.
    type Owned;

    /// Returns the minimum required alignment of the inline portion of the
    /// encoded object. It must be a (nonzero) power of two.
    fn inline_align(context: Context) -> usize;

    /// Returns the size of the inline portion of the encoded object, including
    /// padding for alignment. Must be a multiple of `inline_align`.
    fn inline_size(context: Context) -> usize;

    /// Returns true if the memory layout of `Self::Owned` matches the FIDL wire
    /// format and encoding requires no validation. When true, we can optimize
    /// encoding arrays and vectors of `Self::Owned` to a single memcpy.
    ///
    /// This can be true even when `decode_is_copy` is false. For example, bools
    /// require validation when decoding, but they do not require validation
    /// when encoding because Rust guarantees a bool is either 0x00 or 0x01.
    #[inline(always)]
    fn encode_is_copy() -> bool {
        false
    }

    /// Returns true if the memory layout of `Self::Owned` matches the FIDL wire
    /// format and decoding requires no validation. When true, we can optimize
    /// decoding arrays and vectors of `Self::Owned` to a single memcpy.
    #[inline(always)]
    fn decode_is_copy() -> bool {
        false
    }
}

/// A FIDL value type marker.
///
/// Value types are guaranteed to never contain handles. As a result, they can
/// be encoded by immutable reference (or by value for `Copy` types).
pub trait ValueTypeMarker: TypeMarker {
    /// The Rust type to use for encoding. This is a particular `Encode<Self>`
    /// type cheaply obtainable from `&Self::Owned`. There are three cases:
    ///
    /// - Special cases such as `&[T]` for vectors.
    /// - For primitives, bits, and enums, it is `Owned`.
    /// - Otherwise, it is `&Owned`.
    type Borrowed<'a>;

    /// Cheaply converts from `&Self::Owned` to `Self::Borrowed`.
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_>;
}

/// A FIDL resource type marker.
///
/// Resource types are allowed to contain handles. As a result, they must be
/// encoded by mutable reference so that handles can be zeroed out.
pub trait ResourceTypeMarker: TypeMarker {
    /// The Rust type to use for encoding. This is a particular `Encode<Self>`
    /// type cheaply obtainable from `&mut Self::Owned`. There are three cases:
    ///
    /// - Special cases such as `&mut [T]` for vectors.
    /// - When `Owned: HandleBased`, it is `Owned`.
    /// - Otherwise, it is `&mut Owned`.
    type Borrowed<'a>;

    /// Cheaply converts from `&mut Self::Owned` to `Self::Borrowed`. For
    /// `HandleBased` types this is "take" (it returns an owned handle and
    /// replaces `value` with `Handle::invalid`), and for all other types it is
    /// "borrow" (just converts from one reference to another).
    fn take_or_borrow(value: &mut Self::Owned) -> Self::Borrowed<'_>;
}

/// A Rust type that can be encoded as the FIDL type `T`.
///
/// # Safety
///
/// Implementations of `encode` must write every byte in
/// `encoder.buf[offset..offset + T::inline_size(encoder.context)]` unless
/// returning an `Err` value.
pub unsafe trait Encode<T: TypeMarker, D: ResourceDialect>: Sized {
    /// Encodes the object into the encoder's buffers. Any handles stored in the
    /// object are swapped for `Handle::INVALID`.
    ///
    /// Implementations that encode out-of-line objects must call `depth.increment()?`.
    ///
    /// # Safety
    ///
    /// Callers must ensure `offset` is a multiple of `T::inline_align` and
    /// `encoder.buf` has room for writing `T::inline_size` bytes at `offset`.
    unsafe fn encode(self, encoder: &mut Encoder<'_, D>, offset: usize, depth: Depth)
    -> Result<()>;
}

/// A Rust type that can be decoded from the FIDL type `T`.
pub trait Decode<T: TypeMarker, D>: 'static + Sized {
    /// Creates a valid instance of `Self`. The specific value does not matter,
    /// since it will be overwritten by `decode`.
    // TODO(https://fxbug.dev/42069855): Take context parameter to discourage using this.
    fn new_empty() -> Self;

    /// Decodes an object of type `T` from the decoder's buffers into `self`.
    ///
    /// Implementations must validate every byte in
    /// `decoder.buf[offset..offset + T::inline_size(decoder.context)]` unless
    /// returning an `Err` value. Implementations that decode out-of-line
    /// objects must call `depth.increment()?`.
    ///
    /// # Safety
    ///
    /// Callers must ensure `offset` is a multiple of `T::inline_align` and
    /// `decoder.buf` has room for reading `T::inline_size` bytes at `offset`.
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()>
    where
        D: ResourceDialect;
}

////////////////////////////////////////////////////////////////////////////////
// Resource Dialects
////////////////////////////////////////////////////////////////////////////////

/// Box around an async channel. Needed to implement `ResourceDialect` for
/// `DefaultFuchsiaResourceDialect` but not so useful for that case.
#[derive(Debug)]
pub struct FuchsiaProxyBox(crate::AsyncChannel);

impl FuchsiaProxyBox {
    /// Future that returns when the channel is closed.
    pub fn on_closed(&self) -> crate::OnSignalsRef<'_> {
        self.0.on_closed()
    }

    /// See [`crate::AsyncChannel::read_etc`]
    pub fn read_etc(
        &self,
        cx: &mut std::task::Context<'_>,
        bytes: &mut Vec<u8>,
        handles: &mut Vec<crate::HandleInfo>,
    ) -> std::task::Poll<Result<(), crate::Status>> {
        self.0.read_etc(cx, bytes, handles)
    }

    /// Signal peer
    pub fn signal_peer(
        &self,
        clear: crate::Signals,
        set: crate::Signals,
    ) -> Result<(), zx_status::Status> {
        use crate::Peered;
        self.0.as_ref().signal_peer(clear, set)
    }
}

impl ProxyChannelBox<DefaultFuchsiaResourceDialect> for FuchsiaProxyBox {
    fn write_etc(
        &self,
        bytes: &[u8],
        handles: &mut [HandleDisposition<'static>],
    ) -> Result<(), Option<zx_status::Status>> {
        self.0
            .write_etc(bytes, handles)
            .map_err(|x| Some(x).filter(|x| *x != zx_status::Status::PEER_CLOSED))
    }

    fn recv_etc_from(
        &self,
        ctx: &mut std::task::Context<'_>,
        buf: &mut crate::MessageBufEtc,
    ) -> std::task::Poll<Result<(), Option<zx_status::Status>>> {
        self.0
            .recv_etc_from(ctx, buf)
            .map_err(|x| Some(x).filter(|x| *x != zx_status::Status::PEER_CLOSED))
    }

    fn is_closed(&self) -> bool {
        self.0.is_closed()
    }

    #[cfg(not(target_os = "fuchsia"))]
    fn closed_reason(&self) -> Option<String> {
        self.0.closed_reason()
    }

    fn unbox(self) -> <DefaultFuchsiaResourceDialect as ResourceDialect>::ProxyChannel {
        self.0
    }

    fn as_channel(&self) -> &<DefaultFuchsiaResourceDialect as ResourceDialect>::ProxyChannel {
        &self.0
    }
}

/// The default [`ResourceDialect`]. Encodes everything into a channel
/// MessageBuf for sending via channels between Fuchsia services.
#[derive(Debug, Default, Copy, Clone)]
pub struct DefaultFuchsiaResourceDialect;
impl ResourceDialect for DefaultFuchsiaResourceDialect {
    type Handle = NullableHandle;
    type MessageBufEtc = crate::MessageBufEtc;
    type ProxyChannel = crate::AsyncChannel;

    #[inline]
    fn with_tls_buf<R>(f: impl FnOnce(&mut TlsBuf<Self>) -> R) -> R {
        thread_local!(static TLS_BUF: RefCell<TlsBuf<DefaultFuchsiaResourceDialect>> =
            RefCell::new(TlsBuf::default()));
        TLS_BUF.with(|buf| f(&mut buf.borrow_mut()))
    }
}

impl MessageBufFor<DefaultFuchsiaResourceDialect> for crate::MessageBufEtc {
    fn new() -> crate::MessageBufEtc {
        let mut ret = crate::MessageBufEtc::new();
        ret.ensure_capacity_bytes(MIN_BUF_BYTES_SIZE);
        ret
    }

    fn shrink_bytes_to_fit(&mut self) {
        self.shrink_bytes_to_fit();
    }

    fn split_mut(&mut self) -> (&mut Vec<u8>, &mut Vec<HandleInfo>) {
        self.split_mut()
    }
}

impl ProxyChannelFor<DefaultFuchsiaResourceDialect> for crate::AsyncChannel {
    type Boxed = FuchsiaProxyBox;
    type Error = zx_status::Status;
    type HandleDisposition = HandleDisposition<'static>;

    fn boxed(self) -> FuchsiaProxyBox {
        FuchsiaProxyBox(self)
    }

    fn write_etc(
        &self,
        bytes: &[u8],
        handles: &mut [HandleDisposition<'static>],
    ) -> Result<(), Option<zx_status::Status>> {
        self.write_etc(bytes, handles)
            .map_err(|x| Some(x).filter(|x| *x != zx_status::Status::PEER_CLOSED))
    }
}

impl HandleDispositionFor<DefaultFuchsiaResourceDialect> for HandleDisposition<'static> {
    fn from_handle(handle: NullableHandle, object_type: ObjectType, rights: Rights) -> Self {
        HandleDisposition::new(HandleOp::Move(handle), object_type, rights, Ok(()))
    }
}

impl HandleFor<DefaultFuchsiaResourceDialect> for NullableHandle {
    type HandleInfo = HandleInfo;

    fn invalid() -> Self {
        NullableHandle::invalid()
    }

    fn is_invalid(&self) -> bool {
        self.is_invalid()
    }
}

impl HandleInfoFor<DefaultFuchsiaResourceDialect> for HandleInfo {
    fn consume(
        &mut self,
        expected_object_type: ObjectType,
        expected_rights: Rights,
    ) -> Result<NullableHandle> {
        let handle_info = std::mem::replace(
            self,
            HandleInfo::new(NullableHandle::invalid(), ObjectType::NONE, Rights::NONE),
        );
        let received_object_type = handle_info.object_type;
        if expected_object_type != ObjectType::NONE
            && received_object_type != ObjectType::NONE
            && expected_object_type != received_object_type
        {
            return Err(Error::IncorrectHandleSubtype {
                expected: expected_object_type,
                received: received_object_type,
            });
        }

        let received_rights = handle_info.rights;
        if expected_rights != Rights::SAME_RIGHTS
            && received_rights != Rights::SAME_RIGHTS
            && expected_rights != received_rights
        {
            if !received_rights.contains(expected_rights) {
                return Err(Error::MissingExpectedHandleRights {
                    missing_rights: expected_rights - received_rights,
                });
            }
            return match handle_info.handle.replace_handle(expected_rights) {
                Ok(r) => Ok(r),
                Err(status) => Err(Error::HandleReplace(status)),
            };
        }
        Ok(handle_info.handle)
    }

    #[inline(always)]
    fn drop_in_place(&mut self) {
        *self = HandleInfo::new(NullableHandle::invalid(), ObjectType::NONE, Rights::NONE);
    }
}

/// A never type for handles in `NoHandleResourceDialect`.
#[cfg(not(target_os = "fuchsia"))]
#[derive(Debug)]
pub enum NoHandles {}

#[cfg(not(target_os = "fuchsia"))]
impl ProxyChannelBox<NoHandleResourceDialect> for NoHandles {
    fn recv_etc_from(
        &self,
        _ctx: &mut std::task::Context<'_>,
        _buf: &mut <NoHandleResourceDialect as ResourceDialect>::MessageBufEtc,
    ) -> std::task::Poll<Result<(), Option<zx_status::Status>>> {
        unreachable!()
    }

    fn write_etc(
        &self,
        _bytes: &[u8],
        _handles: &mut [
            <<NoHandleResourceDialect as ResourceDialect>::ProxyChannel
            as ProxyChannelFor<NoHandleResourceDialect>>::HandleDisposition],
    ) -> Result<(), Option<zx_status::Status>> {
        unreachable!()
    }

    fn is_closed(&self) -> bool {
        unreachable!()
    }

    fn unbox(self) -> <NoHandleResourceDialect as ResourceDialect>::ProxyChannel {
        unreachable!()
    }

    fn as_channel(&self) -> &<NoHandleResourceDialect as ResourceDialect>::ProxyChannel {
        unreachable!()
    }
}

/// A resource dialect which doesn't support handles at all.
#[cfg(not(target_os = "fuchsia"))]
#[derive(Debug, Default, Copy, Clone)]
pub struct NoHandleResourceDialect;

#[cfg(not(target_os = "fuchsia"))]
impl ResourceDialect for NoHandleResourceDialect {
    type Handle = NoHandles;
    type MessageBufEtc = NoHandles;
    type ProxyChannel = NoHandles;

    #[inline]
    fn with_tls_buf<R>(f: impl FnOnce(&mut TlsBuf<Self>) -> R) -> R {
        thread_local!(static TLS_BUF: RefCell<TlsBuf<NoHandleResourceDialect>> =
            RefCell::new(TlsBuf::default()));
        TLS_BUF.with(|buf| f(&mut buf.borrow_mut()))
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl MessageBufFor<NoHandleResourceDialect> for NoHandles {
    fn new() -> Self {
        unreachable!()
    }

    fn split_mut(&mut self) -> (&mut Vec<u8>, &mut Vec<NoHandles>) {
        unreachable!()
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl ProxyChannelFor<NoHandleResourceDialect> for NoHandles {
    type Boxed = NoHandles;
    type Error = zx_status::Status;
    type HandleDisposition = NoHandles;

    fn boxed(self) -> NoHandles {
        unreachable!()
    }

    fn write_etc(
        &self,
        _bytes: &[u8],
        _handles: &mut [NoHandles],
    ) -> Result<(), Option<zx_status::Status>> {
        unreachable!()
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl crate::epitaph::ChannelLike for NoHandles {
    fn write_epitaph(&self, _bytes: &[u8]) -> std::result::Result<(), crate::TransportError> {
        unreachable!()
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl HandleFor<NoHandleResourceDialect> for NoHandles {
    type HandleInfo = NoHandles;

    fn invalid() -> Self {
        unreachable!()
    }

    fn is_invalid(&self) -> bool {
        unreachable!()
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl HandleDispositionFor<NoHandleResourceDialect> for NoHandles {
    fn from_handle(
        _handle: <NoHandleResourceDialect as ResourceDialect>::Handle,
        _object_type: crate::ObjectType,
        _rights: crate::Rights,
    ) -> Self {
        unreachable!()
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl HandleInfoFor<NoHandleResourceDialect> for NoHandles {
    fn consume(
        &mut self,
        _expected_object_type: crate::ObjectType,
        _expected_rights: crate::Rights,
    ) -> Result<<NoHandleResourceDialect as ResourceDialect>::Handle> {
        unreachable!()
    }

    fn drop_in_place(&mut self) {
        unreachable!()
    }
}

/// A resource dialect which doesn't support handles at all.
#[cfg(target_os = "fuchsia")]
pub type NoHandleResourceDialect = DefaultFuchsiaResourceDialect;

////////////////////////////////////////////////////////////////////////////////
// Helper functions
////////////////////////////////////////////////////////////////////////////////

/// Rounds `x` up if necessary so that it is a multiple of `align`.
///
/// Requires `align` to be a (nonzero) power of two.
#[doc(hidden)] // only exported for use in macros or generated code
#[inline(always)]
pub fn round_up_to_align(x: usize, align: usize) -> usize {
    debug_assert_ne!(align, 0);
    debug_assert_eq!(align & (align - 1), 0);
    // https://en.wikipedia.org/wiki/Data_structure_alignment#Computing_padding
    (x + align - 1) & !(align - 1)
}

/// Resize a vector without zeroing added bytes.
///
/// The type `T` must be `Copy`. This is not enforced in the type signature
/// because it is used in generic contexts where verifying this requires looking
/// at control flow. See `decode_vector` for an example.
///
/// # Safety
///
/// This is unsafe when `new_len > old_len` because it leaves new elements at
/// indices `old_len..new_len` uninitialized. The caller must overwrite all the
/// new elements before reading them. "Reading" includes any operation that
/// extends the vector, such as `push`, because this could reallocate the vector
/// and copy the uninitialized bytes.
///
/// FIDL conformance tests are used to validate that there are no uninitialized
/// bytes in the output across a range of types and values.
// TODO(https://fxbug.dev/42075223): Fix safety issues, use MaybeUninit.
#[inline]
unsafe fn resize_vec_no_zeroing<T>(buf: &mut Vec<T>, new_len: usize) {
    if new_len > buf.capacity() {
        buf.reserve(new_len - buf.len());
    }
    // Safety:
    // - `new_len` must be less than or equal to `capacity()`:
    //   The if-statement above guarantees this.
    // - The elements at `old_len..new_len` must be initialized:
    //   They are purposely left uninitialized, making this function unsafe.
    unsafe { buf.set_len(new_len) };
}

/// Helper type for checking encoding/decoding recursion depth.
#[doc(hidden)] // only exported for use in macros or generated code
#[derive(Debug, Copy, Clone)]
#[repr(transparent)]
pub struct Depth(usize);

impl Depth {
    /// Increments the depth, and returns an error if it exceeds the limit.
    #[inline(always)]
    pub fn increment(&mut self) -> Result<()> {
        self.0 += 1;
        if self.0 > MAX_RECURSION {
            return Err(Error::MaxRecursionDepth);
        }
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
// Helper macros
////////////////////////////////////////////////////////////////////////////////

/// Given `T: TypeMarker`, expands to a `T::Owned::new_empty` call.
#[doc(hidden)] // only exported for use in macros or generated code
#[macro_export]
macro_rules! new_empty {
    ($ty:ty) => {
        <<$ty as $crate::encoding::TypeMarker>::Owned as $crate::encoding::Decode<$ty, _>>::new_empty()
    };
    ($ty:ty, $d:path) => {
        <<$ty as $crate::encoding::TypeMarker>::Owned as $crate::encoding::Decode<$ty, $d>>::new_empty()
    };
}

/// Given `T: TypeMarker`, expands to a `T::Owned::decode` call.
#[doc(hidden)] // only exported for use in macros or generated code
#[macro_export]
macro_rules! decode {
    ($ty:ty, $out_value:expr, $decoder:expr, $offset:expr, $depth:expr) => {
        <<$ty as $crate::encoding::TypeMarker>::Owned as $crate::encoding::Decode<$ty, _>>::decode(
            $out_value, $decoder, $offset, $depth,
        )
    };
    ($ty:ty, $d:path, $out_value:expr, $decoder:expr, $offset:expr, $depth:expr) => {
        <<$ty as $crate::encoding::TypeMarker>::Owned as $crate::encoding::Decode<$ty, $d>>::decode(
            $out_value, $decoder, $offset, $depth,
        )
    };
}

////////////////////////////////////////////////////////////////////////////////
// Wire format
////////////////////////////////////////////////////////////////////////////////

/// Wire format version to use during encode / decode.
#[derive(Clone, Copy, Debug)]
pub enum WireFormatVersion {
    /// FIDL 2023 wire format.
    V2,
}

/// Context for encoding and decoding.
///
/// WARNING: Do not construct this directly unless you know what you're doing.
/// FIDL uses `Context` to coordinate soft migrations, so improper uses of it
/// could result in ABI breakage.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// Wire format version to use when encoding / decoding.
    pub wire_format_version: WireFormatVersion,
}

// We only support one wire format right now, so context should be zero size.
const_assert_eq!(mem::size_of::<Context>(), 0);

impl Context {
    /// Returns the header flags to set when encoding with this context.
    #[inline]
    pub(crate) fn at_rest_flags(&self) -> AtRestFlags {
        match self.wire_format_version {
            WireFormatVersion::V2 => AtRestFlags::USE_V2_WIRE_FORMAT,
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// Encoder
////////////////////////////////////////////////////////////////////////////////

/// Encoding state
#[derive(Debug)]
pub struct Encoder<'a, D: ResourceDialect> {
    /// Encoding context.
    pub context: Context,

    /// Buffer to write output data into.
    pub buf: &'a mut Vec<u8>,

    /// Buffer to write output handles into.
    handles: &'a mut Vec<<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition>,

    /// Phantom data for `D`, which is here to provide types not values.
    _dialect: PhantomData<D>,
}

/// The default context for encoding.
#[inline]
fn default_encode_context() -> Context {
    Context { wire_format_version: WireFormatVersion::V2 }
}

impl<'a, D: ResourceDialect> Encoder<'a, D> {
    /// FIDL-encodes `x` into the provided data and handle buffers.
    #[inline]
    pub fn encode<T: TypeMarker>(
        buf: &'a mut Vec<u8>,
        handles: &'a mut Vec<<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition>,
        x: impl Encode<T, D>,
    ) -> Result<()> {
        let context = default_encode_context();
        Self::encode_with_context::<T>(context, buf, handles, x)
    }

    /// FIDL-encodes `x` into the provided data and handle buffers, using the
    /// specified encoding context.
    ///
    /// WARNING: Do not call this directly unless you know what you're doing.
    /// FIDL uses `Context` to coordinate soft migrations, so improper uses of
    /// this function could result in ABI breakage.
    #[inline]
    pub fn encode_with_context<T: TypeMarker>(
        context: Context,
        buf: &'a mut Vec<u8>,
        handles: &'a mut Vec<<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition>,
        x: impl Encode<T, D>,
    ) -> Result<()> {
        fn prepare_for_encoding<'a, D: ResourceDialect>(
            context: Context,
            buf: &'a mut Vec<u8>,
            handles: &'a mut Vec<<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition>,
            ty_inline_size: usize,
        ) -> Encoder<'a, D> {
            // An empty response can have size zero.
            // This if statement is needed to not break the padding write below.
            if ty_inline_size != 0 {
                let aligned_inline_size = round_up_to_align(ty_inline_size, 8);
                // Safety: The uninitialized elements are written by `x.encode`,
                // except for the trailing padding which is zeroed below.
                unsafe {
                    resize_vec_no_zeroing(buf, aligned_inline_size);

                    // Zero the last 8 bytes in the block to ensure padding bytes are zero.
                    let padding_ptr = buf.get_unchecked_mut(aligned_inline_size - 8) as *mut u8;
                    (padding_ptr as *mut u64).write_unaligned(0);
                }
            }
            handles.clear();
            Encoder { buf, handles, context, _dialect: PhantomData }
        }
        let mut encoder = prepare_for_encoding(context, buf, handles, T::inline_size(context));
        // Safety: We reserve `T::inline_size` bytes in `encoder.buf` above.
        unsafe { x.encode(&mut encoder, 0, Depth(0)) }
    }

    /// In debug mode only, asserts that there is enough room in the buffer to
    /// write an object of type `T` at `offset`.
    #[inline(always)]
    pub fn debug_check_bounds<T: TypeMarker>(&self, offset: usize) {
        debug_assert!(offset + T::inline_size(self.context) <= self.buf.len());
    }

    /// Encodes a primitive numeric type.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `self.buf` has room for writing
    /// `T::inline_size` bytes as `offset`.
    #[inline(always)]
    pub unsafe fn write_num<T: numeric::Numeric>(&mut self, num: T, offset: usize) {
        debug_assert!(offset + mem::size_of::<T>() <= self.buf.len());
        // SAFETY: The caller ensures `offset` is valid for writing
        // sizeof(T) bytes. Transmuting to a same-or-wider
        // integer or float pointer is safe because we use `write_unaligned`.
        let ptr = unsafe { self.buf.get_unchecked_mut(offset) } as *mut u8;
        unsafe { (ptr as *mut T).write_unaligned(num) };
    }

    /// Writes the given handle to the handles list.
    #[inline(always)]
    pub fn push_next_handle(
        &mut self,
        handle: <D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition,
    ) {
        self.handles.push(handle)
    }

    /// Returns an offset for writing `len` out-of-line bytes. Zeroes padding
    /// bytes at the end if `len` is not a multiple of 8.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `len` is nonzero.
    #[inline]
    pub unsafe fn out_of_line_offset(&mut self, len: usize) -> usize {
        debug_assert!(len > 0);
        let new_offset = self.buf.len();
        let padded_len = round_up_to_align(len, 8);
        debug_assert!(padded_len >= 8);
        let new_len = self.buf.len() + padded_len;
        unsafe { resize_vec_no_zeroing(self.buf, new_len) };
        // Zero the last 8 bytes in the block to ensure padding bytes are zero.
        // It's more efficient to always write 8 bytes regardless of how much
        // padding is needed because we will overwrite non-padding afterwards.
        let padding_ptr = unsafe { self.buf.get_unchecked_mut(new_len - 8) } as *mut u8;
        unsafe { (padding_ptr as *mut u64).write_unaligned(0) };
        new_offset
    }

    /// Write padding at the specified offset.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `self.buf` has room for writing `len` bytes
    /// as `offset`.
    #[inline(always)]
    pub unsafe fn padding(&mut self, offset: usize, len: usize) {
        if len == 0 {
            return;
        }
        debug_assert!(offset + len <= self.buf.len());
        // Safety:
        // - The caller ensures `offset` is valid for writing `len` bytes.
        // - All u8 pointers are properly aligned.
        unsafe { ptr::write_bytes(self.buf.as_mut_ptr().add(offset), 0, len) };
    }
}

unsafe impl<T: Timeline + 'static, U: 'static> TypeMarker for Instant<T, U> {
    type Owned = Self;

    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        mem::align_of::<Self>()
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        mem::size_of::<Self>()
    }
}

impl<T: Timeline + Copy + 'static, U: Copy + 'static> ValueTypeMarker for Instant<T, U> {
    type Borrowed<'a> = Self;
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        *value
    }
}

unsafe impl<T: Timeline + Copy + 'static, D: ResourceDialect> Encode<Instant<T>, D> for Instant<T> {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<Self>(offset);
        unsafe { encoder.write_num(self.into_nanos(), offset) };
        Ok(())
    }
}

unsafe impl<T: Timeline + 'static, D: ResourceDialect> Encode<Ticks<T>, D> for Ticks<T> {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<Self>(offset);
        unsafe { encoder.write_num(self.into_raw(), offset) };
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
// Decoder
////////////////////////////////////////////////////////////////////////////////

/// Decoding state
#[derive(Debug)]
pub struct Decoder<'a, D: ResourceDialect> {
    /// Decoding context.
    pub context: Context,

    /// Buffer from which to read data.
    pub buf: &'a [u8],

    /// Next out of line block in buf.
    next_out_of_line: usize,

    /// Buffer from which to read handles.
    handles: &'a mut [<D::Handle as HandleFor<D>>::HandleInfo],

    /// Index of the next handle to read from the handle array
    next_handle: usize,

    /// The dialect determines how we encode resources.
    _dialect: PhantomData<D>,
}

impl<'a, D: ResourceDialect> Decoder<'a, D> {
    /// Decodes a value of FIDL type `T` into the Rust type `T::Owned` from the
    /// provided data and handle buffers. Assumes the buffers came from inside a
    /// transaction message wrapped by `header`.
    #[inline]
    pub fn decode_into<T: TypeMarker>(
        header: &TransactionHeader,
        buf: &'a [u8],
        handles: &'a mut [<D::Handle as HandleFor<D>>::HandleInfo],
        value: &mut T::Owned,
    ) -> Result<()>
    where
        T::Owned: Decode<T, D>,
    {
        Self::decode_with_context::<T>(header.decoding_context(), buf, handles, value)
    }

    /// Decodes a value of FIDL type `T` into the Rust type `T::Owned` from the
    /// provided data and handle buffers, using the specified context.
    ///
    /// WARNING: Do not call this directly unless you know what you're doing.
    /// FIDL uses `Context` to coordinate soft migrations, so improper uses of
    /// this function could result in ABI breakage.
    #[inline]
    pub fn decode_with_context<T: TypeMarker>(
        context: Context,
        buf: &'a [u8],
        handles: &'a mut [<D::Handle as HandleFor<D>>::HandleInfo],
        value: &mut T::Owned,
    ) -> Result<()>
    where
        T::Owned: Decode<T, D>,
    {
        let inline_size = T::inline_size(context);
        let next_out_of_line = round_up_to_align(inline_size, 8);
        if next_out_of_line > buf.len() {
            return Err(Error::OutOfRange { expected: next_out_of_line, actual: buf.len() });
        }
        let mut decoder = Decoder {
            next_out_of_line,
            buf,
            handles,
            next_handle: 0,
            context,
            _dialect: PhantomData,
        };
        // Safety: buf.len() >= inline_size based on the check above.
        unsafe {
            value.decode(&mut decoder, 0, Depth(0))?;
        }
        // Safety: next_out_of_line <= buf.len() based on the check above.
        unsafe { decoder.post_decoding(inline_size, next_out_of_line) }
    }

    /// Checks for errors after decoding. This is a separate function to reduce
    /// binary bloat.
    ///
    /// # Safety
    ///
    /// Requires `padding_end <= self.buf.len()`.
    unsafe fn post_decoding(&self, padding_start: usize, padding_end: usize) -> Result<()> {
        if self.next_out_of_line < self.buf.len() {
            return Err(Error::ExtraBytes);
        }
        if self.next_handle < self.handles.len() {
            return Err(Error::ExtraHandles);
        }

        let padding = padding_end - padding_start;
        if padding > 0 {
            // Safety:
            // padding_end <= self.buf.len() is guaranteed by the caller.
            let last_u64 = unsafe {
                let last_u64_ptr = self.buf.get_unchecked(padding_end - 8) as *const u8;
                (last_u64_ptr as *const u64).read_unaligned()
            };
            // padding == 0 => mask == 0x0000000000000000
            // padding == 1 => mask == 0xff00000000000000
            // padding == 2 => mask == 0xffff000000000000
            // ...
            let mask = !(!0u64 >> (padding * 8));
            if last_u64 & mask != 0 {
                return Err(self.end_of_block_padding_error(padding_start, padding_end));
            }
        }

        Ok(())
    }

    /// The position of the next out of line block and the end of the current
    /// blocks.
    #[inline(always)]
    pub fn next_out_of_line(&self) -> usize {
        self.next_out_of_line
    }

    /// The number of handles that have not yet been consumed.
    #[inline(always)]
    pub fn remaining_handles(&self) -> usize {
        self.handles.len() - self.next_handle
    }

    /// In debug mode only, asserts that there is enough room in the buffer to
    /// read an object of type `T` at `offset`.
    #[inline(always)]
    pub fn debug_check_bounds<T: TypeMarker>(&self, offset: usize) {
        debug_assert!(offset + T::inline_size(self.context) <= self.buf.len());
    }

    /// Decodes a primitive numeric type. The caller must ensure that `self.buf`
    /// has room for reading `T::inline_size` bytes as `offset`.
    #[inline(always)]
    pub fn read_num<T: numeric::Numeric>(&mut self, offset: usize) -> T {
        debug_assert!(offset + mem::size_of::<T>() <= self.buf.len());
        // Safety: The caller ensures `offset` is valid for reading
        // sizeof(T) bytes. Transmuting to a same-or-wider
        // integer pointer is safe because we use `read_unaligned`.
        unsafe {
            let ptr = self.buf.get_unchecked(offset) as *const u8;
            (ptr as *const T).read_unaligned()
        }
    }

    /// Returns an offset for reading `len` out-of-line bytes. Validates that
    /// padding bytes at the end are zero if `len` is not a multiple of 8.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `len` is nonzero.
    #[inline(always)]
    pub unsafe fn out_of_line_offset(&mut self, len: usize) -> Result<usize> {
        debug_assert!(len > 0);
        let offset = self.next_out_of_line;
        let aligned_len = round_up_to_align(len, 8);
        self.next_out_of_line += aligned_len;
        debug_assert!(self.next_out_of_line >= 8);
        if self.next_out_of_line > self.buf.len() {
            return Err(Error::OutOfRange {
                expected: aligned_len,
                actual: self.buf.len() - offset,
            });
        }
        // Validate padding bytes at the end of the block.
        // Safety:
        // - The caller ensures `len > 0`, therefore `aligned_len >= 8`.
        // - After `self.next_out_of_line += aligned_len`, we know `self.next_out_of_line >= aligned_len >= 8`.
        // - Therefore `self.next_out_of_line - 8 >= 0` is a valid *const u64.
        let last_u64_ptr =
            unsafe { self.buf.get_unchecked(self.next_out_of_line - 8) } as *const u8;
        let last_u64 = unsafe { (last_u64_ptr as *const u64).read_unaligned() };
        let padding = aligned_len - len;
        // padding == 0 => mask == 0x0000000000000000
        // padding == 1 => mask == 0xff00000000000000
        // padding == 2 => mask == 0xffff000000000000
        // ...
        let mask = !(!0u64 >> (padding * 8));
        if last_u64 & mask != 0 {
            return Err(self.end_of_block_padding_error(offset + len, self.next_out_of_line));
        }

        Ok(offset)
    }

    /// Generates an error for bad padding bytes at the end of a block.
    /// Assumes it is already known that there is a nonzero padding byte.
    fn end_of_block_padding_error(&self, start: usize, end: usize) -> Error {
        for byte in &self.buf[start..end] {
            if *byte != 0 {
                return Error::NonZeroPadding { padding_start: start };
            }
        }
        // This should be unreachable because we only call this after finding
        // nonzero padding. Abort instead of panicking to save code size.
        std::process::abort();
    }

    /// Checks that the specified padding bytes are in fact zeroes. Like
    /// `Decode::decode`, the caller is responsible for bounds checks.
    #[inline]
    pub fn check_padding(&self, offset: usize, len: usize) -> Result<()> {
        if len == 0 {
            // Skip body (so it can be optimized out).
            return Ok(());
        }
        debug_assert!(offset + len <= self.buf.len());
        for i in offset..offset + len {
            // Safety: Caller guarantees offset..offset+len is in bounds.
            if unsafe { *self.buf.get_unchecked(i) } != 0 {
                return Err(Error::NonZeroPadding { padding_start: offset });
            }
        }
        Ok(())
    }

    /// Checks the padding of the inline value portion of an envelope. Like
    /// `Decode::decode`, the caller is responsible for bounds checks.
    ///
    /// Note: `check_padding` could be used instead, but doing so leads to long
    /// compilation times which is why this method exists.
    #[inline]
    pub fn check_inline_envelope_padding(
        &self,
        value_offset: usize,
        value_len: usize,
    ) -> Result<()> {
        // Safety: The caller ensures `value_offset` is valid for reading
        // `value_len` bytes.
        let valid_padding = unsafe {
            match value_len {
                1 => {
                    *self.buf.get_unchecked(value_offset + 1) == 0
                        && *self.buf.get_unchecked(value_offset + 2) == 0
                        && *self.buf.get_unchecked(value_offset + 3) == 0
                }
                2 => {
                    *self.buf.get_unchecked(value_offset + 2) == 0
                        && *self.buf.get_unchecked(value_offset + 3) == 0
                }
                3 => *self.buf.get_unchecked(value_offset + 3) == 0,
                4 => true,
                value_len => unreachable!("value_len={}", value_len),
            }
        };
        if valid_padding {
            Ok(())
        } else {
            Err(Error::NonZeroPadding { padding_start: value_offset + value_len })
        }
    }

    /// Take the next handle from the `handles` list.
    #[inline]
    pub fn take_next_handle(
        &mut self,
        expected_object_type: crate::ObjectType,
        expected_rights: crate::Rights,
    ) -> Result<D::Handle> {
        let Some(next_handle) = self.handles.get_mut(self.next_handle) else {
            return Err(Error::OutOfHandles);
        };
        let handle = next_handle.consume(expected_object_type, expected_rights)?;
        self.next_handle += 1;
        Ok(handle)
    }

    /// Drops the next handle in the handle array.
    #[inline]
    pub fn drop_next_handle(&mut self) -> Result<()> {
        let Some(next_handle) = self.handles.get_mut(self.next_handle) else {
            return Err(Error::OutOfHandles);
        };
        next_handle.drop_in_place();
        self.next_handle += 1;
        Ok(())
    }
}

impl<T: Timeline + 'static, D: ResourceDialect> Decode<Self, D> for Instant<T> {
    #[inline(always)]
    fn new_empty() -> Self {
        Instant::ZERO
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Self>(offset);
        *self = Self::from_nanos(decoder.read_num(offset));
        Ok(())
    }
}

impl<T: Timeline + 'static, D: ResourceDialect> Decode<Self, D> for Ticks<T> {
    #[inline(always)]
    fn new_empty() -> Self {
        Ticks::<T>::ZERO
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Self>(offset);
        *self = Self::from_raw(decoder.read_num(offset));
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
// Ambiguous types
////////////////////////////////////////////////////////////////////////////////

/// A fake FIDL type that can encode from and decode into any Rust type.
///
/// This exists solely to prevent the compiler from inferring `T: TypeMarker`,
/// allowing us to add new generic impls without source breakage. It also
/// improves error messages when no suitable `T: TypeMarker` exists, preventing
/// spurious guesses about what you should do (e.g. implement `HandleBased`).
pub struct Ambiguous1;

/// Like `Ambiguous1`. There needs to be two of these types so that the compiler
/// doesn't infer one of them and generate a call to the panicking methods.
pub struct Ambiguous2;

/// An uninhabited type used as owned and borrowed type for ambiguous markers.
/// Can be replaced by `!` once that is stable.
pub enum AmbiguousNever {}

macro_rules! impl_ambiguous {
    ($ambiguous:ident) => {
        unsafe impl TypeMarker for $ambiguous {
            type Owned = AmbiguousNever;

            fn inline_align(_context: Context) -> usize {
                panic!("reached code for fake ambiguous type");
            }

            fn inline_size(_context: Context) -> usize {
                panic!("reached code for fake ambiguous type");
            }
        }

        impl ValueTypeMarker for $ambiguous {
            type Borrowed<'a> = AmbiguousNever;

            fn borrow(value: &<Self as TypeMarker>::Owned) -> Self::Borrowed<'_> {
                match *value {}
            }
        }

        impl ResourceTypeMarker for $ambiguous {
            type Borrowed<'a> = AmbiguousNever;
            fn take_or_borrow(value: &mut <Self as TypeMarker>::Owned) -> Self::Borrowed<'_>
            where
                Self: TypeMarker,
            {
                match *value {}
            }
        }

        unsafe impl<T, D: ResourceDialect> Encode<$ambiguous, D> for T {
            unsafe fn encode(
                self,
                _encoder: &mut Encoder<'_, D>,
                _offset: usize,
                _depth: Depth,
            ) -> Result<()> {
                panic!("reached code for fake ambiguous type");
            }
        }

        // TODO(https://fxbug.dev/42069855): impl for `T: 'static` this once user code has
        // migrated off new_empty(), which is meant to be internal.
        impl<D: ResourceDialect> Decode<$ambiguous, D> for AmbiguousNever {
            fn new_empty() -> Self {
                panic!("reached code for fake ambiguous type");
            }

            unsafe fn decode(
                &mut self,
                _decoder: &mut Decoder<'_, D>,
                _offset: usize,
                _depth: Depth,
            ) -> Result<()> {
                match *self {}
            }
        }
    };
}

impl_ambiguous!(Ambiguous1);
impl_ambiguous!(Ambiguous2);

////////////////////////////////////////////////////////////////////////////////
// Empty types
////////////////////////////////////////////////////////////////////////////////

/// A FIDL type representing an empty payload (0 bytes).
pub struct EmptyPayload;

unsafe impl TypeMarker for EmptyPayload {
    type Owned = ();
    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        1
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        0
    }
}

impl ValueTypeMarker for EmptyPayload {
    type Borrowed<'a> = ();
    #[inline(always)]
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        *value
    }
}

unsafe impl<D: ResourceDialect> Encode<EmptyPayload, D> for () {
    #[inline(always)]
    unsafe fn encode(
        self,
        _encoder: &mut Encoder<'_, D>,
        _offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        Ok(())
    }
}

impl<D: ResourceDialect> Decode<EmptyPayload, D> for () {
    #[inline(always)]
    fn new_empty() -> Self {}

    #[inline(always)]
    unsafe fn decode(
        &mut self,
        _decoder: &mut Decoder<'_, D>,
        _offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        Ok(())
    }
}

/// The FIDL type used for an empty success variant in a result union. Result
/// unions occur in two-way methods that are flexible or that use error syntax.
pub struct EmptyStruct;

unsafe impl TypeMarker for EmptyStruct {
    type Owned = ();
    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        1
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        1
    }
}

impl ValueTypeMarker for EmptyStruct {
    type Borrowed<'a> = ();
    #[inline(always)]
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        *value
    }
}

unsafe impl<D: ResourceDialect> Encode<EmptyStruct, D> for () {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<EmptyStruct>(offset);
        unsafe { encoder.write_num(0u8, offset) };
        Ok(())
    }
}

impl<D: ResourceDialect> Decode<EmptyStruct, D> for () {
    #[inline(always)]
    fn new_empty() -> Self {}

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<EmptyStruct>(offset);
        match decoder.read_num::<u8>(offset) {
            0 => Ok(()),
            _ => Err(Error::Invalid),
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// Primitive types
////////////////////////////////////////////////////////////////////////////////

// Private module to prevent others from implementing `Numeric`.
mod numeric {
    use super::*;

    /// Marker trait for primitive numeric types.
    pub trait Numeric {}

    /// Implements `Numeric`, `TypeMarker`, `ValueTypeMarker`, `Encode`, and
    /// `Decode` for a primitive numeric type (integer or float).
    macro_rules! impl_numeric {
        ($numeric_ty:ty) => {
            impl Numeric for $numeric_ty {}

            unsafe impl TypeMarker for $numeric_ty {
                type Owned = $numeric_ty;
                #[inline(always)]
                fn inline_align(_context: Context) -> usize {
                    mem::align_of::<$numeric_ty>()
                }

                #[inline(always)]
                fn inline_size(_context: Context) -> usize {
                    mem::size_of::<$numeric_ty>()
                }

                #[inline(always)]
                fn encode_is_copy() -> bool {
                    true
                }

                #[inline(always)]
                fn decode_is_copy() -> bool {
                    true
                }
            }

            impl ValueTypeMarker for $numeric_ty {
                type Borrowed<'a> = $numeric_ty;

                #[inline(always)]
                fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
                    *value
                }
            }

            unsafe impl<D: ResourceDialect> Encode<$numeric_ty, D> for $numeric_ty {
                #[inline(always)]
                unsafe fn encode(
                    self,
                    encoder: &mut Encoder<'_, D>,
                    offset: usize,
                    _depth: Depth,
                ) -> Result<()> {
                    encoder.debug_check_bounds::<$numeric_ty>(offset);
                    unsafe { encoder.write_num::<$numeric_ty>(self, offset) };
                    Ok(())
                }
            }

            impl<D: ResourceDialect> Decode<$numeric_ty, D> for $numeric_ty {
                #[inline(always)]
                fn new_empty() -> Self {
                    0 as $numeric_ty
                }

                #[inline(always)]
                unsafe fn decode(
                    &mut self,
                    decoder: &mut Decoder<'_, D>,
                    offset: usize,
                    _depth: Depth,
                ) -> Result<()> {
                    decoder.debug_check_bounds::<$numeric_ty>(offset);
                    *self = decoder.read_num::<$numeric_ty>(offset);
                    Ok(())
                }
            }
        };
    }

    impl_numeric!(u8);
    impl_numeric!(u16);
    impl_numeric!(u32);
    impl_numeric!(u64);
    impl_numeric!(i8);
    impl_numeric!(i16);
    impl_numeric!(i32);
    impl_numeric!(i64);
    impl_numeric!(f32);
    impl_numeric!(f64);
}

unsafe impl TypeMarker for bool {
    type Owned = bool;

    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        mem::align_of::<bool>()
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        mem::size_of::<bool>()
    }

    #[inline(always)]
    fn encode_is_copy() -> bool {
        // Rust guarantees a bool is 0x00 or 0x01.
        // https://doc.rust-lang.org/reference/types/boolean.html
        true
    }

    #[inline(always)]
    fn decode_is_copy() -> bool {
        // Decoding isn't just a copy because we have to ensure it's 0x00 or 0x01.
        false
    }
}

impl ValueTypeMarker for bool {
    type Borrowed<'a> = bool;

    #[inline(always)]
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        *value
    }
}

unsafe impl<D: ResourceDialect> Encode<bool, D> for bool {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<bool>(offset);
        // From https://doc.rust-lang.org/std/primitive.bool.html: "If you
        // cast a bool into an integer, true will be 1 and false will be 0."
        unsafe { encoder.write_num(self as u8, offset) };
        Ok(())
    }
}

impl<D: ResourceDialect> Decode<bool, D> for bool {
    #[inline(always)]
    fn new_empty() -> Self {
        false
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<bool>(offset);
        // Safety: The caller ensures `offset` is valid for reading 1 byte.
        *self = match unsafe { *decoder.buf.get_unchecked(offset) } {
            0 => false,
            1 => true,
            _ => return Err(Error::InvalidBoolean),
        };
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
// Arrays
////////////////////////////////////////////////////////////////////////////////

/// The FIDL type `array<T, N>`.
pub struct Array<T: TypeMarker, const N: usize>(PhantomData<T>);

unsafe impl<T: TypeMarker, const N: usize> TypeMarker for Array<T, N> {
    type Owned = [T::Owned; N];
    #[inline(always)]
    fn inline_align(context: Context) -> usize {
        T::inline_align(context)
    }

    #[inline(always)]
    fn inline_size(context: Context) -> usize {
        N * T::inline_size(context)
    }

    #[inline(always)]
    fn encode_is_copy() -> bool {
        T::encode_is_copy()
    }

    #[inline(always)]
    fn decode_is_copy() -> bool {
        T::decode_is_copy()
    }
}

impl<T: ValueTypeMarker, const N: usize> ValueTypeMarker for Array<T, N> {
    type Borrowed<'a> = &'a [T::Owned; N];
    #[inline(always)]
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        value
    }
}

impl<T: ResourceTypeMarker, const N: usize> ResourceTypeMarker for Array<T, N> {
    type Borrowed<'a> = &'a mut [T::Owned; N];
    #[inline(always)]
    fn take_or_borrow(value: &mut <Self as TypeMarker>::Owned) -> Self::Borrowed<'_> {
        value
    }
}

unsafe impl<T: ValueTypeMarker, const N: usize, D: ResourceDialect> Encode<Array<T, N>, D>
    for &[T::Owned; N]
where
    for<'q> T::Borrowed<'q>: Encode<T, D>,
{
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<Array<T, N>>(offset);
        unsafe { encode_array_value::<T, D>(self, encoder, offset, depth) }
    }
}

unsafe impl<T: ResourceTypeMarker, const N: usize, D: ResourceDialect> Encode<Array<T, N>, D>
    for &mut [<T as TypeMarker>::Owned; N]
where
    for<'q> T::Borrowed<'q>: Encode<T, D>,
{
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<Array<T, N>>(offset);
        unsafe { encode_array_resource::<T, D>(self, encoder, offset, depth) }
    }
}

impl<T: TypeMarker, const N: usize, D: ResourceDialect> Decode<Array<T, N>, D> for [T::Owned; N]
where
    T::Owned: Decode<T, D>,
{
    #[inline]
    fn new_empty() -> Self {
        let mut arr = mem::MaybeUninit::<[T::Owned; N]>::uninit();
        unsafe {
            let arr_ptr = arr.as_mut_ptr() as *mut T::Owned;
            for i in 0..N {
                ptr::write(arr_ptr.add(i), T::Owned::new_empty());
            }
            arr.assume_init()
        }
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Array<T, N>>(offset);
        unsafe { decode_array::<T, D>(self, decoder, offset, depth) }
    }
}

#[inline]
unsafe fn encode_array_value<T: ValueTypeMarker, D: ResourceDialect>(
    slice: &[T::Owned],
    encoder: &mut Encoder<'_, D>,
    offset: usize,
    depth: Depth,
) -> Result<()>
where
    for<'a> T::Borrowed<'a>: Encode<T, D>,
{
    let stride = T::inline_size(encoder.context);
    let len = slice.len();
    // Not a safety requirement, but len should be nonzero since FIDL does not allow empty arrays.
    debug_assert_ne!(len, 0);
    if T::encode_is_copy() {
        debug_assert_eq!(stride, mem::size_of::<T::Owned>());
        // Safety:
        // - The caller ensures `offset` if valid for writing `stride` bytes
        //   (inline size of `T`) `len` times, i.e. `len * stride`.
        // - Since T::inline_size is the same as mem::size_of for simple
        //   copy types, `slice` also has exactly `len * stride` bytes.
        // - Rust guarantees `slice` and `encoder.buf` do not alias.
        unsafe {
            let src = slice.as_ptr() as *const u8;
            let dst: *mut u8 = encoder.buf.as_mut_ptr().add(offset);
            ptr::copy_nonoverlapping(src, dst, len * stride);
        }
    } else {
        for i in 0..len {
            // SAFETY: `i` is in bounds since `len` is defined as `slice.len()`.
            let item = unsafe { slice.get_unchecked(i) };
            unsafe { T::borrow(item).encode(encoder, offset + i * stride, depth)? };
        }
    }
    Ok(())
}

#[inline]
unsafe fn encode_array_resource<T: ResourceTypeMarker + TypeMarker, D: ResourceDialect>(
    slice: &mut [T::Owned],
    encoder: &mut Encoder<'_, D>,
    offset: usize,
    depth: Depth,
) -> Result<()>
where
    for<'a> T::Borrowed<'a>: Encode<T, D>,
{
    let stride = T::inline_size(encoder.context);
    let len = slice.len();
    // Not a safety requirement, but len should be nonzero since FIDL does not allow empty arrays.
    debug_assert_ne!(len, 0);
    if T::encode_is_copy() {
        debug_assert_eq!(stride, mem::size_of::<T::Owned>());
        // Safety:
        // - The caller ensures `offset` if valid for writing `stride` bytes
        //   (inline size of `T`) `len` times, i.e. `len * stride`.
        // - Since T::inline_size is the same as mem::size_of for simple
        //   copy types, `slice` also has exactly `len * stride` bytes.
        // - Rust guarantees `slice` and `encoder.buf` do not alias.
        unsafe {
            let src = slice.as_ptr() as *const u8;
            let dst: *mut u8 = encoder.buf.as_mut_ptr().add(offset);
            ptr::copy_nonoverlapping(src, dst, len * stride);
        }
    } else {
        for i in 0..len {
            // SAFETY: `i` is in bounds since `len` is defined as `slice.len()`.
            let item = unsafe { slice.get_unchecked_mut(i) };
            unsafe { T::take_or_borrow(item).encode(encoder, offset + i * stride, depth)? };
        }
    }
    Ok(())
}

#[inline]
unsafe fn decode_array<T: TypeMarker, D: ResourceDialect>(
    slice: &mut [T::Owned],
    decoder: &mut Decoder<'_, D>,
    offset: usize,
    depth: Depth,
) -> Result<()>
where
    T::Owned: Decode<T, D>,
{
    let stride = T::inline_size(decoder.context);
    let len = slice.len();
    // Not a safety requirement, but len should be nonzero since FIDL does not allow empty arrays.
    debug_assert_ne!(len, 0);
    if T::decode_is_copy() {
        debug_assert_eq!(stride, mem::size_of::<T::Owned>());
        // Safety:
        // - The caller ensures `offset` if valid for reading `stride` bytes
        //   (inline size of `T`) `len` times, i.e. `len * stride`.
        // - Since T::inline_size is the same as mem::size_of for simple copy
        //   types, `slice` also has exactly `len * stride` bytes.
        // - Rust guarantees `slice` and `decoder.buf` do not alias.
        unsafe {
            let src: *const u8 = decoder.buf.as_ptr().add(offset);
            let dst = slice.as_mut_ptr() as *mut u8;
            ptr::copy_nonoverlapping(src, dst, len * stride);
        }
    } else {
        for i in 0..len {
            // SAFETY: `i` is in bounds since `len` is defined as `slice.len()`.
            let item = unsafe { slice.get_unchecked_mut(i) };
            unsafe { item.decode(decoder, offset + i * stride, depth)? };
        }
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
// Vectors
////////////////////////////////////////////////////////////////////////////////

/// The maximum vector bound, corresponding to the `MAX` constraint in FIDL.
pub const MAX_BOUND: usize = usize::MAX;

/// The FIDL type `vector<T>:N`.
pub struct Vector<T: TypeMarker, const N: usize>(PhantomData<T>);

/// The FIDL type `vector<T>` or `vector<T>:MAX`.
pub type UnboundedVector<T> = Vector<T, MAX_BOUND>;

unsafe impl<T: TypeMarker, const N: usize> TypeMarker for Vector<T, N> {
    type Owned = Vec<T::Owned>;

    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        8
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        16
    }
}

impl<T: ValueTypeMarker, const N: usize> ValueTypeMarker for Vector<T, N> {
    type Borrowed<'a> = &'a [T::Owned];
    #[inline(always)]
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        value
    }
}

impl<T: ResourceTypeMarker, const N: usize> ResourceTypeMarker for Vector<T, N> {
    type Borrowed<'a> = &'a mut [T::Owned];
    #[inline(always)]
    fn take_or_borrow(value: &mut <Self as TypeMarker>::Owned) -> Self::Borrowed<'_> {
        value.as_mut_slice()
    }
}

unsafe impl<T: ValueTypeMarker, const N: usize, D: ResourceDialect> Encode<Vector<T, N>, D>
    for &[<T as TypeMarker>::Owned]
where
    for<'q> T::Borrowed<'q>: Encode<T, D>,
{
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<Vector<T, N>>(offset);
        unsafe { encode_vector_value::<T, D>(self, N, check_vector_length, encoder, offset, depth) }
    }
}

unsafe impl<T: ResourceTypeMarker + TypeMarker, const N: usize, D: ResourceDialect>
    Encode<Vector<T, N>, D> for &mut [<T as TypeMarker>::Owned]
where
    for<'q> T::Borrowed<'q>: Encode<T, D>,
{
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<Vector<T, N>>(offset);
        unsafe { encode_vector_resource::<T, D>(self, N, encoder, offset, depth) }
    }
}

impl<T: TypeMarker, const N: usize, D: ResourceDialect> Decode<Vector<T, N>, D> for Vec<T::Owned>
where
    T::Owned: Decode<T, D>,
{
    #[inline(always)]
    fn new_empty() -> Self {
        Vec::new()
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Vector<T, N>>(offset);
        unsafe { decode_vector::<T, D>(self, N, decoder, offset, depth) }
    }
}

#[inline]
unsafe fn encode_vector_value<T: ValueTypeMarker, D: ResourceDialect>(
    slice: &[<T as TypeMarker>::Owned],
    max_length: usize,
    check_length: impl Fn(usize, usize) -> Result<()>,
    encoder: &mut Encoder<'_, D>,
    offset: usize,
    mut depth: Depth,
) -> Result<()>
where
    for<'a> T::Borrowed<'a>: Encode<T, D>,
{
    unsafe { encoder.write_num(slice.len() as u64, offset) };
    unsafe { encoder.write_num(fidl_constants::ALLOC_PRESENT_U64, offset + 8) };
    // Calling encoder.out_of_line_offset(0) is not allowed.
    if slice.is_empty() {
        return Ok(());
    }
    check_length(slice.len(), max_length)?;
    depth.increment()?;
    let bytes_len = slice.len() * T::inline_size(encoder.context);
    let offset = unsafe { encoder.out_of_line_offset(bytes_len) };
    unsafe { encode_array_value::<T, D>(slice, encoder, offset, depth) }
}

#[inline]
unsafe fn encode_vector_resource<T: ResourceTypeMarker + TypeMarker, D: ResourceDialect>(
    slice: &mut [T::Owned],
    max_length: usize,
    encoder: &mut Encoder<'_, D>,
    offset: usize,
    mut depth: Depth,
) -> Result<()>
where
    for<'a> T::Borrowed<'a>: Encode<T, D>,
{
    unsafe { encoder.write_num(slice.len() as u64, offset) };
    unsafe { encoder.write_num(ALLOC_PRESENT_U64, offset + 8) };
    // Calling encoder.out_of_line_offset(0) is not allowed.
    if slice.is_empty() {
        return Ok(());
    }
    check_vector_length(slice.len(), max_length)?;
    depth.increment()?;
    let bytes_len = slice.len() * T::inline_size(encoder.context);
    let offset = unsafe { encoder.out_of_line_offset(bytes_len) };
    unsafe { encode_array_resource::<T, D>(slice, encoder, offset, depth) }
}

#[inline]
unsafe fn decode_vector<T: TypeMarker, D: ResourceDialect>(
    vec: &mut Vec<T::Owned>,
    max_length: usize,
    decoder: &mut Decoder<'_, D>,
    offset: usize,
    mut depth: Depth,
) -> Result<()>
where
    T::Owned: Decode<T, D>,
{
    let Some(len) = decode_vector_header(decoder, offset)? else {
        return Err(Error::NotNullable);
    };
    // Calling decoder.out_of_line_offset(0) is not allowed.
    if len == 0 {
        return Ok(());
    }
    check_vector_length(len, max_length)?;
    depth.increment()?;
    let bytes_len = len * T::inline_size(decoder.context);
    let offset = unsafe { decoder.out_of_line_offset(bytes_len)? };
    if T::decode_is_copy() {
        // Safety: The uninitialized elements are immediately written by
        // `decode_array`, which always succeeds in the simple copy case.
        unsafe {
            resize_vec_no_zeroing(vec, len);
        }
    } else {
        vec.resize_with(len, T::Owned::new_empty);
    }
    // Safety: `vec` has `len` elements based on the above code.
    unsafe { decode_array::<T, D>(vec, decoder, offset, depth)? };
    Ok(())
}

/// Decodes and validates a 16-byte vector header. Returns `Some(len)` if
/// the vector is present (including empty vectors), otherwise `None`.
#[doc(hidden)] // only exported for use in macros or generated code
#[inline]
pub fn decode_vector_header<D: ResourceDialect>(
    decoder: &mut Decoder<'_, D>,
    offset: usize,
) -> Result<Option<usize>> {
    let len = decoder.read_num::<u64>(offset) as usize;
    match decoder.read_num::<u64>(offset + 8) {
        ALLOC_PRESENT_U64 => {
            // Check that the length does not exceed `u32::MAX` (per RFC-0059)
            // nor the total size of the message (to avoid a huge allocation
            // when the message cannot possibly be valid).
            if len <= u32::MAX as usize && len <= decoder.buf.len() {
                Ok(Some(len))
            } else {
                Err(Error::OutOfRange { expected: len, actual: decoder.buf.len() })
            }
        }
        ALLOC_ABSENT_U64 => {
            if len == 0 {
                Ok(None)
            } else {
                Err(Error::UnexpectedNullRef)
            }
        }
        _ => Err(Error::InvalidPresenceIndicator),
    }
}

#[inline(always)]
fn check_vector_length(actual_length: usize, max_length: usize) -> Result<()> {
    if actual_length > max_length {
        return Err(Error::VectorTooLong { max_length, actual_length });
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
// Strings
////////////////////////////////////////////////////////////////////////////////

/// The FIDL type `string:N`.
pub struct BoundedString<const N: usize>;

/// The FIDL type `string` or `string:MAX`.
pub type UnboundedString = BoundedString<MAX_BOUND>;

unsafe impl<const N: usize> TypeMarker for BoundedString<N> {
    type Owned = String;

    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        8
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        16
    }
}

impl<const N: usize> ValueTypeMarker for BoundedString<N> {
    type Borrowed<'a> = &'a str;
    #[inline(always)]
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        value
    }
}

unsafe impl<const N: usize, D: ResourceDialect> Encode<BoundedString<N>, D> for &str {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<BoundedString<N>>(offset);
        unsafe {
            encode_vector_value::<u8, D>(
                self.as_bytes(),
                N,
                check_string_length,
                encoder,
                offset,
                depth,
            )
        }
    }
}

impl<const N: usize, D: ResourceDialect> Decode<BoundedString<N>, D> for String {
    #[inline(always)]
    fn new_empty() -> Self {
        String::new()
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<BoundedString<N>>(offset);
        decode_string(self, N, decoder, offset, depth)
    }
}

#[inline]
fn decode_string<D: ResourceDialect>(
    string: &mut String,
    max_length: usize,
    decoder: &mut Decoder<'_, D>,
    offset: usize,
    mut depth: Depth,
) -> Result<()> {
    let Some(len) = decode_vector_header(decoder, offset)? else {
        return Err(Error::NotNullable);
    };
    // Calling decoder.out_of_line_offset(0) is not allowed.
    if len == 0 {
        return Ok(());
    }
    check_string_length(len, max_length)?;
    depth.increment()?;
    // Safety: we return early above if `len == 0`.
    let offset = unsafe { decoder.out_of_line_offset(len)? };
    // Safety: `out_of_line_offset` does this bounds check.
    let bytes = unsafe { &decoder.buf.get_unchecked(offset..offset + len) };
    let utf8 = str::from_utf8(bytes).map_err(|_| Error::Utf8Error)?;
    let boxed_utf8: Box<str> = utf8.into();
    *string = boxed_utf8.into_string();
    Ok(())
}

#[inline(always)]
fn check_string_length(actual_bytes: usize, max_bytes: usize) -> Result<()> {
    if actual_bytes > max_bytes {
        return Err(Error::StringTooLong { max_bytes, actual_bytes });
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
// Handles
////////////////////////////////////////////////////////////////////////////////

impl<T: From<NullableHandle> + Into<NullableHandle>> EncodableAsHandle for T {
    type Dialect = DefaultFuchsiaResourceDialect;
}

/// The FIDL type `zx.Handle:<OBJECT_TYPE, RIGHTS>`, or a `client_end` or `server_end`.
pub struct HandleType<T: EncodableAsHandle, const OBJECT_TYPE: u32, const RIGHTS: u32>(
    PhantomData<T>,
);

/// An abbreviation of `HandleType` that for channels with default rights, used
/// for the FIDL types `client_end:P` and `server_end:P`.
pub type Endpoint<T> = HandleType<
    T,
    { crate::ObjectType::CHANNEL.into_raw() },
    { crate::Rights::CHANNEL_DEFAULT.bits() },
>;

unsafe impl<T: 'static + EncodableAsHandle, const OBJECT_TYPE: u32, const RIGHTS: u32> TypeMarker
    for HandleType<T, OBJECT_TYPE, RIGHTS>
{
    type Owned = T;

    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        4
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        4
    }
}

impl<T: 'static + EncodableAsHandle, const OBJECT_TYPE: u32, const RIGHTS: u32> ResourceTypeMarker
    for HandleType<T, OBJECT_TYPE, RIGHTS>
{
    type Borrowed<'a> = T;
    #[inline(always)]
    fn take_or_borrow(value: &mut <Self as TypeMarker>::Owned) -> Self::Borrowed<'_> {
        mem::replace(value, <T::Dialect as ResourceDialect>::Handle::invalid().into())
    }
}

unsafe impl<T: 'static + EncodableAsHandle, const OBJECT_TYPE: u32, const RIGHTS: u32>
    Encode<HandleType<T, OBJECT_TYPE, RIGHTS>, T::Dialect> for T
{
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, T::Dialect>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<HandleType<T, OBJECT_TYPE, RIGHTS>>(offset);
        unsafe {
            encode_handle(
                self.into(),
                crate::ObjectType::from_raw(OBJECT_TYPE),
                crate::Rights::from_bits_retain(RIGHTS),
                encoder,
                offset,
            )
        }
    }
}

impl<T: 'static + EncodableAsHandle, const OBJECT_TYPE: u32, const RIGHTS: u32>
    Decode<HandleType<T, OBJECT_TYPE, RIGHTS>, T::Dialect> for T
{
    #[inline(always)]
    fn new_empty() -> Self {
        <T::Dialect as ResourceDialect>::Handle::invalid().into()
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, T::Dialect>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<HandleType<T, OBJECT_TYPE, RIGHTS>>(offset);
        *self = unsafe {
            decode_handle(
                crate::ObjectType::from_raw(OBJECT_TYPE),
                crate::Rights::from_bits_retain(RIGHTS),
                decoder,
                offset,
            )?
        }
        .into();
        Ok(())
    }
}

#[inline]
unsafe fn encode_handle<D: ResourceDialect>(
    handle: D::Handle,
    object_type: crate::ObjectType,
    rights: crate::Rights,
    encoder: &mut Encoder<'_, D>,
    offset: usize,
) -> Result<()> {
    if handle.is_invalid() {
        return Err(Error::NotNullable);
    }
    unsafe { encoder.write_num(ALLOC_PRESENT_U32, offset) };
    encoder.handles.push(<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition::from_handle(
        handle,
        object_type,
        rights,
    ));
    Ok(())
}

#[inline]
unsafe fn decode_handle<D: ResourceDialect>(
    object_type: crate::ObjectType,
    rights: crate::Rights,
    decoder: &mut Decoder<'_, D>,
    offset: usize,
) -> Result<D::Handle> {
    match decoder.read_num::<u32>(offset) {
        ALLOC_PRESENT_U32 => {}
        ALLOC_ABSENT_U32 => return Err(Error::NotNullable),
        _ => return Err(Error::InvalidPresenceIndicator),
    }
    decoder.take_next_handle(object_type, rights)
}

////////////////////////////////////////////////////////////////////////////////
// Optionals
////////////////////////////////////////////////////////////////////////////////

/// The FIDL type `T:optional` where `T` is a vector, string, handle, or client/server end.
pub struct Optional<T: TypeMarker>(PhantomData<T>);

/// The FIDL type `T:optional` where `T` is a union.
pub struct OptionalUnion<T: TypeMarker>(PhantomData<T>);

/// The FIDL type `box<T>`.
pub struct Boxed<T: TypeMarker>(PhantomData<T>);

unsafe impl<T: TypeMarker> TypeMarker for Optional<T> {
    type Owned = Option<T::Owned>;

    #[inline(always)]
    fn inline_align(context: Context) -> usize {
        T::inline_align(context)
    }

    #[inline(always)]
    fn inline_size(context: Context) -> usize {
        T::inline_size(context)
    }
}

unsafe impl<T: TypeMarker> TypeMarker for OptionalUnion<T> {
    type Owned = Option<Box<T::Owned>>;

    #[inline(always)]
    fn inline_align(context: Context) -> usize {
        T::inline_align(context)
    }

    #[inline(always)]
    fn inline_size(context: Context) -> usize {
        T::inline_size(context)
    }
}

unsafe impl<T: TypeMarker> TypeMarker for Boxed<T> {
    type Owned = Option<Box<T::Owned>>;

    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        8
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        8
    }
}

impl<T: ValueTypeMarker> ValueTypeMarker for Optional<T> {
    type Borrowed<'a> = Option<T::Borrowed<'a>>;

    #[inline(always)]
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        value.as_ref().map(T::borrow)
    }
}

impl<T: ValueTypeMarker> ValueTypeMarker for OptionalUnion<T> {
    type Borrowed<'a> = Option<T::Borrowed<'a>>;

    #[inline(always)]
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        value.as_deref().map(T::borrow)
    }
}

impl<T: ValueTypeMarker> ValueTypeMarker for Boxed<T> {
    type Borrowed<'a> = Option<T::Borrowed<'a>>;

    #[inline(always)]
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        value.as_deref().map(T::borrow)
    }
}

impl<T: ResourceTypeMarker + TypeMarker> ResourceTypeMarker for Optional<T> {
    type Borrowed<'a> = Option<T::Borrowed<'a>>;
    #[inline(always)]
    fn take_or_borrow(value: &mut <Self as TypeMarker>::Owned) -> Self::Borrowed<'_> {
        value.as_mut().map(T::take_or_borrow)
    }
}

impl<T: ResourceTypeMarker + TypeMarker> ResourceTypeMarker for OptionalUnion<T> {
    type Borrowed<'a> = Option<T::Borrowed<'a>>;
    #[inline(always)]
    fn take_or_borrow(value: &mut <Self as TypeMarker>::Owned) -> Self::Borrowed<'_> {
        value.as_deref_mut().map(T::take_or_borrow)
    }
}

impl<T: ResourceTypeMarker + TypeMarker> ResourceTypeMarker for Boxed<T> {
    type Borrowed<'a> = Option<T::Borrowed<'a>>;
    #[inline(always)]
    fn take_or_borrow(value: &mut <Self as TypeMarker>::Owned) -> Self::Borrowed<'_> {
        value.as_deref_mut().map(T::take_or_borrow)
    }
}

unsafe impl<T: TypeMarker, E: Encode<T, D>, D: ResourceDialect> Encode<Optional<T>, D>
    for Option<E>
{
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<Optional<T>>(offset);
        unsafe { encode_naturally_optional::<T, E, D>(self, encoder, offset, depth) }
    }
}

unsafe impl<T: TypeMarker, E: Encode<T, D>, D: ResourceDialect> Encode<OptionalUnion<T>, D>
    for Option<E>
{
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<OptionalUnion<T>>(offset);
        unsafe { encode_naturally_optional::<T, E, D>(self, encoder, offset, depth) }
    }
}

unsafe impl<T: TypeMarker, E: Encode<T, D>, D: ResourceDialect> Encode<Boxed<T>, D> for Option<E> {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        mut depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<Boxed<T>>(offset);
        match self {
            Some(val) => {
                depth.increment()?;
                unsafe { encoder.write_num(ALLOC_PRESENT_U64, offset) };
                let offset = unsafe { encoder.out_of_line_offset(T::inline_size(encoder.context)) };
                unsafe { val.encode(encoder, offset, depth)? };
            }
            None => unsafe { encoder.write_num(ALLOC_ABSENT_U64, offset) },
        }
        Ok(())
    }
}

impl<T: TypeMarker, D: ResourceDialect> Decode<Optional<T>, D> for Option<T::Owned>
where
    T::Owned: Decode<T, D>,
{
    #[inline(always)]
    fn new_empty() -> Self {
        None
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Optional<T>>(offset);
        let inline_size = T::inline_size(decoder.context);
        if check_for_presence(decoder, offset, inline_size) {
            unsafe { self.get_or_insert(T::Owned::new_empty()).decode(decoder, offset, depth) }
        } else {
            *self = None;
            decoder.check_padding(offset, inline_size)?;
            Ok(())
        }
    }
}

impl<T: TypeMarker, D: ResourceDialect> Decode<OptionalUnion<T>, D> for Option<Box<T::Owned>>
where
    T::Owned: Decode<T, D>,
{
    #[inline(always)]
    fn new_empty() -> Self {
        None
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<OptionalUnion<T>>(offset);
        let inline_size = T::inline_size(decoder.context);
        if check_for_presence(decoder, offset, inline_size) {
            unsafe {
                decode!(
                    T,
                    self.get_or_insert_with(|| Box::new(T::Owned::new_empty())),
                    decoder,
                    offset,
                    depth
                )
            }
        } else {
            *self = None;
            decoder.check_padding(offset, inline_size)?;
            Ok(())
        }
    }
}

impl<T: TypeMarker, D: ResourceDialect> Decode<Boxed<T>, D> for Option<Box<T::Owned>>
where
    T::Owned: Decode<T, D>,
{
    #[inline(always)]
    fn new_empty() -> Self {
        None
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        mut depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Boxed<T>>(offset);
        match decoder.read_num::<u64>(offset) {
            ALLOC_PRESENT_U64 => {
                depth.increment()?;
                let offset =
                    unsafe { decoder.out_of_line_offset(T::inline_size(decoder.context))? };
                unsafe {
                    decode!(
                        T,
                        self.get_or_insert_with(|| Box::new(T::Owned::new_empty())),
                        decoder,
                        offset,
                        depth
                    )?
                };
                Ok(())
            }
            ALLOC_ABSENT_U64 => {
                *self = None;
                Ok(())
            }
            _ => Err(Error::InvalidPresenceIndicator),
        }
    }
}

/// Encodes a "naturally optional" value, i.e. one where absence is represented
/// by a run of 0x00 bytes matching the type's inline size.
#[inline]
unsafe fn encode_naturally_optional<T: TypeMarker, E: Encode<T, D>, D: ResourceDialect>(
    value: Option<E>,
    encoder: &mut Encoder<'_, D>,
    offset: usize,
    depth: Depth,
) -> Result<()> {
    match value {
        Some(val) => unsafe { val.encode(encoder, offset, depth)? },
        None => unsafe { encoder.padding(offset, T::inline_size(encoder.context)) },
    }
    Ok(())
}

/// Presence indicators always include at least one non-zero byte, while absence
/// indicators should always be entirely zeros. Like `Decode::decode`, the
/// caller is responsible for bounds checks.
#[inline]
fn check_for_presence<D: ResourceDialect>(
    decoder: &Decoder<'_, D>,
    offset: usize,
    inline_size: usize,
) -> bool {
    debug_assert!(offset + inline_size <= decoder.buf.len());
    let range = unsafe { decoder.buf.get_unchecked(offset..offset + inline_size) };
    range.iter().any(|byte| *byte != 0)
}

////////////////////////////////////////////////////////////////////////////////
// Envelopes
////////////////////////////////////////////////////////////////////////////////

#[doc(hidden)] // only exported for use in macros or generated code
#[inline]
pub unsafe fn encode_in_envelope<T: TypeMarker, D: ResourceDialect>(
    val: impl Encode<T, D>,
    encoder: &mut Encoder<'_, D>,
    offset: usize,
    mut depth: Depth,
) -> Result<()> {
    depth.increment()?;
    let bytes_before = encoder.buf.len();
    let handles_before = encoder.handles.len();
    let inline_size = T::inline_size(encoder.context);
    if inline_size <= 4 {
        // Zero out the 4 byte inlined region and set the flag at the same time.
        unsafe { encoder.write_num(1u64 << 48, offset) };
        unsafe { val.encode(encoder, offset, depth)? };
        let handles_written = (encoder.handles.len() - handles_before) as u16;
        unsafe { encoder.write_num(handles_written, offset + 4) };
    } else {
        let out_of_line_offset = unsafe { encoder.out_of_line_offset(inline_size) };
        unsafe { val.encode(encoder, out_of_line_offset, depth)? };
        let bytes_written = (encoder.buf.len() - bytes_before) as u32;
        let handles_written = (encoder.handles.len() - handles_before) as u32;
        debug_assert_eq!(bytes_written % 8, 0);
        unsafe { encoder.write_num(bytes_written, offset) };
        unsafe { encoder.write_num(handles_written, offset + 4) };
    }
    Ok(())
}

#[doc(hidden)] // only exported for use in macros or generated code
#[inline]
pub unsafe fn encode_in_envelope_optional<T: TypeMarker, D: ResourceDialect>(
    val: Option<impl Encode<T, D>>,
    encoder: &mut Encoder<'_, D>,
    offset: usize,
    depth: Depth,
) -> Result<()> {
    match val {
        None => unsafe { encoder.write_num(0u64, offset) },
        Some(val) => unsafe { encode_in_envelope(val, encoder, offset, depth)? },
    }
    Ok(())
}

/// Decodes and validates an envelope header. Returns `None` if absent and
/// `Some((inlined, num_bytes, num_handles))` if present.
#[doc(hidden)] // only exported for use in macros or generated code
#[inline(always)]
pub unsafe fn decode_envelope_header<D: ResourceDialect>(
    decoder: &mut Decoder<'_, D>,
    offset: usize,
) -> Result<Option<(bool, u32, u32)>> {
    let num_bytes = decoder.read_num::<u32>(offset);
    let num_handles = decoder.read_num::<u16>(offset + 4) as u32;
    let inlined = decoder.read_num::<u16>(offset + 6);
    match (num_bytes, num_handles, inlined) {
        (0, 0, 0) => Ok(None),
        (_, _, 1) => Ok(Some((true, 4, num_handles))),
        (_, _, 0) if num_bytes.is_multiple_of(8) => Ok(Some((false, num_bytes, num_handles))),
        (_, _, 0) => Err(Error::InvalidNumBytesInEnvelope),
        _ => Err(Error::InvalidInlineMarkerInEnvelope),
    }
}

/// Decodes a FIDL envelope and skips over any out-of-line bytes and handles.
#[doc(hidden)] // only exported for use in macros or generated code
#[inline]
pub unsafe fn decode_unknown_envelope<D: ResourceDialect>(
    decoder: &mut Decoder<'_, D>,
    offset: usize,
    mut depth: Depth,
) -> Result<()> {
    if let Some((inlined, num_bytes, num_handles)) =
        unsafe { decode_envelope_header(decoder, offset)? }
    {
        if !inlined {
            depth.increment()?;
            // Calling decoder.out_of_line_offset(0) is not allowed.
            if num_bytes != 0 {
                let _ = unsafe { decoder.out_of_line_offset(num_bytes as usize)? };
            }
        }
        if num_handles != 0 {
            for _ in 0..num_handles {
                decoder.drop_next_handle()?;
            }
        }
    }
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
// Unions
////////////////////////////////////////////////////////////////////////////////

/// Decodes the inline portion of a union.
/// Returns `(ordinal, inlined, num_bytes, num_handles)`.
#[doc(hidden)] // only exported for use in macros or generated code
#[inline]
pub unsafe fn decode_union_inline_portion<D: ResourceDialect>(
    decoder: &mut Decoder<'_, D>,
    offset: usize,
) -> Result<(u64, bool, u32, u32)> {
    let ordinal = decoder.read_num::<u64>(offset);
    match unsafe { decode_envelope_header(decoder, offset + 8)? } {
        Some((inlined, num_bytes, num_handles)) => Ok((ordinal, inlined, num_bytes, num_handles)),
        None => Err(Error::NotNullable),
    }
}

////////////////////////////////////////////////////////////////////////////////
// Result unions
////////////////////////////////////////////////////////////////////////////////

/// The FIDL union generated for strict two-way methods with errors.
pub struct ResultType<T: TypeMarker, E: TypeMarker>(PhantomData<(T, E)>);

/// The FIDL union generated for flexible two-way methods without errors.
pub struct FlexibleType<T: TypeMarker>(PhantomData<T>);

/// The FIDL union generated for flexible two-way methods with errors.
pub struct FlexibleResultType<T: TypeMarker, E: TypeMarker>(PhantomData<(T, E)>);

/// Owned type for `FlexibleType`.
#[doc(hidden)] // only exported for use in macros or generated code
#[derive(Debug)]
pub enum Flexible<T> {
    Ok(T),
    FrameworkErr(FrameworkErr),
}

/// Owned type for `FlexibleResultType`.
#[doc(hidden)] // only exported for use in macros or generated code
#[derive(Debug)]
pub enum FlexibleResult<T, E> {
    Ok(T),
    DomainErr(E),
    FrameworkErr(FrameworkErr),
}

/// Internal FIDL framework error type used to identify unknown methods.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(i32)]
pub enum FrameworkErr {
    /// Method was not recognized.
    UnknownMethod = zx_types::ZX_ERR_NOT_SUPPORTED,
}

impl FrameworkErr {
    #[inline]
    fn from_primitive(prim: i32) -> Option<Self> {
        match prim {
            zx_types::ZX_ERR_NOT_SUPPORTED => Some(Self::UnknownMethod),
            _ => None,
        }
    }

    #[inline(always)]
    const fn into_primitive(self) -> i32 {
        self as i32
    }
}

unsafe impl TypeMarker for FrameworkErr {
    type Owned = Self;
    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        std::mem::align_of::<i32>()
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        std::mem::size_of::<i32>()
    }

    #[inline(always)]
    fn encode_is_copy() -> bool {
        true
    }

    #[inline(always)]
    fn decode_is_copy() -> bool {
        false
    }
}

impl ValueTypeMarker for FrameworkErr {
    type Borrowed<'a> = Self;
    #[inline(always)]
    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        *value
    }
}

unsafe impl<D: ResourceDialect> Encode<Self, D> for FrameworkErr {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<Self>(offset);
        unsafe { encoder.write_num(self.into_primitive(), offset) };
        Ok(())
    }
}

impl<D: ResourceDialect> Decode<Self, D> for FrameworkErr {
    #[inline(always)]
    fn new_empty() -> Self {
        Self::UnknownMethod
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Self>(offset);
        let prim = decoder.read_num::<i32>(offset);
        *self = Self::from_primitive(prim).ok_or(Error::InvalidEnumValue)?;
        Ok(())
    }
}

impl<T> Flexible<T> {
    /// Creates a new instance from the underlying value.
    pub fn new(value: T) -> Self {
        Self::Ok(value)
    }

    /// Converts to a `fidl::Result`, mapping framework errors to `fidl::Error`.
    pub fn into_result<P: ProtocolMarker>(self, method_name: &'static str) -> Result<T> {
        match self {
            Flexible::Ok(ok) => Ok(ok),
            Flexible::FrameworkErr(FrameworkErr::UnknownMethod) => {
                Err(Error::UnsupportedMethod { method_name, protocol_name: P::DEBUG_NAME })
            }
        }
    }
}

impl<T, E> FlexibleResult<T, E> {
    /// Creates a new instance from an `std::result::Result`.
    pub fn new(result: std::result::Result<T, E>) -> Self {
        match result {
            Ok(value) => Self::Ok(value),
            Err(err) => Self::DomainErr(err),
        }
    }

    /// Converts to a `fidl::Result`, mapping framework errors to `fidl::Error`.
    pub fn into_result<P: ProtocolMarker>(
        self,
        method_name: &'static str,
    ) -> Result<std::result::Result<T, E>> {
        match self {
            FlexibleResult::Ok(ok) => Ok(Ok(ok)),
            FlexibleResult::DomainErr(err) => Ok(Err(err)),
            FlexibleResult::FrameworkErr(FrameworkErr::UnknownMethod) => {
                Err(Error::UnsupportedMethod { method_name, protocol_name: P::DEBUG_NAME })
            }
        }
    }
}

/// Implements `TypeMarker`, `Encode`, and `Decode` for a result union type.
macro_rules! impl_result_union {
    (
        params: [$($encode_param:ident: Encode<$type_param:ident>),*],
        ty: $ty:ty,
        owned: $owned:ty,
        encode: $encode:ty,
        members: [$(
            {
                ctor: { $($member_ctor:tt)* },
                ty: $member_ty:ty,
                ordinal: $member_ordinal:tt,
            },
        )*]
    ) => {
        unsafe impl<$($type_param: TypeMarker),*> TypeMarker for $ty {
            type Owned = $owned;

            #[inline(always)]
            fn inline_align(_context: Context) -> usize {
                8
            }

            #[inline(always)]
            fn inline_size(_context: Context) -> usize {
                16
            }
        }

        unsafe impl<D: ResourceDialect, $($type_param: TypeMarker, $encode_param: Encode<$type_param, D>),*> Encode<$ty, D> for $encode {
            #[inline]
            unsafe fn encode(self, encoder: &mut Encoder<'_, D>, offset: usize, depth: Depth) -> Result<()> {
                encoder.debug_check_bounds::<$ty>(offset);
                match self {
                    $(
                        $($member_ctor)*(val) => {
                            unsafe { encoder.write_num::<u64>($member_ordinal, offset) };
                            unsafe { encode_in_envelope::<$member_ty, D>(val, encoder, offset + 8, depth) }
                        }
                    )*
                }
            }
        }

        impl<D: ResourceDialect, $($type_param: TypeMarker),*> Decode<$ty, D> for $owned
        where $($type_param::Owned: Decode<$type_param, D>),*
        {
            #[inline(always)]
            fn new_empty() -> Self {
                #![allow(unreachable_code)]
                $(
                    return $($member_ctor)*(new_empty!($member_ty, D));
                )*
            }

            #[inline]
            unsafe fn decode(&mut self, decoder: &mut Decoder<'_, D>, offset: usize, mut depth: Depth) -> Result<()> {
                decoder.debug_check_bounds::<$ty>(offset);
                let next_out_of_line = decoder.next_out_of_line();
                let handles_before = decoder.remaining_handles();
                let (ordinal, inlined, num_bytes, num_handles) = unsafe { decode_union_inline_portion(decoder, offset)? };
                let member_inline_size = match ordinal {
                    $(
                        $member_ordinal => <$member_ty as TypeMarker>::inline_size(decoder.context),
                    )*
                    _ => return Err(Error::UnknownUnionTag),
                };
                if inlined != (member_inline_size <= 4) {
                    return Err(Error::InvalidInlineBitInEnvelope);
                }
                let inner_offset;
                if inlined {
                    decoder.check_inline_envelope_padding(offset + 8, member_inline_size)?;
                    inner_offset = offset + 8;
                } else {
                    depth.increment()?;
                    inner_offset = unsafe { decoder.out_of_line_offset(member_inline_size)? };
                }
                match ordinal {
                    $(
                        $member_ordinal => {
                            #[allow(irrefutable_let_patterns)]
                            if let $($member_ctor)*(_) = self {
                                // Do nothing, read the value into the object
                            } else {
                                // Initialize `self` to the right variant
                                *self = $($member_ctor)*(new_empty!($member_ty, D));
                            }
                            #[allow(irrefutable_let_patterns)]
                            if let $($member_ctor)*(val) = self {
                                unsafe { decode!($member_ty, D, val, decoder, inner_offset, depth)? };
                            } else {
                                unreachable!()
                            }
                        }
                    )*
                    ordinal => panic!("unexpected ordinal {:?}", ordinal)
                }
                if !inlined && decoder.next_out_of_line() != next_out_of_line + (num_bytes as usize) {
                    return Err(Error::InvalidNumBytesInEnvelope);
                }
                if handles_before != decoder.remaining_handles() + (num_handles as usize) {
                    return Err(Error::InvalidNumHandlesInEnvelope);
                }
                Ok(())
            }
        }
    };
}

impl_result_union! {
    params: [X: Encode<T>, Y: Encode<E>],
    ty: ResultType<T, E>,
    owned: std::result::Result<T::Owned, E::Owned>,
    encode: std::result::Result<X, Y>,
    members: [
        { ctor: { Ok }, ty: T, ordinal: 1, },
        { ctor: { Err }, ty: E, ordinal: 2, },
    ]
}

impl_result_union! {
    params: [X: Encode<T>],
    ty: FlexibleType<T>,
    owned: Flexible<T::Owned>,
    encode: Flexible<X>,
    members: [
        { ctor: { Flexible::Ok }, ty: T, ordinal: 1, },
        { ctor: { Flexible::FrameworkErr }, ty: FrameworkErr, ordinal: 3, },
    ]
}

impl_result_union! {
    params: [X: Encode<T>, Y: Encode<E>],
    ty: FlexibleResultType<T, E>,
    owned: FlexibleResult<T::Owned, E::Owned>,
    encode: FlexibleResult<X, Y>,
    members: [
        { ctor: { FlexibleResult::Ok }, ty: T, ordinal: 1, },
        { ctor: { FlexibleResult::DomainErr }, ty: E, ordinal: 2, },
        { ctor: { FlexibleResult::FrameworkErr }, ty: FrameworkErr, ordinal: 3, },
    ]
}

////////////////////////////////////////////////////////////////////////////////
// Epitaphs
////////////////////////////////////////////////////////////////////////////////

/// The body of a FIDL Epitaph
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct EpitaphBody {
    /// The error status.
    pub error: Result<(), zx_status::Status>,
}

unsafe impl TypeMarker for EpitaphBody {
    type Owned = Self;

    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        4
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        4
    }
}

impl ValueTypeMarker for EpitaphBody {
    type Borrowed<'a> = &'a Self;

    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        value
    }
}

unsafe impl<D: ResourceDialect> Encode<EpitaphBody, D> for &EpitaphBody {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<EpitaphBody>(offset);
        unsafe { encoder.write_num::<i32>(zx_status::Status::result_into_raw(self.error), offset) };
        Ok(())
    }
}

impl<D: ResourceDialect> Decode<Self, D> for EpitaphBody {
    #[inline(always)]
    fn new_empty() -> Self {
        Self { error: Ok(()) }
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Self>(offset);
        self.error = zx_status::Status::ok(decoder.read_num::<i32>(offset));
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
// Zircon types
////////////////////////////////////////////////////////////////////////////////

unsafe impl TypeMarker for ObjectType {
    type Owned = Self;

    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        mem::align_of::<Self>()
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        mem::size_of::<Self>()
    }
}

impl ValueTypeMarker for ObjectType {
    type Borrowed<'a> = Self;

    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        *value
    }
}

unsafe impl<D: ResourceDialect> Encode<ObjectType, D> for ObjectType {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<Self>(offset);
        unsafe { encoder.write_num(self.into_raw(), offset) };
        Ok(())
    }
}

impl<D: ResourceDialect> Decode<Self, D> for ObjectType {
    #[inline(always)]
    fn new_empty() -> Self {
        ObjectType::NONE
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Self>(offset);
        *self = Self::from_raw(decoder.read_num(offset));
        Ok(())
    }
}

unsafe impl TypeMarker for Rights {
    type Owned = Self;

    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        mem::align_of::<Self>()
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        mem::size_of::<Self>()
    }
}

impl ValueTypeMarker for Rights {
    type Borrowed<'a> = Self;

    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        *value
    }
}

unsafe impl<D: ResourceDialect> Encode<Rights, D> for Rights {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<Self>(offset);
        if self.bits() & Self::all().bits() != self.bits() {
            return Err(Error::InvalidBitsValue);
        }
        unsafe { encoder.write_num(self.bits(), offset) };
        Ok(())
    }
}

impl<D: ResourceDialect> Decode<Self, D> for Rights {
    #[inline(always)]
    fn new_empty() -> Self {
        Rights::empty()
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Self>(offset);
        *self = Self::from_bits(decoder.read_num(offset)).ok_or(Error::InvalidBitsValue)?;
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
// Messages
////////////////////////////////////////////////////////////////////////////////

/// The FIDL type for a message consisting of a header `H` and body `T`.
pub struct GenericMessageType<H: ValueTypeMarker, T: TypeMarker>(PhantomData<(H, T)>);

/// A struct which encodes as `GenericMessageType<H, T>` where `E: Encode<T>`.
pub struct GenericMessage<H, E> {
    /// Header of the message.
    pub header: H,
    /// Body of the message.
    pub body: E,
}

/// The owned type for `GenericMessageType`.
///
/// Uninhabited because we never decode full messages. We decode the header and
/// body separately, as we usually we don't know the body's type until after
/// we've decoded the header.
pub enum GenericMessageOwned {}

unsafe impl<H: ValueTypeMarker, T: TypeMarker> TypeMarker for GenericMessageType<H, T> {
    type Owned = GenericMessageOwned;

    #[inline(always)]
    fn inline_align(context: Context) -> usize {
        std::cmp::max(H::inline_align(context), T::inline_align(context))
    }

    #[inline(always)]
    fn inline_size(context: Context) -> usize {
        H::inline_size(context) + T::inline_size(context)
    }
}

unsafe impl<H: ValueTypeMarker, T: TypeMarker, E: Encode<T, D>, D: ResourceDialect>
    Encode<GenericMessageType<H, T>, D> for GenericMessage<<H as TypeMarker>::Owned, E>
where
    for<'a> H::Borrowed<'a>: Encode<H, D>,
{
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<GenericMessageType<H, T>>(offset);
        unsafe {
            H::borrow(&self.header).encode(encoder, offset, depth)?;
            self.body.encode(encoder, offset + H::inline_size(encoder.context), depth)
        }
    }
}

impl<H: ValueTypeMarker, T: TypeMarker, D: ResourceDialect> Decode<GenericMessageType<H, T>, D>
    for GenericMessageOwned
{
    fn new_empty() -> Self {
        panic!("cannot create GenericMessageOwned");
    }

    unsafe fn decode(
        &mut self,
        _decoder: &mut Decoder<'_, D>,
        _offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        match *self {}
    }
}

////////////////////////////////////////////////////////////////////////////////
// Transaction messages
////////////////////////////////////////////////////////////////////////////////

/// The FIDL type for a transaction message with body `T`.
pub type TransactionMessageType<T> = GenericMessageType<TransactionHeader, T>;

/// A struct which encodes as `TransactionMessageType<T>` where `E: Encode<T>`.
pub type TransactionMessage<E> = GenericMessage<TransactionHeader, E>;

/// Header for transactional FIDL messages
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct TransactionHeader {
    /// Transaction ID which identifies a request-response pair
    pub tx_id: u32,
    /// Flags set for this message. MUST NOT be validated by bindings. Usually
    /// temporarily during migrations.
    pub at_rest_flags: [u8; 2],
    /// Flags used for dynamically interpreting the request if it is unknown to
    /// the receiver.
    pub dynamic_flags: u8,
    /// Magic number indicating the message's wire format. Two sides with
    /// different magic numbers are incompatible with each other.
    pub magic_number: u8,
    /// Ordinal which identifies the FIDL method
    pub ordinal: u64,
}

impl TransactionHeader {
    /// Returns whether the message containing this TransactionHeader is in a
    /// compatible wire format
    #[inline]
    pub fn is_compatible(&self) -> bool {
        self.magic_number == MAGIC_NUMBER_INITIAL
    }
}

bitflags! {
    /// Bitflags type for transaction header at-rest flags.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct AtRestFlags: u16 {
        /// Indicates that the V2 wire format should be used instead of the V1
        /// wire format.
        /// This includes the following RFCs:
        /// - Efficient envelopes
        /// - Inlining small values in FIDL envelopes
        const USE_V2_WIRE_FORMAT = 2;
    }
}

bitflags! {
    /// Bitflags type to flags that aid in dynamically identifying features of
    /// the request.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct DynamicFlags: u8 {
        /// Indicates that the request is for a flexible method.
        const FLEXIBLE = 1 << 7;
    }
}

impl From<AtRestFlags> for [u8; 2] {
    #[inline]
    fn from(value: AtRestFlags) -> Self {
        value.bits().to_le_bytes()
    }
}

impl TransactionHeader {
    /// Creates a new transaction header with the default encode context and magic number.
    #[inline]
    pub fn new(tx_id: u32, ordinal: u64, dynamic_flags: DynamicFlags) -> Self {
        TransactionHeader::new_full(
            tx_id,
            ordinal,
            default_encode_context(),
            dynamic_flags,
            MAGIC_NUMBER_INITIAL,
        )
    }

    /// Creates a new transaction header with a specific context and magic number.
    #[inline]
    pub fn new_full(
        tx_id: u32,
        ordinal: u64,
        context: Context,
        dynamic_flags: DynamicFlags,
        magic_number: u8,
    ) -> Self {
        TransactionHeader {
            tx_id,
            at_rest_flags: context.at_rest_flags().into(),
            dynamic_flags: dynamic_flags.bits(),
            magic_number,
            ordinal,
        }
    }

    /// Returns true if the header is for an epitaph message.
    #[inline]
    pub fn is_epitaph(&self) -> bool {
        self.ordinal == EPITAPH_ORDINAL
    }

    /// Returns an error if this header has an incompatible wire format.
    #[inline]
    pub fn validate_wire_format(&self) -> Result<()> {
        if self.magic_number != MAGIC_NUMBER_INITIAL {
            return Err(Error::IncompatibleMagicNumber(self.magic_number));
        }
        if !self.at_rest_flags().contains(AtRestFlags::USE_V2_WIRE_FORMAT) {
            return Err(Error::UnsupportedWireFormatVersion);
        }
        Ok(())
    }

    /// Returns an error if this request header has an incorrect transaction id
    /// for the given method type.
    #[inline]
    pub fn validate_request_tx_id(&self, method_type: MethodType) -> Result<()> {
        match method_type {
            MethodType::OneWay if self.tx_id != 0 => Err(Error::InvalidRequestTxid),
            MethodType::TwoWay if self.tx_id == 0 => Err(Error::InvalidRequestTxid),
            _ => Ok(()),
        }
    }

    /// Returns the header's migration flags as a `AtRestFlags` value.
    #[inline]
    pub fn at_rest_flags(&self) -> AtRestFlags {
        AtRestFlags::from_bits_truncate(u16::from_le_bytes(self.at_rest_flags))
    }

    /// Returns the header's dynamic flags as a `DynamicFlags` value.
    #[inline]
    pub fn dynamic_flags(&self) -> DynamicFlags {
        DynamicFlags::from_bits_truncate(self.dynamic_flags)
    }

    /// Returns the context to use for decoding the message body associated with
    /// this header. During migrations, this is dependent on `self.flags()` and
    /// controls dynamic behavior in the read path.
    #[inline]
    pub fn decoding_context(&self) -> Context {
        Context { wire_format_version: WireFormatVersion::V2 }
    }
}

/// Decodes the transaction header from a message.
/// Returns the header and a reference to the tail of the message.
pub fn decode_transaction_header(bytes: &[u8]) -> Result<(TransactionHeader, &[u8])> {
    let mut header = new_empty!(TransactionHeader, NoHandleResourceDialect);
    let context = Context { wire_format_version: WireFormatVersion::V2 };
    let header_len = <TransactionHeader as TypeMarker>::inline_size(context);
    if bytes.len() < header_len {
        return Err(Error::OutOfRange { expected: header_len, actual: bytes.len() });
    }
    let (header_bytes, body_bytes) = bytes.split_at(header_len);
    Decoder::<NoHandleResourceDialect>::decode_with_context::<TransactionHeader>(
        context,
        header_bytes,
        &mut [],
        &mut header,
    )
    .map_err(|_| Error::InvalidHeader)?;
    header.validate_wire_format()?;
    Ok((header, body_bytes))
}

unsafe impl TypeMarker for TransactionHeader {
    type Owned = Self;

    #[inline(always)]
    fn inline_align(_context: Context) -> usize {
        8
    }

    #[inline(always)]
    fn inline_size(_context: Context) -> usize {
        16
    }
}

impl ValueTypeMarker for TransactionHeader {
    type Borrowed<'a> = &'a Self;

    fn borrow(value: &Self::Owned) -> Self::Borrowed<'_> {
        value
    }
}

unsafe impl<D: ResourceDialect> Encode<TransactionHeader, D> for &TransactionHeader {
    #[inline]
    unsafe fn encode(
        self,
        encoder: &mut Encoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        encoder.debug_check_bounds::<TransactionHeader>(offset);
        unsafe {
            let buf_ptr = encoder.buf.as_mut_ptr().add(offset);
            (buf_ptr as *mut TransactionHeader).write_unaligned(*self);
        }
        Ok(())
    }
}

impl<D: ResourceDialect> Decode<Self, D> for TransactionHeader {
    #[inline(always)]
    fn new_empty() -> Self {
        Self { tx_id: 0, at_rest_flags: [0; 2], dynamic_flags: 0, magic_number: 0, ordinal: 0 }
    }

    #[inline]
    unsafe fn decode(
        &mut self,
        decoder: &mut Decoder<'_, D>,
        offset: usize,
        _depth: Depth,
    ) -> Result<()> {
        decoder.debug_check_bounds::<Self>(offset);
        unsafe {
            let buf_ptr = decoder.buf.as_ptr().add(offset);
            let obj_ptr = self as *mut TransactionHeader;
            std::ptr::copy_nonoverlapping(buf_ptr, obj_ptr as *mut u8, 16);
        }
        Ok(())
    }
}

////////////////////////////////////////////////////////////////////////////////
// TLS buffer
////////////////////////////////////////////////////////////////////////////////

/// Thread-local buffer for encoding and decoding FIDL transactions. Needed to
/// implement `ResourceDialect`.
pub struct TlsBuf<D: ResourceDialect> {
    bytes: Vec<u8>,
    encode_handles: Vec<<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition>,
    decode_handles: Vec<<D::Handle as HandleFor<D>>::HandleInfo>,
}

impl<D: ResourceDialect> Default for TlsBuf<D> {
    /// Create a new `TlsBuf`
    fn default() -> TlsBuf<D> {
        TlsBuf {
            bytes: Vec::with_capacity(MIN_BUF_BYTES_SIZE),
            encode_handles: Vec::new(),
            decode_handles: Vec::new(),
        }
    }
}

#[inline]
fn with_tls_buf<D: ResourceDialect, R>(f: impl FnOnce(&mut TlsBuf<D>) -> R) -> R {
    D::with_tls_buf(f)
}

pub(crate) const MIN_BUF_BYTES_SIZE: usize = 512;

/// Acquire a mutable reference to the thread-local buffers used for encoding.
///
/// This function may not be called recursively.
#[inline]
pub fn with_tls_encode_buf<R, D: ResourceDialect>(
    f: impl FnOnce(
        &mut Vec<u8>,
        &mut Vec<<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition>,
    ) -> R,
) -> R {
    with_tls_buf::<D, R>(|buf| {
        let res = f(&mut buf.bytes, &mut buf.encode_handles);
        buf.bytes.clear();
        buf.encode_handles.clear();
        res
    })
}

/// Acquire a mutable reference to the thread-local buffers used for decoding.
///
/// This function may not be called recursively.
#[inline]
pub fn with_tls_decode_buf<R, D: ResourceDialect>(
    f: impl FnOnce(&mut Vec<u8>, &mut Vec<<D::Handle as HandleFor<D>>::HandleInfo>) -> R,
) -> R {
    with_tls_buf::<D, R>(|buf| {
        let res = f(&mut buf.bytes, &mut buf.decode_handles);
        buf.bytes.clear();
        buf.decode_handles.clear();
        res
    })
}

/// Clear the thread local buffers used for encoding and decoding.
#[inline]
pub fn clear_tls_buf<D: ResourceDialect>() {
    with_tls_buf::<D, ()>(|buf| {
        buf.bytes.clear();
        buf.bytes.shrink_to_fit();
        buf.encode_handles.clear();
        buf.encode_handles.shrink_to_fit();
        buf.decode_handles.clear();
        buf.decode_handles.shrink_to_fit();
    });
}

/// Encodes the provided type into the thread-local encoding buffers.
///
/// This function may not be called recursively.
#[inline]
pub fn with_tls_encoded<T: TypeMarker, D: ResourceDialect, Out>(
    val: impl Encode<T, D>,
    f: impl FnOnce(
        &mut Vec<u8>,
        &mut Vec<<D::ProxyChannel as ProxyChannelFor<D>>::HandleDisposition>,
    ) -> Result<Out>,
) -> Result<Out> {
    with_tls_encode_buf::<Result<Out>, D>(|bytes, handles| {
        Encoder::<D>::encode(bytes, handles, val)?;
        f(bytes, handles)
    })
}

////////////////////////////////////////////////////////////////////////////////
// Tests
////////////////////////////////////////////////////////////////////////////////

#[cfg(test)]
mod test {
    use super::*;
    use crate::handle::convert_handle_dispositions_to_infos;
    use crate::time::{BootInstant, BootTicks, MonotonicInstant, MonotonicTicks};
    use assert_matches::assert_matches;
    use std::fmt;

    const CONTEXTS: [Context; 1] = [Context { wire_format_version: WireFormatVersion::V2 }];

    const OBJECT_TYPE_NONE: u32 = crate::handle::ObjectType::NONE.into_raw();
    const SAME_RIGHTS: u32 = crate::handle::Rights::SAME_RIGHTS.bits();

    #[track_caller]
    fn to_infos(dispositions: &mut Vec<HandleDisposition<'_>>) -> Vec<HandleInfo> {
        convert_handle_dispositions_to_infos(mem::take(dispositions)).unwrap()
    }

    #[track_caller]
    pub fn encode_decode<T: TypeMarker>(
        ctx: Context,
        start: impl Encode<T, DefaultFuchsiaResourceDialect>,
    ) -> T::Owned
    where
        T::Owned: Decode<T, DefaultFuchsiaResourceDialect>,
    {
        let buf = &mut Vec::new();
        let handle_buf = &mut Vec::new();
        Encoder::encode_with_context::<T>(ctx, buf, handle_buf, start).expect("Encoding failed");
        let mut out = T::Owned::new_empty();
        Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<T>(
            ctx,
            buf,
            &mut to_infos(handle_buf),
            &mut out,
        )
        .expect("Decoding failed");
        out
    }

    #[track_caller]
    fn encode_assert_bytes<T: TypeMarker>(
        ctx: Context,
        data: impl Encode<T, DefaultFuchsiaResourceDialect>,
        encoded_bytes: &[u8],
    ) {
        let buf = &mut Vec::new();
        let handle_buf = &mut Vec::new();
        Encoder::encode_with_context::<T>(ctx, buf, handle_buf, data).expect("Encoding failed");
        assert_eq!(buf, encoded_bytes);
    }

    #[track_caller]
    fn identity<T>(data: &T::Owned)
    where
        T: ValueTypeMarker,
        T::Owned: fmt::Debug + PartialEq + Decode<T, DefaultFuchsiaResourceDialect>,
        for<'a> T::Borrowed<'a>: Encode<T, DefaultFuchsiaResourceDialect>,
    {
        for ctx in CONTEXTS {
            assert_eq!(*data, encode_decode(ctx, T::borrow(data)));
        }
    }

    #[track_caller]
    fn identities<T>(values: &[T::Owned])
    where
        T: ValueTypeMarker,
        T::Owned: fmt::Debug + PartialEq + Decode<T, DefaultFuchsiaResourceDialect>,
        for<'a> T::Borrowed<'a>: Encode<T, DefaultFuchsiaResourceDialect>,
    {
        for value in values {
            identity::<T>(value);
        }
    }

    #[test]
    fn encode_decode_byte() {
        identities::<u8>(&[0u8, 57u8, 255u8]);
        identities::<i8>(&[0i8, -57i8, 12i8]);
        identity::<Optional<Vector<i32, 3>>>(&None::<Vec<i32>>);
    }

    #[test]
    fn encode_decode_multibyte() {
        identities::<u64>(&[0u64, 1u64, u64::MAX, u64::MIN]);
        identities::<i64>(&[0i64, 1i64, i64::MAX, i64::MIN]);
        identities::<f32>(&[0f32, 1f32, f32::MAX, f32::MIN]);
        identities::<f64>(&[0f64, 1f64, f64::MAX, f64::MIN]);
    }

    #[test]
    fn encode_decode_nan() {
        for ctx in CONTEXTS {
            assert!(encode_decode::<f32>(ctx, f32::NAN).is_nan());
            assert!(encode_decode::<f64>(ctx, f64::NAN).is_nan());
        }
    }

    #[test]
    fn encode_decode_instants() {
        let monotonic = MonotonicInstant::from_nanos(987654321);
        let boot = BootInstant::from_nanos(987654321);
        let monotonic_ticks = MonotonicTicks::from_raw(111111111);
        let boot_ticks = BootTicks::from_raw(22222222);
        for ctx in CONTEXTS {
            assert_eq!(encode_decode::<BootInstant>(ctx, boot), boot);
            assert_eq!(encode_decode::<MonotonicInstant>(ctx, monotonic), monotonic);
            assert_eq!(encode_decode::<BootTicks>(ctx, boot_ticks), boot_ticks);
            assert_eq!(encode_decode::<MonotonicTicks>(ctx, monotonic_ticks), monotonic_ticks);
        }
    }

    #[test]
    fn encode_decode_out_of_line() {
        type V<T> = UnboundedVector<T>;
        type S = UnboundedString;
        type O<T> = Optional<T>;

        identity::<V<i32>>(&Vec::<i32>::new());
        identity::<V<i32>>(&vec![1, 2, 3]);
        identity::<O<V<i32>>>(&None::<Vec<i32>>);
        identity::<O<V<i32>>>(&Some(Vec::<i32>::new()));
        identity::<O<V<i32>>>(&Some(vec![1, 2, 3]));
        identity::<O<V<V<i32>>>>(&Some(vec![vec![1, 2, 3]]));
        identity::<O<V<O<V<i32>>>>>(&Some(vec![Some(vec![1, 2, 3])]));
        identity::<S>(&"".to_string());
        identity::<S>(&"foo".to_string());
        identity::<O<S>>(&None::<String>);
        identity::<O<S>>(&Some("".to_string()));
        identity::<O<S>>(&Some("foo".to_string()));
        identity::<O<V<O<S>>>>(&Some(vec![None, Some("foo".to_string())]));
        identity::<V<S>>(&vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn array_of_arrays() {
        identity::<Array<Array<u32, 5>, 2>>(&[[1, 2, 3, 4, 5], [5, 4, 3, 2, 1]]);
    }

    fn slice_identity<T>(start: &[T::Owned])
    where
        T: ValueTypeMarker,
        T::Owned: fmt::Debug + PartialEq + Decode<T, DefaultFuchsiaResourceDialect>,
        for<'a> T::Borrowed<'a>: Encode<T, DefaultFuchsiaResourceDialect>,
    {
        for ctx in CONTEXTS {
            let decoded = encode_decode::<UnboundedVector<T>>(ctx, start);
            assert_eq!(start, UnboundedVector::<T>::borrow(&decoded));
        }
    }

    #[test]
    fn encode_slices_of_primitives() {
        slice_identity::<u8>(&[]);
        slice_identity::<u8>(&[0]);
        slice_identity::<u8>(&[1, 2, 3, 4, 5, 255]);

        slice_identity::<i8>(&[]);
        slice_identity::<i8>(&[0]);
        slice_identity::<i8>(&[1, 2, 3, 4, 5, -128, 127]);

        slice_identity::<u64>(&[]);
        slice_identity::<u64>(&[0]);
        slice_identity::<u64>(&[1, 2, 3, 4, 5, u64::MAX]);

        slice_identity::<f32>(&[]);
        slice_identity::<f32>(&[0.0]);
        slice_identity::<f32>(&[1.0, 2.0, 3.0, 4.0, 5.0, f32::MIN, f32::MAX]);

        slice_identity::<f64>(&[]);
        slice_identity::<f64>(&[0.0]);
        slice_identity::<f64>(&[1.0, 2.0, 3.0, 4.0, 5.0, f64::MIN, f64::MAX]);
    }

    #[test]
    fn result_encode_empty_ok_value() {
        for ctx in CONTEXTS {
            // An empty response is represented by () and has zero size.
            encode_assert_bytes::<EmptyPayload>(ctx, (), &[]);
        }
        // But in the context of an error result type Result<(), ErrorType>, the
        // () in Ok(()) represents an empty struct (with size 1).
        encode_assert_bytes::<ResultType<EmptyStruct, i32>>(
            Context { wire_format_version: WireFormatVersion::V2 },
            Ok::<(), i32>(()),
            &[
                0x01, 0x00, 0x00, 0x00, // success ordinal
                0x00, 0x00, 0x00, 0x00, // success ordinal [cont.]
                0x00, 0x00, 0x00, 0x00, // inline value: empty struct + 3 bytes padding
                0x00, 0x00, 0x01, 0x00, // 0 handles, flags (inlined)
            ],
        );
    }

    #[test]
    fn result_decode_empty_ok_value() {
        let mut result = Err(0);
        Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<ResultType<EmptyStruct, u32>>(
            Context { wire_format_version: WireFormatVersion::V2 },
            &[
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // success ordinal
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, // empty struct inline
            ],
            &mut [],
            &mut result,
        )
        .expect("Decoding failed");
        assert_matches!(result, Ok(()));
    }

    #[test]
    fn encode_decode_result() {
        type Res = ResultType<UnboundedString, u32>;
        for ctx in CONTEXTS {
            assert_eq!(encode_decode::<Res>(ctx, Ok::<&str, u32>("foo")), Ok("foo".to_string()));
            assert_eq!(encode_decode::<Res>(ctx, Err::<&str, u32>(5)), Err(5));
        }
    }

    #[test]
    fn result_validates_num_bytes() {
        type Res = ResultType<u64, u64>;
        for ctx in CONTEXTS {
            for ordinal in [1, 2] {
                // Envelope should have num_bytes set to 8, not 16.
                let bytes = [
                    ordinal, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // ordinal
                    0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // 16 bytes, 0 handles
                    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // present
                    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, // number
                ];
                let mut out = new_empty!(Res, DefaultFuchsiaResourceDialect);
                assert_matches!(
                    Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<Res>(
                        ctx,
                        &bytes,
                        &mut [],
                        &mut out
                    ),
                    Err(Error::InvalidNumBytesInEnvelope)
                );
            }
        }
    }

    #[test]
    fn result_validates_num_handles() {
        type Res = ResultType<u64, u64>;
        for ctx in CONTEXTS {
            for ordinal in [1, 2] {
                // Envelope should have num_handles set to 0, not 1.
                let bytes = [
                    ordinal, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // ordinal
                    0x08, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, // 16 bytes, 1 handle
                    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, // present
                    0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, // number
                ];
                let mut out = new_empty!(Res, DefaultFuchsiaResourceDialect);
                assert_matches!(
                    Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<Res>(
                        ctx,
                        &bytes,
                        &mut [],
                        &mut out
                    ),
                    Err(Error::InvalidNumHandlesInEnvelope)
                );
            }
        }
    }

    #[test]
    fn decode_result_unknown_tag() {
        type Res = ResultType<u32, u32>;
        let ctx = Context { wire_format_version: WireFormatVersion::V2 };

        let bytes: &[u8] = &[
            // Ordinal 3 (not known to result) ----------|
            0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // inline value -----|  NHandles |  Flags ---|
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        ];
        let handle_buf = &mut Vec::<HandleInfo>::new();

        let mut out = new_empty!(Res, DefaultFuchsiaResourceDialect);
        let res = Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<Res>(
            ctx, bytes, handle_buf, &mut out,
        );
        assert_matches!(res, Err(Error::UnknownUnionTag));
    }

    #[test]
    fn decode_result_success_invalid_empty_struct() {
        type Res = ResultType<EmptyStruct, u32>;
        let ctx = Context { wire_format_version: WireFormatVersion::V2 };

        let bytes: &[u8] = &[
            // Ordinal 1 (success) ----------------------|
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            // inline value -----|  NHandles |  Flags ---|
            0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00,
        ];
        let handle_buf = &mut Vec::<HandleInfo>::new();

        let mut out = new_empty!(Res, DefaultFuchsiaResourceDialect);
        let res = Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<Res>(
            ctx, bytes, handle_buf, &mut out,
        );
        assert_matches!(res, Err(Error::Invalid));
    }

    #[test]
    fn encode_decode_transaction_msg() {
        for ctx in CONTEXTS {
            let header = TransactionHeader {
                tx_id: 4,
                ordinal: 6,
                at_rest_flags: [2, 0],
                dynamic_flags: 0,
                magic_number: 1,
            };
            type Body = UnboundedString;
            let body = "hello";

            let start = TransactionMessage { header, body };

            let buf = &mut Vec::new();
            let handle_buf = &mut Vec::new();
            Encoder::<DefaultFuchsiaResourceDialect>::encode_with_context::<
                TransactionMessageType<Body>,
            >(ctx, buf, handle_buf, start)
            .expect("Encoding failed");

            let (out_header, out_buf) =
                decode_transaction_header(buf).expect("Decoding header failed");
            assert_eq!(header, out_header);

            let mut body_out = String::new();
            Decoder::<DefaultFuchsiaResourceDialect>::decode_into::<Body>(
                &header,
                out_buf,
                &mut to_infos(handle_buf),
                &mut body_out,
            )
            .expect("Decoding body failed");
            assert_eq!(body, body_out);
        }
    }

    #[test]
    fn direct_encode_transaction_header_strict() {
        let bytes = &[
            0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, //
            0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        ];
        let header = TransactionHeader {
            tx_id: 4,
            ordinal: 6,
            at_rest_flags: [0; 2],
            dynamic_flags: DynamicFlags::empty().bits(),
            magic_number: 1,
        };

        for ctx in CONTEXTS {
            encode_assert_bytes::<TransactionHeader>(ctx, &header, bytes);
        }
    }

    #[test]
    fn direct_decode_transaction_header_strict() {
        let bytes = &[
            0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, //
            0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        ];
        let header = TransactionHeader {
            tx_id: 4,
            ordinal: 6,
            at_rest_flags: [0; 2],
            dynamic_flags: DynamicFlags::empty().bits(),
            magic_number: 1,
        };

        for ctx in CONTEXTS {
            let mut out = new_empty!(TransactionHeader, DefaultFuchsiaResourceDialect);
            Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<TransactionHeader>(
                ctx,
                bytes,
                &mut [],
                &mut out,
            )
            .expect("Decoding failed");
            assert_eq!(out, header);
        }
    }

    #[test]
    fn direct_encode_transaction_header_flexible() {
        let bytes = &[
            0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, //
            0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        ];
        let header = TransactionHeader {
            tx_id: 4,
            ordinal: 6,
            at_rest_flags: [0; 2],
            dynamic_flags: DynamicFlags::FLEXIBLE.bits(),
            magic_number: 1,
        };

        for ctx in CONTEXTS {
            encode_assert_bytes::<TransactionHeader>(ctx, &header, bytes);
        }
    }

    #[test]
    fn direct_decode_transaction_header_flexible() {
        let bytes = &[
            0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, //
            0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        ];
        let header = TransactionHeader {
            tx_id: 4,
            ordinal: 6,
            at_rest_flags: [0; 2],
            dynamic_flags: DynamicFlags::FLEXIBLE.bits(),
            magic_number: 1,
        };

        for ctx in CONTEXTS {
            let mut out = new_empty!(TransactionHeader, DefaultFuchsiaResourceDialect);
            Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<TransactionHeader>(
                ctx,
                bytes,
                &mut [],
                &mut out,
            )
            .expect("Decoding failed");
            assert_eq!(out, header);
        }
    }

    #[test]
    fn extra_data_is_disallowed() {
        for ctx in CONTEXTS {
            assert_matches!(
                Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<EmptyPayload>(
                    ctx,
                    &[0],
                    &mut [],
                    &mut ()
                ),
                Err(Error::ExtraBytes)
            );
            assert_matches!(
                Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<EmptyPayload>(
                    ctx,
                    &[],
                    &mut [HandleInfo::new(
                        NullableHandle::invalid(),
                        ObjectType::NONE,
                        Rights::NONE,
                    )],
                    &mut ()
                ),
                Err(Error::ExtraHandles)
            );
        }
    }

    #[test]
    fn encode_default_context() {
        let buf = &mut Vec::new();
        Encoder::<DefaultFuchsiaResourceDialect>::encode::<u8>(buf, &mut Vec::new(), 1u8)
            .expect("Encoding failed");
        assert_eq!(buf, &[1u8, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn encode_handle() {
        type T = HandleType<NullableHandle, OBJECT_TYPE_NONE, SAME_RIGHTS>;
        for ctx in CONTEXTS {
            let handle = crate::handle::Event::create().into_handle();
            let raw_handle = handle.raw_handle();
            let buf = &mut Vec::new();
            let handle_buf = &mut Vec::new();
            Encoder::<DefaultFuchsiaResourceDialect>::encode_with_context::<T>(
                ctx, buf, handle_buf, handle,
            )
            .expect("Encoding failed");

            assert_eq!(handle_buf.len(), 1);
            assert!(handle_buf[0].is_move());
            assert_eq!(handle_buf[0].raw_handle(), raw_handle);

            let mut handle_out = new_empty!(T);
            Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<T>(
                ctx,
                buf,
                &mut to_infos(handle_buf),
                &mut handle_out,
            )
            .expect("Decoding failed");
            assert_eq!(
                handle_out.raw_handle(),
                raw_handle,
                "decoded handle must match encoded handle"
            );
        }
    }

    #[test]
    fn decode_too_few_handles() {
        type T = HandleType<NullableHandle, OBJECT_TYPE_NONE, SAME_RIGHTS>;
        for ctx in CONTEXTS {
            let bytes: &[u8] = &[0xff; 8];
            let handle_buf = &mut Vec::new();
            let mut handle_out = NullableHandle::invalid();
            let res = Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<T>(
                ctx,
                bytes,
                handle_buf,
                &mut handle_out,
            );
            assert_matches!(res, Err(Error::OutOfHandles));
        }
    }

    #[test]
    fn encode_epitaph() {
        for ctx in CONTEXTS {
            let buf = &mut Vec::new();
            let handle_buf = &mut Vec::new();
            Encoder::<DefaultFuchsiaResourceDialect>::encode_with_context::<EpitaphBody>(
                ctx,
                buf,
                handle_buf,
                &EpitaphBody { error: Err(zx_status::Status::UNAVAILABLE) },
            )
            .expect("encoding failed");
            assert_eq!(buf, &[0xe4, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00]);

            let mut out = new_empty!(EpitaphBody, DefaultFuchsiaResourceDialect);
            Decoder::<DefaultFuchsiaResourceDialect>::decode_with_context::<EpitaphBody>(
                ctx,
                buf,
                &mut to_infos(handle_buf),
                &mut out,
            )
            .expect("Decoding failed");
            assert_eq!(EpitaphBody { error: Err(zx_status::Status::UNAVAILABLE) }, out);
        }
    }
}
