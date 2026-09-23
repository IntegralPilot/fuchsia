// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use super::dispatcher_ffi::{
    cpp_dispatcher_add_observer, cpp_dispatcher_clear_signals, cpp_dispatcher_current_handle_count,
    cpp_dispatcher_get_name, cpp_dispatcher_get_ref_counted, cpp_dispatcher_get_related_koid,
    cpp_dispatcher_get_type, cpp_dispatcher_on_zero_handles, cpp_dispatcher_recycle,
    cpp_dispatcher_remove_observer, cpp_dispatcher_set_name, cpp_dispatcher_signals_state_locked,
    cpp_dispatcher_update_state, cpp_dispatcher_update_state_locked,
};
use super::handle::HandleValue;
use super::process_dispatcher_ffi::cpp_handle_table_get_dispatcher;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use fbl::{Recyclable, RefPtr, pin_make_ref_counted, ref_counted};
use kalloc::AllocError;
use ksync::{KMutex, LockToken, RawCriticalMutex, guarded};
use relaxed_atomic::RelaxedAtomicU64;
use zx_status::Status;
use zx_types::zx_rights_t;

/// Common trait defining ops for kernel Dispatcher facades.
pub trait DispatcherOps {
    type LockClass;
    const TYPE: zx_types::zx_obj_type_t;

    fn dispatcher(&self) -> *const Dispatcher;

    fn on_zero_handles(&self) {
        // SAFETY: self.dispatcher() returns a valid pointer to an initialized Dispatcher.
        unsafe {
            cpp_dispatcher_on_zero_handles(self.dispatcher());
        }
    }

    fn clear_signals(&self, signals: zx_types::zx_signals_t) {
        // SAFETY: self.dispatcher() returns a valid pointer to an initialized Dispatcher.
        unsafe {
            cpp_dispatcher_clear_signals(self.dispatcher(), signals);
        }
    }

    fn update_state(&self, clear_mask: u32, set_mask: u32) {
        self.update_state_with_strobe(clear_mask, set_mask, 0);
    }

    fn update_state_with_strobe(&self, clear_mask: u32, set_mask: u32, strobe_mask: u32) {
        // SAFETY: self.dispatcher() returns a valid pointer to an initialized Dispatcher.
        unsafe {
            cpp_dispatcher_update_state(self.dispatcher(), clear_mask, set_mask, strobe_mask);
        }
    }

    fn update_state_locked(
        &self,
        token: &LockToken<'_, Self::LockClass>,
        clear_mask: u32,
        set_mask: u32,
    ) {
        self.update_state_with_strobe_locked(token, clear_mask, set_mask, 0);
    }

    fn update_state_with_strobe_locked(
        &self,
        _token: &LockToken<'_, Self::LockClass>,
        clear_mask: u32,
        set_mask: u32,
        strobe_mask: u32,
    ) {
        // SAFETY: self.dispatcher() is valid, and the proof token guarantees the state lock is
        // held.
        unsafe {
            cpp_dispatcher_update_state_locked(
                self.dispatcher(),
                clear_mask,
                set_mask,
                strobe_mask,
            );
        }
    }

    fn signals_state_locked(
        &self,
        _token: &LockToken<'_, Self::LockClass>,
    ) -> zx_types::zx_signals_t {
        // SAFETY: self.dispatcher() is valid, and the proof token guarantees the state lock is
        // held.
        unsafe { cpp_dispatcher_signals_state_locked(self.dispatcher()) }
    }
}

/// Helper macro to declare facade structs and implement common facade traits for Dispatcher
/// subtypes.
macro_rules! impl_dispatcher_facade {
    ($(#[$meta:meta])* $vis:vis struct $type:ident, $obj_type:expr) => {
        $crate::object::dispatcher::impl_dispatcher_facade!($(#[$meta])* $vis struct $type, $obj_type, ());
    };
    ($(#[$meta:meta])* $vis:vis struct $type:ident, $obj_type:expr, $lock_class:ty) => {
        $(#[$meta])*
        #[repr(C)]
        $vis struct $type {
            _facade: fbl::OpaqueRefCountedFacade<$crate::object::Dispatcher>,
        }

        impl core::ops::Deref for $type {
            type Target = $crate::object::Dispatcher;
            fn deref(&self) -> &Self::Target {
                // SAFETY: `self` is a valid facade reference, and the base `Dispatcher`
                // is part of the same allocation.
                unsafe { &*<Self as $crate::object::DispatcherOps>::dispatcher(self) }
            }
        }

        // SAFETY: `$type` is a `#[repr(C)]` facade struct that starts with `Dispatcher`
        // at offset 0 and is layout-compatible with `Dispatcher`.
        unsafe impl fbl::IsOpaqueRefCounted for $type {
            type TargetBase = $crate::object::Dispatcher;
        }

        impl $crate::object::DispatcherOps for $type {
            const TYPE: zx_types::zx_obj_type_t = $obj_type;
            type LockClass = $lock_class;

            fn dispatcher(&self) -> *const $crate::object::Dispatcher {
                self as *const Self as *const $crate::object::Dispatcher
            }
        }
    };
}
pub(crate) use impl_dispatcher_facade;

/// Helper macro to declare facade structs and implement common facade traits and state access
/// methods for Dispatcher subtypes with state.
macro_rules! impl_dispatcher_facade_with_state {
    (
        @base
        $(#[$meta:meta])* $vis:vis struct $type:ident,
        $state:ident,
        $obj_type:expr,
        $offset_const:expr,
        $lock_class:ty,
        $lock_ret:ty,
        $get_lock:expr
    ) => {
        paste::paste! {
            $crate::object::dispatcher::impl_dispatcher_facade!($(#[$meta])* $vis struct $type, $obj_type, $lock_class);

            impl $type {
                /// Returns a reference to the underlying state object.
                pub fn state(&self) -> &$state {
                    // SAFETY: The state object is located at a verified offset within the
                    // same allocation as the facade.
                    unsafe {
                        let ptr = (self as *const Self)
                            .cast::<u8>()
                            .add($offset_const as usize)
                            .cast::<$state>();
                        &*ptr
                    }
                }
            }

            /// Returns a pointer to the mutex inside `$state`.
            ///
            /// # Safety
            ///
            /// When `$type` has a lock, `ptr` must point to a valid, initialized `$state`.
            ///
            /// # Panics
            ///
            /// Panics if `$type` does not have an internal lock (e.g. configured with `no_lock`).
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn [<rust_ $type:snake _state_get_lock>](
                ptr: *const $state,
            ) -> $lock_ret {
                ($get_lock)(ptr)
            }

            /// Destroys a `$state` in-place.
            ///
            /// # Safety
            ///
            /// The caller must ensure `state` is a valid reference to an initialized `$state`, and
            /// must not use the state (or the enclosing dispatcher) after this function returns.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn [<rust_ $type:snake _state_destroy>](
                state: &mut $state,
            ) {
                // SAFETY: The caller is destroying the dispatcher and will not use it again.
                unsafe {
                    core::ptr::drop_in_place(state);
                }
            }
        }
    };
    (
        @base
        $(#[$meta:meta])* $vis:vis struct $type:ident,
        $state:ident,
        $obj_type:expr,
        $offset_const:expr,
        $lock_class:ty,
        $get_lock:expr
    ) => {
        $crate::object::dispatcher::impl_dispatcher_facade_with_state!(
            @base
            $(#[$meta])* $vis struct $type,
            $state,
            $obj_type,
            $offset_const,
            $lock_class,
            *mut ksync::KMutex<$lock_class, ksync::RawCriticalMutex>,
            |ptr: *const $state| {
                let lock_ref: &ksync::KMutex<$lock_class, ksync::RawCriticalMutex> = ($get_lock)(ptr);
                zr::ToMutPtr::to_mut_ptr(lock_ref)
            }
        );
    };
    ($(#[$meta:meta])* $vis:vis struct $type:ident, $state:ident, $obj_type:expr, $offset_const:expr) => {
        paste::paste! {
            $crate::object::dispatcher::impl_dispatcher_facade_with_state!(
                @base
                $(#[$meta])* $vis struct $type,
                $state,
                $obj_type,
                $offset_const,
                [<$state LockClass>],
                // SAFETY: The caller of `rust_<type>_state_get_lock` guarantees `ptr` points
                // to a valid, initialized `$state`.
                |ptr: *const $state| unsafe { &(*ptr).lock }
            );
        }
    };
    (
        $(#[$meta:meta])* $vis:vis struct $type:ident,
        $state:ident,
        $obj_type:expr,
        $offset_const:expr,
        no_lock $(,)?
    ) => {
        $crate::object::dispatcher::impl_dispatcher_facade_with_state!(
            @base
            $(#[$meta])* $vis struct $type,
            $state,
            $obj_type,
            $offset_const,
            (),
            *mut core::ffi::c_void,
            |_ptr: *const $state| panic!(concat!(stringify!($type), " does not have a lock"))
        );
    };
}
pub(crate) use impl_dispatcher_facade_with_state;

/// Helper macro to declare facade structs and implement common facade traits, state access methods,
/// and peered dispatcher operations (init_peer, get_related_koid, user_signal_self,
/// user_signal_peer, on_zero_handles) for PeeredDispatcher subtypes with state.
macro_rules! impl_peered_dispatcher_facade_with_state {
    (
        $(#[$meta:meta])* $vis:vis struct $type:ident,
        $state:ident,
        $obj_type:expr,
        $offset_const:expr,
        allowed_signals: $allowed_signals:expr $(,)?
    ) => {
        $crate::object::dispatcher::impl_dispatcher_facade_with_state!(
            @base
            $(#[$meta])* $vis struct $type,
            $state,
            $obj_type,
            $offset_const,
            $crate::object::dispatcher::PeerHolderMuClass<$type>,
            // SAFETY: The caller of `rust_<type>_state_get_lock` guarantees `ptr` points
            // to a valid, initialized `$state`.
            |ptr: *const $state| unsafe { &(&(*ptr).peered.holder).mu }
        );

        impl $type {
            /// Initializes the peer reference and peer KOID.
            pub(crate) fn init_peer(&self, peer: fbl::RefPtr<Self>) {
                let peer_koid = peer.get_koid();
                ksync::lock!(let mut guard = self.state().peered.lock());
                *guard.as_mut().peer_mut() = Some(peer);
                self.state().peered.set_peer_koid(peer_koid);
            }

            /// Returns the related KOID of the peer dispatcher.
            pub fn get_related_koid(&self) -> zx_types::zx_koid_t {
                self.state().peered.peer_koid()
            }

            /// Signals this dispatcher endpoint.
            pub fn user_signal_self(
                &self,
                clear_mask: u32,
                set_mask: u32,
            ) -> Result<(), zx_status::Status> {
                if (set_mask & !$allowed_signals) != 0 || (clear_mask & !$allowed_signals) != 0 {
                    return Err(zx_status::Status::INVALID_ARGS);
                }
                ksync::lock!(let guard = self.state().peered.lock());
                self.update_state_locked(guard.token(), clear_mask, set_mask);
                Ok(())
            }

            /// Signals the peer dispatcher endpoint.
            pub fn user_signal_peer(
                &self,
                clear_mask: u32,
                set_mask: u32,
            ) -> Result<(), zx_status::Status> {
                if (set_mask & !$allowed_signals) != 0 || (clear_mask & !$allowed_signals) != 0 {
                    return Err(zx_status::Status::INVALID_ARGS);
                }
                ksync::lock!(let guard = self.state().peered.lock());
                let peer = guard.peer().as_ref().ok_or(zx_status::Status::PEER_CLOSED)?;
                peer.update_state_locked(guard.token(), clear_mask, set_mask);
                Ok(())
            }

            /// Handles zero handles condition by clearing the peer reference and calling
            /// `on_zero_handles_locked` on self and `on_peer_zero_handles_locked` on peer.
            pub fn on_zero_handles(&self) {
                ksync::lock!(let mut guard = self.state().peered.lock());
                let peer = guard.as_mut().peer_mut().take();
                self.on_zero_handles_locked(guard.as_mut().token_mut());

                if let Some(peer) = peer {
                    *peer.state().peered.guard_mu_mut(guard.as_mut().token_mut()).peer_mut() = None;
                    peer.on_peer_zero_handles_locked(guard.as_mut().token_mut());
                }
            }
        }
    };
}
pub(crate) use impl_peered_dispatcher_facade_with_state;

/// Helper macro to generate standard `rust_<type>_state_init` and peered FFI trampolines.
macro_rules! impl_peered_dispatcher_state_init {
    ($type:ident, $state:ident $(, $arg:ident : $arg_ty:ty)* $(,)?) => {
        paste::paste! {
            /// Initializes a `$state` in-place using `$state::init(holder, ...)`.
            ///
            /// # Safety
            ///
            /// `ptr` must point to uninitialized memory of at least `size_of::<$state>()` bytes.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn [<rust_ $type:snake _state_init>](
                ptr: *mut $state,
                holder: fbl::RefPtr<$crate::object::dispatcher::PeerHolder<$type>>,
                $( $arg : $arg_ty ),*
            ) {
                // SAFETY: `ptr` points to uninitialized memory allocated for `$state`.
                unsafe {
                    let _ = pin_init::PinInit::__pinned_init(
                        $state::init(holder, $( $arg ),*),
                        ptr,
                    );
                }
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn [<rust_ $type:snake _get_related_koid>](
                dispatcher: &$type,
            ) -> zx_types::zx_koid_t {
                dispatcher.get_related_koid()
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn [<rust_ $type:snake _user_signal_self>](
                dispatcher: &$type,
                clear_mask: u32,
                set_mask: u32,
            ) -> zx_types::zx_status_t {
                zx_status::Status::result_into_raw(dispatcher.user_signal_self(clear_mask, set_mask))
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn [<rust_ $type:snake _user_signal_peer>](
                dispatcher: &$type,
                clear_mask: u32,
                set_mask: u32,
            ) -> zx_types::zx_status_t {
                zx_status::Status::result_into_raw(dispatcher.user_signal_peer(clear_mask, set_mask))
            }

            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn [<rust_ $type:snake _on_zero_handles>](
                dispatcher: &$type,
            ) {
                dispatcher.on_zero_handles();
            }
        }
    };
}
pub(crate) use impl_peered_dispatcher_state_init;

/// Helper macro to generate standard `rust_<type>_state_init` FFI trampolines.
macro_rules! impl_dispatcher_state_init {
    ($type:ident, $state:ident $(, $arg:ident : $arg_ty:ty)* $(,)?) => {
        paste::paste! {
            /// Initializes a `$state` in-place using `$state::init(dispatcher, ...)`.
            ///
            /// # Safety
            ///
            /// `ptr` must point to uninitialized memory of at least `size_of::<$state>()` bytes,
            /// and `dispatcher` must point to the enclosing `$type`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn [<rust_ $type:snake _state_init>](
                ptr: *mut $state,
                dispatcher: *const $type,
                $( $arg : $arg_ty ),*
            ) {
                // SAFETY: `ptr` points to uninitialized memory allocated for `$state`.
                unsafe {
                    let _ = pin_init::PinInit::__pinned_init(
                        $state::init(dispatcher, $( $arg ),*),
                        ptr,
                    );
                }
            }
        }
    };
}
pub(crate) use impl_dispatcher_state_init;

fbl::impl_opaque_ref_counted_facade!(
    /// Base facade type for kernel Dispatchers.
    pub struct Dispatcher,
    cpp_dispatcher_recycle,
    cpp_dispatcher_get_ref_counted,
);

impl Dispatcher {
    /// Returns the ZX object type of this Dispatcher.
    pub fn get_type(&self) -> zx_types::zx_obj_type_t {
        // SAFETY: self is a valid reference to an initialized Dispatcher.
        unsafe { cpp_dispatcher_get_type(self) }
    }

    /// Returns the kernel object ID (KOID) of this Dispatcher.
    pub fn get_koid(&self) -> zx_types::zx_koid_t {
        // SAFETY: self is a valid reference to an initialized Dispatcher.
        unsafe { super::dispatcher_ffi::cpp_dispatcher_get_koid(self) }
    }

    /// Returns the related koid of this Dispatcher.
    pub fn get_related_koid(&self) -> zx_types::zx_koid_t {
        // SAFETY: self is a valid reference to an initialized Dispatcher.
        unsafe { cpp_dispatcher_get_related_koid(self) }
    }

    /// Adds an observer to this dispatcher.
    ///
    /// # Safety
    ///
    /// `observer` must point to a valid `SignalObserver`. `handle` must be a valid handle pointer.
    pub unsafe fn add_observer(
        &self,
        observer: *mut core::ffi::c_void,
        handle: *const core::ffi::c_void,
        signals: zx_types::zx_signals_t,
    ) -> Result<(), Status> {
        let status = unsafe { cpp_dispatcher_add_observer(self, observer, handle, signals) };
        Status::ok(status)
    }

    /// Removes an observer from this dispatcher.
    ///
    /// # Safety
    ///
    /// `observer` must point to a valid `SignalObserver`.
    pub unsafe fn remove_observer(
        &self,
        observer: *mut core::ffi::c_void,
        out_signals: &mut zx_types::zx_signals_t,
    ) -> bool {
        unsafe { cpp_dispatcher_remove_observer(self, observer, out_signals) }
    }

    /// Safely downcasts a `&Dispatcher` reference to a specific facade reference `&T` if the
    /// dispatcher types match.
    pub fn downcast<T: DispatcherOps>(&self) -> Option<&T> {
        if T::TYPE == zx_types::ZX_OBJ_TYPE_NONE || self.get_type() == T::TYPE {
            // SAFETY: `T` implements `DispatcherOps` and its `TYPE` matches `self.get_type()`.
            // All facade types (`ThreadDispatcher`, `ProcessDispatcher`, etc.) are `#[repr(C)]`
            // layout-compatible with `Dispatcher`.
            unsafe { Some(&*(self as *const Self as *const T)) }
        } else {
            None
        }
    }

    /// Gets the name of the dispatcher.
    pub fn get_name(&self, out_name: &mut [u8; zx_types::ZX_MAX_NAME_LEN]) -> Result<(), Status> {
        // SAFETY: self is a valid Dispatcher and out_name points to a valid buffer of ZX_MAX_NAME_LEN bytes.
        let status = unsafe { cpp_dispatcher_get_name(self, out_name.as_mut_ptr().cast()) };
        Status::ok(status)
    }

    /// Sets the name of the dispatcher.
    pub fn set_name(&self, name: &[u8]) -> Result<(), Status> {
        // SAFETY: self is a valid Dispatcher and name points to name.len() bytes of valid memory.
        let status = unsafe { cpp_dispatcher_set_name(self, name.as_ptr().cast(), name.len()) };
        Status::ok(status)
    }

    /// Returns the current number of handles pointing to this dispatcher.
    pub fn current_handle_count(&self) -> u32 {
        // SAFETY: self is a valid Dispatcher reference.
        unsafe { cpp_dispatcher_current_handle_count(self) }
    }

    /// Returns the basic handle information for this dispatcher.
    pub fn get_handle_info(&self, rights: zx_rights_t) -> zx_types::zx_info_handle_basic_t {
        zx_types::zx_info_handle_basic_t {
            koid: self.get_koid(),
            rights,
            type_: self.get_type(),
            related_koid: self.get_related_koid(),
            ..Default::default()
        }
    }

    /// Resolves a handle to a dispatcher of type T without requiring any rights.
    ///
    /// # Errors
    ///
    /// - `ZX_ERR_BAD_HANDLE` if `handle` is not valid.
    /// - `ZX_ERR_WRONG_TYPE` if the dispatcher's type does not match `T::TYPE`.
    pub fn get<T>(handle: HandleValue) -> Result<RefPtr<T>, Status>
    where
        T: DispatcherOps + fbl::HasRefCount + fbl::Recyclable,
    {
        Self::get_with_rights::<T>(handle, zx_types::ZX_RIGHT_NONE)
    }

    /// Resolves a handle to a dispatcher of type T with required rights.
    ///
    /// # Errors
    ///
    /// - `ZX_ERR_BAD_HANDLE` if `handle` is not valid.
    /// - `ZX_ERR_WRONG_TYPE` if the dispatcher's type does not match `T::TYPE`.
    /// - `ZX_ERR_ACCESS_DENIED` if `handle` lacks the requested `rights`.
    pub fn get_with_rights<T>(handle: HandleValue, rights: zx_rights_t) -> Result<RefPtr<T>, Status>
    where
        T: DispatcherOps + fbl::HasRefCount + fbl::Recyclable,
    {
        let (dispatcher, _actual_rights) = Self::get_with_rights_and_actual::<T>(handle, rights)?;
        Ok(dispatcher)
    }

    /// Resolves a handle to a dispatcher of type T with required rights and returns its actual rights.
    ///
    /// # Errors
    ///
    /// - `ZX_ERR_BAD_HANDLE` if `handle` is not valid.
    /// - `ZX_ERR_WRONG_TYPE` if the dispatcher's type does not match `T::TYPE`.
    /// - `ZX_ERR_ACCESS_DENIED` if `handle` lacks the requested `rights`.
    pub fn get_with_rights_and_actual<T>(
        handle: HandleValue,
        rights: zx_rights_t,
    ) -> Result<(RefPtr<T>, zx_rights_t), Status>
    where
        T: DispatcherOps + fbl::HasRefCount + fbl::Recyclable,
    {
        let (dispatcher, actual_rights) = Self::get_and_rights::<T>(handle)?;
        if (actual_rights & rights) != rights {
            return Err(Status::ACCESS_DENIED);
        }
        Ok((dispatcher, actual_rights))
    }

    /// Resolves a handle to a dispatcher of type T and returns its associated rights.
    ///
    /// # Errors
    ///
    /// - `ZX_ERR_BAD_HANDLE` if `handle` is not valid.
    /// - `ZX_ERR_WRONG_TYPE` if the dispatcher's type does not match `T::TYPE`.
    pub fn get_and_rights<T>(handle: HandleValue) -> Result<(RefPtr<T>, zx_rights_t), Status>
    where
        T: DispatcherOps + fbl::HasRefCount + fbl::Recyclable,
    {
        let (dispatcher, actual_rights) = Self::get_dispatcher_and_rights(handle)?;
        if T::TYPE != zx_types::ZX_OBJ_TYPE_NONE && dispatcher.get_type() != T::TYPE {
            return Err(Status::WRONG_TYPE);
        }
        // SAFETY: We verified the type of the dispatcher, so it is safe to cast.
        unsafe { Ok((dispatcher.cast::<T>(), actual_rights)) }
    }

    /// Resolves a handle to a dispatcher and returns its associated rights.
    pub fn get_dispatcher_and_rights(
        handle: HandleValue,
    ) -> Result<(fbl::RefPtr<Dispatcher>, zx_rights_t), Status> {
        let mut ref_ptr = MaybeUninit::<fbl::RefPtr<Dispatcher>>::uninit();
        let mut actual_rights = MaybeUninit::<zx_rights_t>::uninit();
        // SAFETY: ref_ptr and actual_rights point to valid, writable uninitialized memory.
        unsafe {
            let status = cpp_handle_table_get_dispatcher(
                handle,
                ref_ptr.as_mut_ptr(),
                actual_rights.as_mut_ptr(),
            );
            Status::ok(status)?;
            Ok((ref_ptr.assume_init(), actual_rights.assume_init()))
        }
    }
}

impl DispatcherOps for Dispatcher {
    type LockClass = ();
    const TYPE: zx_types::zx_obj_type_t = zx_types::ZX_OBJ_TYPE_NONE;

    fn dispatcher(&self) -> *const Dispatcher {
        self
    }
}

/// Peered dispatchers have opposing endpoints to coordinate state with. For example, writing into
/// one endpoint of a Channel needs to modify `zx_signals_t` state (for the readability bit) on the
/// opposite side. To coordinate their state, they share a mutex, which is held by the
/// `PeerHolder`. Both endpoints have a `RefPtr` back to the `PeerHolder`; no one else ever does.
#[guarded]
#[ref_counted]
#[derive(Recyclable)]
#[repr(C)]
pub struct PeerHolder<Endpoint> {
    #[mutex]
    pub mu: KMutex<RawCriticalMutex>,
    _phantom: PhantomData<Endpoint>,
}

impl<Endpoint> PeerHolder<Endpoint> {
    /// Creates a new `PeerHolder`.
    pub fn create() -> Result<RefPtr<Self>, AllocError> {
        pin_make_ref_counted!(Self {
            mu <- KMutex::init(),
            _phantom: PhantomData,
        })
    }
}

/// State shared by peered dispatcher subtypes.
///
/// Peered dispatchers have opposing endpoints to coordinate state with (such as peer KOID, the peer
/// reference, and the shared `PeerHolder` mutex).
#[guarded]
#[repr(C)]
pub struct PeeredState<T: fbl::IsOpaqueRefCounted> {
    peer_koid: RelaxedAtomicU64,
    pub holder: RefPtr<PeerHolder<T>>,
    #[guarded_by(mu)]
    pub peer: Option<RefPtr<T>>,
    #[mutex(PeerHolderMuClass<T>)]
    pub mu: KMutex<ksync::PhantomMutex>,
}

impl<T: fbl::IsOpaqueRefCounted> PeeredState<T> {
    /// Initializes a `PeeredState` with the given `PeerHolder`.
    pub fn init(
        holder: RefPtr<PeerHolder<T>>,
    ) -> impl pin_init::PinInit<Self, core::convert::Infallible> {
        pin_init::pin_init!(Self {
            peer_koid: RelaxedAtomicU64::new(0),
            holder,
            peer: ksync::KCell::new(None),
            mu: KMutex::new(ksync::PhantomMutex),
        })
    }

    /// Returns the peer KOID as `zx_koid_t`, or `0` if not yet initialized.
    #[inline]
    pub fn peer_koid(&self) -> zx_types::zx_koid_t {
        self.peer_koid.load()
    }

    /// Returns the peer KOID as `Option<NonZero<zx_koid_t>>`, or `None` if not yet initialized.
    #[inline]
    pub fn peer_koid_non_zero(&self) -> Option<core::num::NonZero<zx_types::zx_koid_t>> {
        core::num::NonZero::new(self.peer_koid.load())
    }

    /// Sets the peer KOID.
    #[inline]
    pub fn set_peer_koid(&self, koid: zx_types::zx_koid_t) {
        self.peer_koid.store(koid);
    }

    /// Locks the underlying shared `PeerHolder` mutex (`self.holder.mu`) and returns a guard for
    /// accessing fields protected by `self.mu`.
    #[inline]
    pub fn lock(
        &self,
    ) -> impl pin_init::PinInit<PeeredStateMuGuard<'_, T, RawCriticalMutex>, core::convert::Infallible>
    {
        self.lock_mu(&self.holder.mu)
    }
}
