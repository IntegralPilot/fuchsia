// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon handles.

use crate::{
    Koid, MonotonicInstant, Name, ObjectQuery, ObjectType, Port, Property, PropertyQuery, Rights,
    Signals, Status, Topic, WaitAsyncOpts, WaitItem, ok, sys,
};
use std::marker::PhantomData;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::num::NonZeroU32;
use zerocopy::{FromBytes, Immutable, IntoBytes};

/// Tuning constant for Handle::get_info_vec(). pub(crate) to support unit tests.
pub(crate) const INFO_VEC_SIZE_INITIAL: usize = 16;

/// An owned and valid Zircon
/// [handle](https://fuchsia.dev/fuchsia-src/concepts/objects/handles) to a kernel object.
///
/// This type can be interconverted to and from more specific types. Those conversions are not
/// enforced in the type system; attempting to use them will result in errors returned by the
/// kernel. These conversions don't change the underlying representation, but do change the type and
/// what operations are available.
///
/// # Lifecycle
///
/// This type closes the handle it owns when dropped.
///
/// # Layout
///
/// `Option<Handle>` is guaranteed to have the same layout and bit patterns as `zx_handle_t`.
/// Unlike many types in this crate it does not implement `zerocopy` traits because those are not
/// appropriate for types with real `Drop` implementations.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Handle(NonZeroU32);

// Ensure ABI-compatibility with zx_handle_t with the following static assertions. NonZeroU32 lets
// Option store the None variant in the all-zeroes bit pattern, which is ZX_HANDLE_INVALID. By
// banning use of ZX_HANDLE_INVALID inside a Handle, we ensure that Option<Handle> is ABI-compatible
// with sys::zx_handle_t while providing statically checkable nullability.
static_assertions::const_assert_eq!(sys::ZX_HANDLE_INVALID, 0);
static_assertions::assert_eq_size!(Handle, sys::zx_handle_t);
static_assertions::assert_eq_align!(Handle, sys::zx_handle_t);
static_assertions::assert_eq_size!(Option<Handle>, sys::zx_handle_t);
static_assertions::assert_eq_align!(Option<Handle>, sys::zx_handle_t);

impl Handle {
    /// Take exclusive ownership over a raw handle.
    ///
    /// # Safety
    ///
    /// `raw` must either be `ZX_HANDLE_INVALID` or be a valid handle present in the current handle
    /// table that will not be closed by another owner.
    pub unsafe fn from_raw(raw: sys::zx_handle_t) -> Option<Self> {
        let inner = NonZeroU32::new(raw)?;
        debug_assert!(Self::check_raw_valid(raw).is_ok());
        Some(Self(inner))
    }

    /// Take exclusive ownership over a raw handle.
    ///
    /// # Safety
    ///
    /// `raw` must be a valid handle present in the current handle table that will not be closed by
    /// another owner.
    pub unsafe fn from_raw_unchecked(raw: sys::zx_handle_t) -> Self {
        debug_assert!(Self::check_raw_valid(raw).is_ok());
        // SAFETY: invariant is passed on to our caller.
        Self(unsafe { NonZeroU32::new_unchecked(raw) })
    }

    // A program which uses a handle returned from this function for anything other than being
    // `Drop`ped may be terminated by the kernel.
    #[doc(hidden)]
    pub fn poison() -> Self {
        Self(unsafe { NonZeroU32::new_unchecked(1) })
    }

    #[doc(hidden)]
    pub fn is_poison(&self) -> bool {
        // From https://fuchsia.dev/fuchsia-src/concepts/kernel/handles:
        //
        // > The integer value for a handle is any 32-bit number except the value corresponding to
        // > `ZX_HANDLE_INVALID` which will always have the value of 0. In addition to this, the
        // > integer value of a valid handle will always have two least significant bits of the
        // > handle set. The mask representing these bits may be accessed using
        // > `ZX_HANDLE_FIXED_BITS_MASK`.
        (self.0.get() & sys::ZX_HANDLE_FIXED_BITS_MASK) != sys::ZX_HANDLE_FIXED_BITS_MASK
    }

    /// Wraps the
    /// [zx_handle_check_valid](https://fuchsia.dev/fuchsia-src/reference/syscalls/handle_check_valid.md)
    /// syscall.
    ///
    /// Note that this does *not* guarantee that the handle is safe to pass to `Handle::from_raw`
    /// in cases where another function may close the handle.
    pub fn check_raw_valid(raw: sys::zx_handle_t) -> Result<(), Status> {
        // SAFETY: basic FFI call.
        ok(unsafe { sys::zx_handle_check_valid(raw) })
    }

    /// Returns the raw handle's integer value.
    pub const fn raw_handle(&self) -> sys::zx_handle_t {
        self.0.get()
    }

    /// Return the raw handle's integer value without closing it when `self` is dropped.
    pub fn into_raw(self) -> sys::zx_handle_t {
        let ret = self.0.get();
        std::mem::forget(self);
        ret
    }

    /// Wraps the
    /// [`zx_handle_duplicate`](https://fuchsia.dev/fuchsia-src/reference/syscalls/handle_duplicate)
    /// syscall.
    pub fn duplicate(&self, rights: Rights) -> Result<Self, Status> {
        let mut out = 0;
        // SAFETY: basic FFI call.
        let status =
            unsafe { sys::zx_handle_duplicate(self.raw_handle(), rights.bits(), &mut out) };
        ok(status)?;

        // SAFETY: zx_handle_duplicate returns a valid handle that this function owns.
        Ok(unsafe { Self::from_raw_unchecked(out) })
    }

    /// Wraps the
    /// [`zx_handle_replace`](https://fuchsia.dev/fuchsia-src/reference/syscalls/handle_replace)
    /// syscall.
    pub fn replace(self, rights: Rights) -> Result<Self, Status> {
        let mut out = 0;

        // SAFETY: basic FFI call.
        let status = unsafe { sys::zx_handle_replace(self.into_raw(), rights.bits(), &mut out) };
        ok(status)?;

        // SAFETY: zx_handle_replace gives us a valid owned handle if the call succeeded.
        unsafe { Ok(Self::from_raw_unchecked(out)) }
    }

    /// Wraps the [`zx_object_signal`](https://fuchsia.dev/reference/syscalls/object_signal) syscall.
    pub fn signal(&self, clear_mask: Signals, set_mask: Signals) -> Result<(), Status> {
        // SAFETY: basic FFI call.
        ok(unsafe { sys::zx_object_signal(self.raw_handle(), clear_mask.bits(), set_mask.bits()) })
    }

    /// Wraps the [`zx_object_wait_one`](https://fuchsia.dev/reference/syscalls/object_wait_one)
    /// syscall.
    pub fn wait_one(&self, signals: Signals, deadline: MonotonicInstant) -> WaitResult {
        let mut pending = Signals::empty().bits();
        // SAFETY: basic FFI call.
        let status = unsafe {
            sys::zx_object_wait_one(
                self.raw_handle(),
                signals.bits(),
                deadline.into_nanos(),
                &mut pending,
            )
        };
        let signals = Signals::from_bits_truncate(pending);
        match ok(status) {
            Ok(()) => WaitResult::Ok(signals),
            Err(Status::TIMED_OUT) => WaitResult::TimedOut(signals),
            Err(Status::CANCELED) => WaitResult::Canceled(signals),
            Err(e) => WaitResult::Err(e),
        }
    }

    /// Wraps the [zx_object_wait_async](https://fuchsia.dev/reference/syscalls/object_wait_async)
    /// syscall.
    pub fn wait_async(
        &self,
        port: &Port,
        key: u64,
        signals: Signals,
        options: WaitAsyncOpts,
    ) -> Result<(), Status> {
        // SAFETY: basic FFI call.
        ok(unsafe {
            sys::zx_object_wait_async(
                self.raw_handle(),
                port.raw_handle(),
                key,
                signals.bits(),
                options.bits(),
            )
        })
    }

    /// Return a [`WaitItem`] for this handle and `signals` that can be used with
    /// [`object_wait_many`].
    pub fn wait_item(&self, signals: Signals) -> WaitItem<'_> {
        WaitItem::new(self.as_handle_ref(), signals)
    }

    /// Get the [Property::NAME] property for this object.
    ///
    /// Wraps a call to the
    /// [zx_object_get_property](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_property.md)
    /// syscall for the `ZX_PROP_NAME` property.
    pub fn get_name(&self) -> Result<Name, Status> {
        self.get_property::<NameProperty>()
    }

    /// Set the [Property::NAME] property for this object.
    ///
    /// The name's length must be less than [sys::ZX_MAX_NAME_LEN], i.e.
    /// name.[to_bytes_with_nul()](CStr::to_bytes_with_nul()).len() <= [sys::ZX_MAX_NAME_LEN], or
    /// Err([Status::INVALID_ARGS]) will be returned.
    ///
    /// Wraps a call to the
    /// [`zx_object_get_property`](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_property.md)
    /// syscall for the `ZX_PROP_NAME` property.
    pub fn set_name(&self, name: &Name) -> Result<(), Status> {
        self.set_property::<NameProperty>(&name)
    }

    /// Get a property on a zircon object
    pub(crate) fn get_property<P: PropertyQuery>(&self) -> Result<P::PropTy, Status>
    where
        P::PropTy: FromBytes + Immutable,
    {
        let mut out = ::std::mem::MaybeUninit::<P::PropTy>::uninit();

        // SAFETY: safe due to the contract on the P::PropTy type in the ObjectProperty trait.
        let status = unsafe {
            sys::zx_object_get_property(
                self.raw_handle(),
                *P::PROPERTY,
                out.as_mut_ptr().cast::<u8>(),
                std::mem::size_of::<P::PropTy>(),
            )
        };
        Status::ok(status).map(|_| unsafe { out.assume_init() })
    }

    /// Set a property on a zircon object
    pub(crate) fn set_property<P: PropertyQuery>(&self, val: &P::PropTy) -> Result<(), Status>
    where
        P::PropTy: IntoBytes + Immutable,
    {
        let status = unsafe {
            sys::zx_object_set_property(
                self.raw_handle(),
                *P::PROPERTY,
                std::ptr::from_ref(val).cast::<u8>(),
                std::mem::size_of::<P::PropTy>(),
            )
        };
        Status::ok(status)
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_HANDLE_BASIC topic.
    pub fn basic_info(&self) -> Result<HandleBasicInfo, Status> {
        Ok(HandleBasicInfo::from(self.get_info_single::<HandleBasicInfoQuery>()?))
    }

    /// Wraps the
    /// [zx_object_get_info](https://fuchsia.dev/fuchsia-src/reference/syscalls/object_get_info.md)
    /// syscall for the ZX_INFO_HANDLE_COUNT topic.
    pub fn count_info(&self) -> Result<HandleCountInfo, Status> {
        Ok(HandleCountInfo::from(self.get_info_single::<HandleCountInfoQuery>()?))
    }

    /// Returns the koid (kernel object ID) for the object to which this handle refers.
    pub fn koid(&self) -> Result<Koid, Status> {
        self.basic_info().map(|info| info.koid)
    }

    /// Query information about a zircon object. Returns a valid slice and any remaining capacity on
    /// success, along with a count of how many infos the kernel had available.
    pub(crate) fn get_info<'a, Q: ObjectQuery>(
        &self,
        out: &'a mut [MaybeUninit<Q::InfoTy>],
    ) -> Result<(&'a mut [Q::InfoTy], &'a mut [MaybeUninit<Q::InfoTy>], usize), Status>
    where
        Q::InfoTy: FromBytes + Immutable,
    {
        let mut actual = 0;
        let mut avail = 0;

        // SAFETY: The slice pointer is known valid to write to for `size_of_val` because it came
        // from a mutable reference.
        let status = unsafe {
            sys::zx_object_get_info(
                self.raw_handle(),
                *Q::TOPIC,
                out.as_mut_ptr().cast::<u8>(),
                std::mem::size_of_val(out),
                &mut actual,
                &mut avail,
            )
        };
        ok(status)?;

        let (initialized, uninit) = out.split_at_mut(actual);

        // TODO(https://fxbug.dev/352398385) switch to MaybeUninit::slice_assume_init_mut
        // SAFETY: these values have been initialized by the kernel and implement the right zerocopy
        // traits to be instantiated from arbitrary bytes.
        let initialized: &mut [Q::InfoTy] = unsafe {
            std::slice::from_raw_parts_mut(
                initialized.as_mut_ptr().cast::<Q::InfoTy>(),
                initialized.len(),
            )
        };

        Ok((initialized, uninit, avail))
    }

    /// Query information about a zircon object, expecting only a single info in the return.
    pub(crate) fn get_info_single<Q: ObjectQuery>(&self) -> Result<Q::InfoTy, Status>
    where
        Q::InfoTy: Copy + FromBytes + Immutable,
    {
        let mut info = MaybeUninit::<Q::InfoTy>::uninit();
        let (info, _uninit, _avail) = self.get_info::<Q>(std::slice::from_mut(&mut info))?;
        Ok(info[0])
    }

    /// Query multiple records of information about a zircon object.
    /// Returns a vec of Q::InfoTy on success.
    /// Intended for calls that return multiple small objects.
    pub(crate) fn get_info_vec<Q: ObjectQuery>(&self) -> Result<Vec<Q::InfoTy>, Status> {
        // Start with a few slots
        let mut out = Vec::<Q::InfoTy>::with_capacity(INFO_VEC_SIZE_INITIAL);
        loop {
            let (init, _uninit, avail) = self.get_info::<Q>(out.spare_capacity_mut())?;
            let num_initialized = init.len();
            if num_initialized == avail {
                // SAFETY: the kernel has initialized all of these values.
                unsafe { out.set_len(num_initialized) };
                return Ok(out);
            } else {
                if avail > out.capacity() {
                    out.reserve_exact(avail - out.len());
                }
            }
        }
    }
}

impl AsHandleRef for Handle {
    fn as_handle_ref(&self) -> HandleRef<'_> {
        // SAFETY: inner is a guaranteed valid handle that will not be closed for self's lifetime.
        unsafe { Unowned::from_raw_handle(self.raw_handle()) }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.is_poison() {
            // SAFETY: basic FFI call.
            unsafe { sys::zx_handle_close(self.0.get()) };
        }
    }
}

/// An object representing a Zircon
/// [handle](https://fuchsia.dev/fuchsia-src/concepts/objects/handles).
///
/// Internally, it is represented as a 32-bit integer, but this wrapper enforces
/// strict ownership semantics. The `Drop` implementation closes the handle.
///
/// This type represents the most general reference to a kernel object, and can
/// be interconverted to and from more specific types. Those conversions are not
/// enforced in the type system; attempting to use them will result in errors
/// returned by the kernel. These conversions don't change the underlying
/// representation, but do change the type and thus what operations are available.
// TODO(https://fxbug.dev/465766514): remove
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct NullableHandle(Option<Handle>);

impl AsHandleRef for NullableHandle {
    fn as_handle_ref(&self) -> HandleRef<'_> {
        if let Some(inner) = &self.0 {
            // SAFETY: inner is a guaranteed valid handle.
            unsafe { Unowned::from_raw_handle(inner.raw_handle()) }
        } else {
            // SAFETY: ZX_HANDLE_INVALID is a valid handle for Unowned::from_raw_handle.
            unsafe { Unowned::from_raw_handle(sys::ZX_HANDLE_INVALID) }
        }
    }
}

impl NullableHandle {
    /// Initialize a handle backed by ZX_HANDLE_INVALID, the only safe non-handle.
    #[inline(always)]
    pub const fn invalid() -> Self {
        Self(None)
    }

    /// If a raw handle is obtained from some other source, this method converts
    /// it into a type-safe owned handle.
    ///
    /// # Safety
    ///
    /// `raw` must either be a valid handle (i.e. not dangling), or
    /// `ZX_HANDLE_INVALID`. If `raw` is a valid handle, then either:
    /// - `raw` may be closed manually and the returned `NullableHandle` must not be
    ///   dropped.
    /// - Or `raw` must not be closed until the returned `NullableHandle` is dropped, at
    ///   which time it will close `raw`.
    pub const unsafe fn from_raw(raw: sys::zx_handle_t) -> Self {
        // We need to manually construct the inner `Handle` because its constructor is not
        // const since the only valid way to call this function in a `const` context is to pass
        // `ZX_HANDLE_INVALID`.
        if let Some(inner) = NonZeroU32::new(raw) { Self(Some(Handle(inner))) } else { Self(None) }
    }

    pub const fn raw_handle(&self) -> sys::zx_handle_t {
        if let Some(inner) = &self.0 { inner.raw_handle() } else { sys::ZX_HANDLE_INVALID }
    }

    pub fn into_raw(self) -> sys::zx_handle_t {
        self.0.map(Handle::into_raw).unwrap_or(sys::ZX_HANDLE_INVALID)
    }

    pub fn as_handle_ref(&self) -> HandleRef<'_> {
        AsHandleRef::as_handle_ref(self)
    }

    pub const fn is_invalid(&self) -> bool {
        self.0.is_none()
    }

    pub fn duplicate_handle(&self, rights: Rights) -> Result<Self, Status> {
        self.0
            .as_ref()
            .map(|h| h.duplicate(rights).map(|new| Self(Some(new))))
            .unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub fn replace_handle(mut self, rights: Rights) -> Result<Self, Status> {
        if let Some(inner) = self.0.take() {
            let inner = inner.replace(rights)?;
            Ok(Self(Some(inner)))
        } else {
            Ok(Self(None))
        }
    }

    pub fn signal(&self, clear_mask: Signals, set_mask: Signals) -> Result<(), Status> {
        self.0.as_ref().map(|h| h.signal(clear_mask, set_mask)).unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub fn wait_one(&self, signals: Signals, deadline: MonotonicInstant) -> WaitResult {
        self.0
            .as_ref()
            .map(|h| h.wait_one(signals, deadline))
            .unwrap_or(WaitResult::Err(Status::BAD_HANDLE))
    }

    pub fn wait_async(
        &self,
        port: &Port,
        key: u64,
        signals: Signals,
        options: WaitAsyncOpts,
    ) -> Result<(), Status> {
        self.0
            .as_ref()
            .map(|h| h.wait_async(port, key, signals, options))
            .unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub fn wait_item(&self, signals: Signals) -> WaitItem<'_> {
        WaitItem::new(self.as_handle_ref(), signals)
    }

    pub fn get_name(&self) -> Result<Name, Status> {
        self.0.as_ref().map(Handle::get_name).unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub fn set_name(&self, name: &Name) -> Result<(), Status> {
        self.0.as_ref().map(|h| h.set_name(name)).unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub fn basic_info(&self) -> Result<HandleBasicInfo, Status> {
        self.0.as_ref().map(Handle::basic_info).unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub fn count_info(&self) -> Result<HandleCountInfo, Status> {
        self.0.as_ref().map(Handle::count_info).unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub fn koid(&self) -> Result<Koid, Status> {
        self.0.as_ref().map(Handle::koid).unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub(crate) fn get_info<'a, Q: ObjectQuery>(
        &self,
        out: &'a mut [MaybeUninit<Q::InfoTy>],
    ) -> Result<(&'a mut [Q::InfoTy], &'a mut [MaybeUninit<Q::InfoTy>], usize), Status>
    where
        Q::InfoTy: FromBytes + Immutable,
    {
        self.0.as_ref().map(|h| h.get_info::<Q>(out)).unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub(crate) fn get_info_single<Q: ObjectQuery>(&self) -> Result<Q::InfoTy, Status>
    where
        Q::InfoTy: Copy + FromBytes + Immutable,
    {
        self.0.as_ref().map(|h| h.get_info_single::<Q>()).unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub(crate) fn get_info_vec<Q: ObjectQuery>(&self) -> Result<Vec<Q::InfoTy>, Status> {
        self.0.as_ref().map(|h| h.get_info_vec::<Q>()).unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub(crate) fn get_property<P: PropertyQuery>(&self) -> Result<P::PropTy, Status>
    where
        P::PropTy: FromBytes + Immutable,
    {
        self.0.as_ref().map(|h| h.get_property::<P>()).unwrap_or(Err(Status::BAD_HANDLE))
    }

    pub(crate) fn set_property<P: PropertyQuery>(&self, val: &P::PropTy) -> Result<(), Status>
    where
        P::PropTy: IntoBytes + Immutable,
    {
        self.0.as_ref().map(|h| h.set_property::<P>(val)).unwrap_or(Err(Status::BAD_HANDLE))
    }
}

impl From<Handle> for NullableHandle {
    fn from(h: Handle) -> Self {
        Self(Some(h))
    }
}

impl From<Option<Handle>> for NullableHandle {
    fn from(h: Option<Handle>) -> Self {
        Self(h)
    }
}

impl TryFrom<NullableHandle> for Handle {
    type Error = Status;
    fn try_from(h: NullableHandle) -> Result<Self, Self::Error> {
        h.0.ok_or(Status::BAD_HANDLE)
    }
}

struct NameProperty();
// SAFETY: this type is correctly sized and the kernel guarantees that it will be
// null-terminated like the type requires.
unsafe impl PropertyQuery for NameProperty {
    const PROPERTY: Property = Property::NAME;
    type PropTy = Name;
}

/// A borrowed value of type `T`.
///
/// This is primarily used for working with borrowed values of handle wrapper types.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Unowned<'a, T> {
    inner: ManuallyDrop<T>,
    marker: PhantomData<&'a T>,
}

// Ensure ABI-compatibility with zx_handle_t with the following static assertions, like on Handle.
static_assertions::assert_eq_size!(Unowned<'static, Handle>, sys::zx_handle_t);
static_assertions::assert_eq_align!(Unowned<'static, Handle>, sys::zx_handle_t);
static_assertions::assert_eq_size!(Option<Unowned<'static, Handle>>, sys::zx_handle_t);
static_assertions::assert_eq_align!(Option<Unowned<'static, Handle>>, sys::zx_handle_t);

impl<'a, T> std::ops::Deref for Unowned<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &*self.inner
    }
}

impl<'a, T: AsHandleRef> AsHandleRef for Unowned<'a, T> {
    fn as_handle_ref(&self) -> HandleRef<'_> {
        self.inner.as_handle_ref()
    }
}

impl<T: AsHandleRef + From<NullableHandle> + Into<NullableHandle>> Clone for Unowned<'_, T> {
    fn clone(&self) -> Self {
        unsafe { Self::from_raw_handle(self.inner.as_handle_ref().raw_handle()) }
    }
}

pub type HandleRef<'a> = Unowned<'a, NullableHandle>;

impl<'a, T: Into<NullableHandle>> Unowned<'a, T> {
    /// Returns a new object that borrows the underyling handle.  This will work for any type that
    /// implements `From<U>` where `U` is handle-like i.e. it implements `AsHandleRef` and
    /// `From<Handle>`.
    pub fn new<U: AsHandleRef + From<NullableHandle>>(inner: &'a U) -> Self
    where
        T: From<U>,
    {
        // SAFETY: This is safe because we are converting from &U to U to allow us to create T, and
        // then when we drop, we convert T into a handle that we forget.
        Unowned {
            inner: ManuallyDrop::new(T::from(U::from(unsafe {
                NullableHandle::from_raw(inner.as_handle_ref().raw_handle())
            }))),
            marker: PhantomData,
        }
    }
}

impl<'a, T: AsHandleRef + From<NullableHandle> + Into<NullableHandle>> Unowned<'a, T> {
    /// Create a `HandleRef` from a raw handle. Use this method when you are given a raw handle but
    /// should not take ownership of it. Examples include process-global handles like the root
    /// VMAR. This method should be called with an explicitly provided lifetime that must not
    /// outlive the lifetime during which the handle is owned by the current process. It is unsafe
    /// because most of the time, it is better to use a `Handle` to prevent leaking resources.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid handle (i.e. not dangling), or
    /// `ZX_HANDLE_INVALID`. If `handle` is a valid handle, then it must not be
    /// closed for the lifetime `'a`.
    pub unsafe fn from_raw_handle(handle: sys::zx_handle_t) -> Self {
        Unowned {
            inner: ManuallyDrop::new(T::from(unsafe { NullableHandle::from_raw(handle) })),
            marker: PhantomData,
        }
    }

    /// Returns the raw handle's integer value.
    pub fn raw_handle(&self) -> sys::zx_handle_t {
        NullableHandle::raw_handle(&*self.inner.as_handle_ref())
    }
}

/// Result from `HandleRef::wait` and `AsHandleRef::wait_handle`. Conveys the
/// result of the
/// [zx_object_wait_one](https://fuchsia.dev/reference/syscalls/object_wait_one)
/// syscall and the signals that were asserted on the object when the syscall
/// completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WaitResult {
    /// The syscall completed with `ZX_OK` and the provided signals were observed.
    Ok(Signals),

    /// The syscall completed with `ZX_ERR_TIMED_OUT` and the provided signals
    /// were observed. These signals may reflect state changes that occurred
    /// after the deadline passed, but before the syscall returned.
    TimedOut(Signals),

    /// The syscall completed with `ZX_ERR_CANCELED` and the provided signals
    /// were observed. The signals will include `ZX_SIGNAL_HANDLE_CLOSED`.
    ///
    /// Note that the state of these signals may be racy and difficult to
    /// interpret. Often, the correct behavior in this case is to treat this as
    /// an error.
    Canceled(Signals),

    /// The syscall completed with a status other than `ZX_OK`, `ZX_ERR_TIMED_OUT`,
    /// or `ZX_ERR_CANCELED`. No signals are returned in this scenario.
    Err(Status),
}

impl WaitResult {
    /// Convert this `WaitResult` into a `Result<Signals, Status>`. The signals
    /// are discarded in all cases except `WaitResult::Ok`.
    pub const fn to_result(self) -> Result<Signals, Status> {
        match self {
            WaitResult::Ok(signals) => Ok(signals),
            WaitResult::TimedOut(_signals) => Err(Status::TIMED_OUT),
            WaitResult::Canceled(_signals) => Err(Status::CANCELED),
            WaitResult::Err(status) => Err(status),
        }
    }

    // The following definitions are all copied from `std::result::Result`. They
    // allow a `WaitResult` to be treated like a `Result` in many circumstance. All
    // simply delegate to `to_result()`.

    #[must_use = "if you intended to assert that this is ok, consider `.unwrap()` instead"]
    #[inline]
    pub const fn is_ok(&self) -> bool {
        self.to_result().is_ok()
    }

    #[must_use = "if you intended to assert that this is err, consider `.unwrap_err()` instead"]
    #[inline]
    pub const fn is_err(&self) -> bool {
        self.to_result().is_err()
    }

    #[inline]
    pub fn map<U, F: FnOnce(Signals) -> U>(self, op: F) -> Result<U, Status> {
        self.to_result().map(op)
    }

    #[inline]
    pub fn map_err<F, O: FnOnce(Status) -> F>(self, op: O) -> Result<Signals, F> {
        self.to_result().map_err(op)
    }

    #[inline]
    #[track_caller]
    pub fn expect(self, msg: &str) -> Signals {
        self.to_result().expect(msg)
    }

    #[inline]
    #[track_caller]
    pub fn expect_err(self, msg: &str) -> Status {
        self.to_result().expect_err(msg)
    }

    #[inline(always)]
    #[track_caller]
    pub fn unwrap(self) -> Signals {
        self.to_result().unwrap()
    }
}

impl<'a> Unowned<'a, NullableHandle> {
    /// Convert this HandleRef to one of a specific type.
    pub fn cast<T: AsHandleRef + From<NullableHandle> + Into<NullableHandle>>(
        self,
    ) -> Unowned<'a, T> {
        // SAFETY: this function's guarantees are upheld by the self input.
        unsafe { Unowned::from_raw_handle(self.raw_handle()) }
    }
}

/// A trait to get a reference to the underlying handle of an object.
pub trait AsHandleRef {
    /// Get a reference to the handle. One important use of such a reference is
    /// for `object_wait_many`.
    fn as_handle_ref(&self) -> HandleRef<'_>;
}

impl<T: AsHandleRef> AsHandleRef for &T {
    fn as_handle_ref(&self) -> HandleRef<'_> {
        (*self).as_handle_ref()
    }
}

/// A trait implemented by all handles for objects which have a peer.
pub trait Peered: AsHandleRef {
    /// Set and clear userspace-accessible signal bits on the object's peer. Wraps the
    /// [zx_object_signal_peer][osp] syscall.
    ///
    /// [osp]: https://fuchsia.dev/fuchsia-src/reference/syscalls/object_signal_peer.md
    fn signal_peer(&self, clear_mask: Signals, set_mask: Signals) -> Result<(), Status> {
        let handle = self.as_handle_ref().raw_handle();
        let status =
            unsafe { sys::zx_object_signal_peer(handle, clear_mask.bits(), set_mask.bits()) };
        ok(status)
    }

    /// Returns true if the handle has received the `PEER_CLOSED` signal.
    ///
    /// # Errors
    ///
    /// See https://fuchsia.dev/reference/syscalls/object_wait_one?hl=en#errors for a full list of
    /// errors. Note that `Status::TIMED_OUT` errors are converted to `Ok(false)` and all other
    /// errors are propagated.
    fn is_closed(&self) -> Result<bool, Status> {
        match self
            .as_handle_ref()
            .wait_one(Signals::OBJECT_PEER_CLOSED, MonotonicInstant::INFINITE_PAST)
        {
            WaitResult::Ok(signals) => Ok(signals.contains(Signals::OBJECT_PEER_CLOSED)),
            WaitResult::TimedOut(_) => Ok(false),
            WaitResult::Canceled(_) => Err(Status::CANCELED),
            WaitResult::Err(e) => Err(e),
        }
    }
}

/// Basic information about a handle.
///
/// Wrapper for data returned from [Handle::basic_info()].
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct HandleBasicInfo {
    pub koid: Koid,
    pub rights: Rights,
    pub object_type: ObjectType,
    pub related_koid: Koid,
}

impl Default for HandleBasicInfo {
    fn default() -> Self {
        Self::from(sys::zx_info_handle_basic_t::default())
    }
}

impl From<sys::zx_info_handle_basic_t> for HandleBasicInfo {
    fn from(info: sys::zx_info_handle_basic_t) -> Self {
        let sys::zx_info_handle_basic_t { koid, rights, type_, related_koid, .. } = info;

        // Note lossy conversion of Rights and HandleProperty here if either of those types are out
        // of date or incomplete.
        HandleBasicInfo {
            koid: Koid::from_raw(koid),
            rights: Rights::from_bits_truncate(rights),
            object_type: ObjectType::from_raw(type_),
            related_koid: Koid::from_raw(related_koid),
        }
    }
}

// zx_info_handle_basic_t is able to be safely replaced with a byte representation and is a PoD
// type.
struct HandleBasicInfoQuery;
unsafe impl ObjectQuery for HandleBasicInfoQuery {
    const TOPIC: Topic = Topic::HANDLE_BASIC;
    type InfoTy = sys::zx_info_handle_basic_t;
}

sys::zx_info_handle_count_t!(HandleCountInfo);

impl From<sys::zx_info_handle_count_t> for HandleCountInfo {
    fn from(sys::zx_info_handle_count_t { handle_count }: sys::zx_info_handle_count_t) -> Self {
        HandleCountInfo { handle_count }
    }
}

// zx_info_handle_count_t is able to be safely replaced with a byte representation and is a PoD
// type.
struct HandleCountInfoQuery;
unsafe impl ObjectQuery for HandleCountInfoQuery {
    const TOPIC: Topic = Topic::HANDLE_COUNT;
    type InfoTy = sys::zx_info_handle_count_t;
}

/// Handle operation.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum HandleOp<'a> {
    Move(NullableHandle),
    Duplicate(HandleRef<'a>),
}

/// Operation to perform on handles during write. ABI-compatible with `zx_handle_disposition_t`.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(C)]
pub struct HandleDisposition<'a> {
    // Must be either ZX_HANDLE_OP_MOVE or ZX_HANDLE_OP_DUPLICATE.
    operation: sys::zx_handle_op_t,
    // ZX_HANDLE_OP_MOVE==owned, ZX_HANDLE_OP_DUPLICATE==borrowed.
    handle: sys::zx_handle_t,
    // Preserve a borrowed handle's lifetime. Does not occupy any layout.
    _handle_lifetime: std::marker::PhantomData<&'a ()>,

    pub object_type: ObjectType,
    pub rights: Rights,
    pub result: Result<(), Status>,
}

static_assertions::assert_eq_size!(HandleDisposition<'_>, sys::zx_handle_disposition_t);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(HandleDisposition<'_>, operation),
    std::mem::offset_of!(sys::zx_handle_disposition_t, operation)
);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(HandleDisposition<'_>, handle),
    std::mem::offset_of!(sys::zx_handle_disposition_t, handle)
);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(HandleDisposition<'_>, object_type),
    std::mem::offset_of!(sys::zx_handle_disposition_t, type_)
);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(HandleDisposition<'_>, rights),
    std::mem::offset_of!(sys::zx_handle_disposition_t, rights)
);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(HandleDisposition<'_>, result),
    std::mem::offset_of!(sys::zx_handle_disposition_t, result)
);

impl<'a> HandleDisposition<'a> {
    #[inline]
    pub fn new<R: Into<Result<(), Status>>>(
        handle_op: HandleOp<'a>,
        object_type: ObjectType,
        rights: Rights,
        result: R,
    ) -> Self {
        let (operation, handle) = match handle_op {
            HandleOp::Move(h) => (sys::ZX_HANDLE_OP_MOVE, h.into_raw()),
            HandleOp::Duplicate(h) => (sys::ZX_HANDLE_OP_DUPLICATE, h.raw_handle()),
        };

        Self {
            operation,
            handle,
            _handle_lifetime: std::marker::PhantomData,
            object_type,
            rights,
            result: result.into(),
        }
    }

    pub fn raw_handle(&self) -> sys::zx_handle_t {
        self.handle
    }

    pub fn is_move(&self) -> bool {
        self.operation == sys::ZX_HANDLE_OP_MOVE
    }

    pub fn is_duplicate(&self) -> bool {
        self.operation == sys::ZX_HANDLE_OP_DUPLICATE
    }

    pub fn take_op(&mut self) -> HandleOp<'a> {
        match self.operation {
            sys::ZX_HANDLE_OP_MOVE => {
                // SAFETY: this is guaranteed to be a valid handle number by a combination of this
                // type's public API and the kernel's guarantees.
                HandleOp::Move(unsafe {
                    NullableHandle::from_raw(std::mem::replace(
                        &mut self.handle,
                        sys::ZX_HANDLE_INVALID,
                    ))
                })
            }
            sys::ZX_HANDLE_OP_DUPLICATE => {
                // SAFETY: this is guaranteed to be a valid handle number by a combination of this
                // type's public API and the kernel's guarantees.
                HandleOp::Duplicate(Unowned {
                    inner: ManuallyDrop::new(unsafe { NullableHandle::from_raw(self.handle) }),
                    marker: PhantomData,
                })
            }
            _ => unreachable!(),
        }
    }

    pub fn into_raw(mut self) -> sys::zx_handle_disposition_t {
        match self.take_op() {
            HandleOp::Move(mut handle) => sys::zx_handle_disposition_t {
                operation: sys::ZX_HANDLE_OP_MOVE,
                handle: std::mem::replace(&mut handle, NullableHandle::invalid()).into_raw(),
                type_: self.object_type.into_raw(),
                rights: self.rights.bits(),
                result: Status::result_into_raw(self.result),
            },
            HandleOp::Duplicate(handle_ref) => sys::zx_handle_disposition_t {
                operation: sys::ZX_HANDLE_OP_DUPLICATE,
                handle: handle_ref.raw_handle(),
                type_: self.object_type.into_raw(),
                rights: self.rights.bits(),
                result: Status::result_into_raw(self.result),
            },
        }
    }
}

impl<'a> Drop for HandleDisposition<'a> {
    fn drop(&mut self) {
        // Ensure we clean up owned handle variants.
        if self.operation == sys::ZX_HANDLE_OP_MOVE {
            unsafe { drop(NullableHandle::from_raw(self.handle)) };
        }
    }
}

/// Information on handles that were read.
///
/// ABI-compatible with zx_handle_info_t.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(C)]
pub struct HandleInfo {
    pub handle: NullableHandle,
    pub object_type: ObjectType,
    pub rights: Rights,

    // Necessary for ABI compatibility with zx_handle_info_t.
    pub(crate) _unused: u32,
}

static_assertions::assert_eq_size!(HandleInfo, sys::zx_handle_info_t);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(HandleInfo, handle),
    std::mem::offset_of!(sys::zx_handle_info_t, handle)
);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(HandleInfo, object_type),
    std::mem::offset_of!(sys::zx_handle_info_t, ty)
);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(HandleInfo, rights),
    std::mem::offset_of!(sys::zx_handle_info_t, rights)
);
static_assertions::const_assert_eq!(
    std::mem::offset_of!(HandleInfo, _unused),
    std::mem::offset_of!(sys::zx_handle_info_t, unused)
);

impl HandleInfo {
    /// Make a new `HandleInfo`.
    pub const fn new(handle: NullableHandle, object_type: ObjectType, rights: Rights) -> Self {
        Self { handle, object_type, rights, _unused: 0 }
    }

    /// # Safety
    ///
    /// See [`Handle::from_raw`] for requirements about the validity and closing
    /// of `raw.handle`.
    ///
    /// Note that while `raw.ty` _should_ correspond to the type of the handle,
    /// that this is not required for safety.
    pub const unsafe fn from_raw(raw: sys::zx_handle_info_t) -> HandleInfo {
        HandleInfo::new(
            // SAFETY: invariants to not double-close are upheld by the caller.
            unsafe { NullableHandle::from_raw(raw.handle) },
            ObjectType::from_raw(raw.ty),
            Rights::from_bits_retain(raw.rights),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The unit tests are built with a different crate name, but fuchsia_runtime returns a "real"
    // zx::Vmar that we need to use.
    use zx::{
        Channel, HandleDisposition, HandleInfo, HandleOp, Name, NullableHandle, ObjectType, Rights,
        Vmo,
    };
    use zx_sys as sys;

    #[test]
    fn into_raw() {
        let vmo = Vmo::create(1).unwrap();
        let h = vmo.into_raw();
        let vmo2 = Vmo::from(unsafe { NullableHandle::from_raw(h) });
        assert!(vmo2.write(b"1", 0).is_ok());
    }

    #[test]
    fn check_raw_valid() {
        assert!(Handle::check_raw_valid(sys::ZX_HANDLE_INVALID).is_err());
        let vmo = Vmo::create(1).unwrap();
        let vmo_raw = vmo.raw_handle();
        assert!(Handle::check_raw_valid(vmo_raw).is_ok());
        drop(vmo);
        assert!(Handle::check_raw_valid(vmo_raw).is_err());
    }

    /// Test duplication by means of a VMO
    #[test]
    fn duplicate() {
        let hello_length: usize = 5;

        // Create a VMO and write some data to it.
        let vmo = Vmo::create(hello_length as u64).unwrap();
        assert!(vmo.write(b"hello", 0).is_ok());

        // Replace, reducing rights to read.
        let readonly_vmo = vmo.duplicate_handle(Rights::READ).unwrap();
        // Make sure we can read but not write.
        let mut read_vec = vec![0; hello_length];
        assert!(readonly_vmo.read(&mut read_vec, 0).is_ok());
        assert_eq!(read_vec, b"hello");
        assert_eq!(readonly_vmo.write(b"", 0), Err(Status::ACCESS_DENIED));

        // Write new data to the original handle, and read it from the new handle
        assert!(vmo.write(b"bye", 0).is_ok());
        assert!(readonly_vmo.read(&mut read_vec, 0).is_ok());
        assert_eq!(read_vec, b"byelo");
    }

    // Test replace by means of a VMO
    #[test]
    fn replace() {
        let hello_length: usize = 5;

        // Create a VMO and write some data to it.
        let vmo = Vmo::create(hello_length as u64).unwrap();
        assert!(vmo.write(b"hello", 0).is_ok());

        // Replace, reducing rights to read.
        let readonly_vmo = vmo.replace_handle(Rights::READ).unwrap();
        // Make sure we can read but not write.
        let mut read_vec = vec![0; hello_length];
        assert!(readonly_vmo.read(&mut read_vec, 0).is_ok());
        assert_eq!(read_vec, b"hello");
        assert_eq!(readonly_vmo.write(b"", 0), Err(Status::ACCESS_DENIED));
    }

    #[test]
    fn set_get_name() {
        // We need some concrete object to exercise the AsHandleRef<'_> set/get_name functions.
        let vmo = Vmo::create(1).unwrap();
        let short_name = Name::new("v").unwrap();
        assert!(vmo.set_name(&short_name).is_ok());
        assert_eq!(vmo.get_name().unwrap(), short_name);
    }

    #[test]
    fn set_get_max_len_name() {
        let vmo = Vmo::create(1).unwrap();
        let max_len_name = Name::new("a_great_maximum_length_vmo_name").unwrap(); // 31 bytes
        assert!(vmo.set_name(&max_len_name).is_ok());
        assert_eq!(vmo.get_name().unwrap(), max_len_name);
    }

    #[test]
    fn basic_info_channel() {
        let (side1, side2) = Channel::create();
        let info1 = side1.basic_info().expect("side1 basic_info failed");
        let info2 = side2.basic_info().expect("side2 basic_info failed");

        assert_eq!(info1.koid, info2.related_koid);
        assert_eq!(info2.koid, info1.related_koid);

        for info in &[info1, info2] {
            assert!(info.koid.raw_koid() >= sys::ZX_KOID_FIRST);
            assert_eq!(info.object_type, ObjectType::CHANNEL);
            assert!(info.rights.contains(Rights::READ | Rights::WRITE | Rights::WAIT));
        }

        let side1_repl = side1.replace_handle(Rights::READ).expect("side1 replace_handle failed");
        let info1_repl = side1_repl.basic_info().expect("side1_repl basic_info failed");
        assert_eq!(info1_repl.koid, info1.koid);
        assert_eq!(info1_repl.rights, Rights::READ);
    }

    #[test]
    fn basic_info_vmar() {
        // VMARs aren't waitable.
        let root_vmar = fuchsia_runtime::vmar_root_self();
        let info = root_vmar.basic_info().expect("vmar basic_info failed");
        assert_eq!(info.object_type, ObjectType::VMAR);
        assert!(!info.rights.contains(Rights::WAIT));
    }

    #[test]
    fn count_info() {
        let vmo0 = Vmo::create(1).unwrap();
        let count_info = vmo0.count_info().expect("vmo0 count_info failed");
        assert_eq!(count_info.handle_count, 1);

        let vmo1 = vmo0.duplicate_handle(Rights::SAME_RIGHTS).expect("vmo duplicate_handle failed");
        let count_info = vmo1.count_info().expect("vmo1 count_info failed");
        assert_eq!(count_info.handle_count, 2);
    }

    #[test]
    fn raw_handle_disposition() {
        const RAW_HANDLE: sys::zx_handle_t = 1;
        let hd = HandleDisposition::new(
            HandleOp::Move(unsafe { NullableHandle::from_raw(RAW_HANDLE) }),
            ObjectType::VMO,
            Rights::EXECUTE,
            Ok(()),
        );
        let raw_hd = hd.into_raw();
        assert_eq!(raw_hd.operation, sys::ZX_HANDLE_OP_MOVE);
        assert_eq!(raw_hd.handle, RAW_HANDLE);
        assert_eq!(raw_hd.rights, sys::ZX_RIGHT_EXECUTE);
        assert_eq!(raw_hd.type_, sys::ZX_OBJ_TYPE_VMO);
        assert_eq!(raw_hd.result, sys::ZX_OK);
    }

    #[test]
    fn regression_nullable_handle_into_raw_recursion() {
        let h = NullableHandle::invalid();
        // This should not stack overflow
        assert_eq!(h.into_raw(), sys::ZX_HANDLE_INVALID);

        let vmo = Vmo::create(1).unwrap();
        let raw = vmo.raw_handle();
        let h = vmo.into_handle();
        // This should not stack overflow
        assert_eq!(h.into_raw(), raw);
    }

    #[test]
    fn handle_info_from_raw() {
        const RAW_HANDLE: sys::zx_handle_t = 1;
        let raw_hi = sys::zx_handle_info_t {
            handle: RAW_HANDLE,
            ty: sys::ZX_OBJ_TYPE_VMO,
            rights: sys::ZX_RIGHT_EXECUTE,
            unused: 128,
        };
        let hi = unsafe { HandleInfo::from_raw(raw_hi) };
        assert_eq!(hi.handle.into_raw(), RAW_HANDLE);
        assert_eq!(hi.object_type, ObjectType::VMO);
        assert_eq!(hi.rights, Rights::EXECUTE);
    }

    #[test]
    fn basic_peer_closed() {
        let (lhs, rhs) = crate::EventPair::create();
        assert!(!lhs.is_closed().unwrap());
        assert!(!rhs.is_closed().unwrap());
        drop(rhs);
        assert!(lhs.is_closed().unwrap());
    }

    #[test]
    fn poisoned_drops_without_closing() {
        let handle = Handle::poison();
        assert!(handle.is_poison());
        drop(handle);
    }

    #[test]
    fn valid_handles_not_poisoned() {
        let event = zx::Event::create();
        let event: zx::Handle = event.try_into().unwrap();
        assert!(!event.is_poison());
    }
}
