// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Synchronized reference counting primitives.
//!
//! This module introduces a family of reference counted types that allows
//! marking the underlying data for destruction before all strongly references
//! to the data are dropped. This enables the following features:
//!   * Upgrading a weak reference to a strong reference succeeds iff at least
//!     one strong reference exists _and_ the data has not been marked for
//!     destruction.
//!   * Allow waiting for all strongly-held references to be dropped after
//!     marking the data.

use core::fmt::Debug;
use core::hash::{Hash, Hasher};
use core::ops::Deref;
use core::panic::Location;
use core::sync::atomic::{AtomicBool, Ordering};

use derivative::Derivative;

mod caller {
    //! Provides tracking of instances via tracked caller location.
    //!
    //! Callers are only tracked in debug builds. All operations and types
    //! are no-ops and empty unless the `rc-debug-names` feature is enabled.

    use core::fmt::Debug;
    use core::panic::Location;

    /// Records reference-counted names of instances.
    #[derive(Default)]
    pub(super) struct Callers {
        /// The names that were inserted and aren't known to be gone.
        ///
        /// This holds weak references to allow callers to drop without
        /// synchronizing. Invalid weak pointers are cleaned up periodically but
        /// are not logically present.
        ///
        /// Note that using [`std::sync::Mutex`] here is intentional to opt this
        /// out of loom checking, which makes testing with `rc-debug-names`
        /// impossibly slow.
        #[cfg(feature = "rc-debug-names")]
        pub(super) callers: std::sync::Mutex<std::collections::HashMap<Location<'static>, usize>>,
    }

    impl Debug for Callers {
        #[cfg(not(feature = "rc-debug-names"))]
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "(Not Tracked)")
        }
        #[cfg(feature = "rc-debug-names")]
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            let Self { callers } = self;
            let callers = callers.lock().unwrap();
            write!(f, "[\n")?;
            for (l, c) in callers.iter() {
                write!(f, "   {l} => {c},\n")?;
            }
            write!(f, "]")
        }
    }

    impl Callers {
        /// Creates a new [`Callers`] from the given [`Location`].
        ///
        /// On non-debug builds, this is a no-op.
        pub(super) fn insert(&self, caller: &Location<'static>) -> TrackedCaller {
            #[cfg(not(feature = "rc-debug-names"))]
            {
                let _ = caller;
                TrackedCaller {}
            }
            #[cfg(feature = "rc-debug-names")]
            {
                let Self { callers } = self;
                let mut callers = callers.lock().unwrap();
                let count = callers.entry(caller.clone()).or_insert(0);
                *count += 1;
                TrackedCaller { location: caller.clone() }
            }
        }
    }

    #[derive(Debug)]
    pub(super) struct TrackedCaller {
        #[cfg(feature = "rc-debug-names")]
        pub(super) location: Location<'static>,
    }

    impl TrackedCaller {
        #[cfg(not(feature = "rc-debug-names"))]
        pub(super) fn release(&mut self, Callers {}: &Callers) {
            let Self {} = self;
        }

        #[cfg(feature = "rc-debug-names")]
        pub(super) fn release(&mut self, Callers { callers }: &Callers) {
            let Self { location } = self;
            let mut callers = callers.lock().unwrap();
            let mut entry = match callers.entry(location.clone()) {
                std::collections::hash_map::Entry::Vacant(_) => {
                    panic!("location {location:?} was not in the callers map")
                }
                std::collections::hash_map::Entry::Occupied(o) => o,
            };

            let sub = entry
                .get()
                .checked_sub(1)
                .unwrap_or_else(|| panic!("zero-count location {location:?} in map"));
            if sub == 0 {
                let _: usize = entry.remove();
            } else {
                *entry.get_mut() = sub;
            }
        }
    }
}

mod resource_token {
    use core::fmt::Debug;
    use core::sync::atomic::{AtomicU64, Ordering};
    use std::marker::PhantomData;
    use std::num::NonZeroU64;

    /// An opaque token associated with a resource.
    ///
    /// It can be used to create debug and trace identifiers for the resource,
    /// but it should not be used as a unique identifier of the resource inside
    /// the netstack.
    ///
    /// By default the lifetime of a token is bound the resource that token
    /// belongs to, but it can be extended by calling
    /// [`ResourceToken::extend_lifetime`].
    ///
    /// Internally the value is stored as `NonZeroU64`. This is an optimization
    /// to save memory when the value is wrapped in `Option` (it allows the
    /// compiler to fit `Option<ResourceToken>` in 8 bytes).
    #[cfg_attr(any(test, feature = "testutils"), derive(PartialEq, Eq, PartialOrd, Ord))]
    #[derive(Clone)]
    pub struct ResourceToken<'a> {
        value: NonZeroU64,
        _marker: PhantomData<&'a ()>,
    }

    impl<'a> ResourceToken<'a> {
        /// Extends lifetime of the token.
        ///
        /// # Discussion
        ///
        /// It's generally okay to extend the lifetime of the token, but prefer
        /// to use tokens bound to the resource's lifetime whenever possible,
        /// since it provides guardrails against identifiers that outlive the
        /// resource itself.
        pub fn extend_lifetime(self) -> ResourceToken<'static> {
            ResourceToken { value: self.value, _marker: PhantomData }
        }

        /// Returns internal value. Consumes `self`.
        ///
        /// # Discussion
        ///
        /// Export to `u64` when a representation is needed for interaction with
        /// other processes or components such as trace identifiers and eBPF
        /// socket cookies.
        ///
        /// Refrain from using the returned value within the netstack otherwise.
        pub fn export_value(self) -> u64 {
            self.value.get()
        }
    }

    impl<'a> Debug for ResourceToken<'a> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
            write!(f, "{}", self.value)
        }
    }

    /// Holder of a value for `ResourceToken`. Vends `ResourceToken` instances
    /// with the same value and the lifetime bound to the lifetime of the holder.
    ///
    /// The [`Default`] implementation generates a new unique value.
    pub struct ResourceTokenValue(NonZeroU64);

    impl ResourceTokenValue {
        /// Creates a new token.
        pub fn token(&self) -> ResourceToken<'_> {
            let ResourceTokenValue(value) = self;
            ResourceToken { value: *value, _marker: PhantomData }
        }
    }

    impl core::fmt::Debug for ResourceTokenValue {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
            let ResourceTokenValue(value) = self;
            write!(f, "{}", value)
        }
    }

    impl Default for ResourceTokenValue {
        fn default() -> Self {
            static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
            // NB: Fetch add will cause the counter to rollback to 0 if we
            // happen to exceed `u64::MAX` instantiations. In practice, that's
            // an impossibility (at 1 billion instantiations per second, the
            // counter is valid for > 500 years). Spare the CPU cycles and don't
            // bother attempting to handle overflow.
            Self(NonZeroU64::new(NEXT_TOKEN.fetch_add(1, Ordering::Relaxed)).unwrap())
        }
    }
}

pub use resource_token::{ResourceToken, ResourceTokenValue};

mod debug_id {
    use super::ResourceToken;
    use core::fmt::Debug;

    /// A debug identifier for the RC types exposed in the parent module.
    ///
    /// Encompasses the underlying pointer for the RC type, as well as
    /// (optionally) the globally unique [`ResourceToken`].
    pub(super) enum DebugId<T> {
        /// Used in contexts that have access to the [`ResourceToken`], e.g.
        /// [`Primary`], [`Strong`], and sometimes [`Weak`] RC types.
        WithToken { ptr: *const T, token: ResourceToken<'static> },
        /// Used in contexts that don't have access to the [`ResourceToken`], e.g.
        /// [`Weak`] RC types that cannot be upgraded.
        WithoutToken { ptr: *const T },
    }

    impl<T> Debug for DebugId<T> {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
            match self {
                DebugId::WithToken { ptr, token } => write!(f, "{:?}:{:?}", token, ptr),
                DebugId::WithoutToken { ptr } => write!(f, "?:{:?}", ptr),
            }
        }
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
struct Inner<T> {
    marked_for_destruction: AtomicBool,
    callers: caller::Callers,
    data: core::mem::ManuallyDrop<T>,
    // NB: Notifier could be an atomic pointer or atomic box but this mutex is
    // never contended and we don't have to import new code into the repository
    // (i.e. atomicbox) or write unsafe code.
    #[derivative(Debug = "ignore")]
    notifier: crate::Mutex<Option<Box<dyn Notifier<T>>>>,
    resource_token: ResourceTokenValue,
}

impl<T> Inner<T> {
    fn pre_drop_check(marked_for_destruction: &AtomicBool) {
        // `Ordering::Acquire` because we want to synchronize with with the
        // `Ordering::Release` write to `marked_for_destruction` so that all
        // memory writes before the reference was marked for destruction is
        // visible here.
        assert!(marked_for_destruction.load(Ordering::Acquire), "Must be marked for destruction");
    }

    fn unwrap(mut self) -> T {
        // We cannot destructure `self` by value since `Inner` implements
        // `Drop`. So we must manually drop all the fields but data and then
        // forget self.
        let Inner { marked_for_destruction, data, callers: holders, notifier, resource_token } =
            &mut self;

        // Make sure that `inner` is in a valid state for destruction.
        //
        // Note that we do not actually destroy all of `self` here; we decompose
        // it into its parts, keeping what we need & throwing away what we
        // don't. Regardless, we perform the same checks.
        Inner::<T>::pre_drop_check(marked_for_destruction);

        // SAFETY: Safe since we own `self` and `self` is immediately forgotten
        // below so the its destructor (and those of its fields) will not be run
        // as a result of `self` being dropped.
        let data = unsafe {
            // Explicitly drop since we do not need these anymore.
            core::ptr::drop_in_place(marked_for_destruction);
            core::ptr::drop_in_place(holders);
            core::ptr::drop_in_place(notifier);
            core::ptr::drop_in_place(resource_token);

            core::mem::ManuallyDrop::take(data)
        };
        // Forget self now to prevent its `Drop::drop` impl from being run which
        // will attempt to destroy `data` but still perform pre-drop checks on
        // `Inner`'s state.
        core::mem::forget(self);

        data
    }

    /// Sets the notifier for this `Inner`.
    ///
    /// Panics if notifier is already set.
    fn set_notifier<N: Notifier<T> + 'static>(&self, notifier: N) {
        let Self { notifier: slot, .. } = self;

        // Using dynamic dispatch to notify allows us to not have to know the
        // notifier that will be used from creation and spread the type on all
        // reference types in this crate. The assumption is that the allocation
        // and dynamic dispatch costs here are tiny compared to the overall work
        // of destroying the resources this module is targeting.
        let boxed: Box<dyn Notifier<T>> = Box::new(notifier);
        let prev_notifier = { slot.lock().replace(boxed) };
        // Uphold invariant that this can only be done from Primary.
        assert!(prev_notifier.is_none(), "can't have a notifier already installed");
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        let Inner { marked_for_destruction, data, callers: _, notifier, resource_token: _ } = self;
        // Take data out of ManuallyDrop in case we panic in pre_drop_check.
        // That'll ensure data is dropped if we hit the panic.
        //
        //  SAFETY: Safe because ManuallyDrop is not referenced again after
        // taking.
        let data = unsafe { core::mem::ManuallyDrop::take(data) };
        Self::pre_drop_check(marked_for_destruction);
        if let Some(mut notifier) = notifier.lock().take() {
            notifier.notify(data);
        }
    }
}

/// A primary reference.
///
/// Note that only one `Primary` may be associated with data. This is
/// enforced by not implementing [`Clone`].
///
/// For now, this reference is no different than a [`Strong`] but later changes
/// will enable blocking the destruction of a primary reference until all
/// strongly held references are dropped.
#[derive(Debug)]
pub struct Primary<T> {
    inner: core::mem::ManuallyDrop<alloc::sync::Arc<Inner<T>>>,
}

impl<T> Drop for Primary<T> {
    fn drop(&mut self) {
        let was_marked = self.mark_for_destruction();
        let Self { inner } = self;
        // Take the inner out of ManuallyDrop early so its Drop impl will run in
        // case we panic here.
        // SAFETY: Safe because we don't reference ManuallyDrop again.
        let inner = unsafe { core::mem::ManuallyDrop::take(inner) };

        // Make debugging easier: don't panic if a panic is already happening
        // since double-panics are annoying to debug. This means that the
        // invariants provided by Primary are possibly violated during an
        // unwind, but we're sidestepping that problem because Fuchsia is our
        // only audience here.
        if !std::thread::panicking() {
            assert_eq!(was_marked, false, "Must not be marked for destruction yet");

            let Inner {
                marked_for_destruction: _,
                callers,
                data: _,
                notifier: _,
                resource_token: _,
            } = &*inner;

            // Make sure that this `Primary` is the last thing to hold a strong
            // reference to the underlying data when it is being dropped.
            let refs = alloc::sync::Arc::strong_count(&inner).checked_sub(1).unwrap();
            assert!(
                refs == 0,
                "dropped Primary with {refs} strong refs remaining, \
                            Callers={callers:?}"
            );
        }
    }
}

impl<T> AsRef<T> for Primary<T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

impl<T> Deref for Primary<T> {
    type Target = T;

    fn deref(&self) -> &T {
        let Self { inner } = self;
        let Inner { marked_for_destruction: _, data, callers: _, notifier: _, resource_token: _ } =
            &***inner;
        data
    }
}

impl<T> Primary<T> {
    // Marks this primary reference as ready for destruction. Used by all
    // dropping flows. We take &mut self here to ensure we have the only
    // possible reference to Primary. Returns whether it was already marked for
    // destruction.
    fn mark_for_destruction(&mut self) -> bool {
        let Self { inner } = self;
        // `Ordering::Release` because want to make sure that all memory writes
        // before dropping this `Primary` synchronizes with later attempts to
        // upgrade weak pointers and the `Drop::drop` impl of `Inner`.
        inner.marked_for_destruction.swap(true, Ordering::Release)
    }

    /// Returns a new strongly-held reference.
    pub fn new(data: T) -> Primary<T> {
        Primary {
            inner: core::mem::ManuallyDrop::new(alloc::sync::Arc::new(Inner {
                marked_for_destruction: AtomicBool::new(false),
                callers: caller::Callers::default(),
                data: core::mem::ManuallyDrop::new(data),
                notifier: crate::Mutex::new(None),
                resource_token: ResourceTokenValue::default(),
            })),
        }
    }

    /// Constructs a new `Primary<T>` while giving you a Weak<T> to the
    /// allocation, to allow you to construct a `T` which holds a weak pointer
    /// to itself.
    ///
    /// Like for [`Arc::new_cyclic`], the `Weak` reference provided to `data_fn`
    /// cannot be upgraded until the [`Primary`] is constructed.
    pub fn new_cyclic(data_fn: impl FnOnce(Weak<T>) -> T) -> Primary<T> {
        Primary {
            inner: core::mem::ManuallyDrop::new(alloc::sync::Arc::new_cyclic(move |weak| Inner {
                marked_for_destruction: AtomicBool::new(false),
                callers: caller::Callers::default(),
                data: core::mem::ManuallyDrop::new(data_fn(Weak(weak.clone()))),
                notifier: crate::Mutex::new(None),
                resource_token: ResourceTokenValue::default(),
            })),
        }
    }

    /// Clones a strongly-held reference.
    #[cfg_attr(feature = "rc-debug-names", track_caller)]
    pub fn clone_strong(Self { inner }: &Self) -> Strong<T> {
        let Inner { data: _, callers, marked_for_destruction: _, notifier: _, resource_token: _ } =
            &***inner;
        let caller = callers.insert(Location::caller());
        Strong { inner: alloc::sync::Arc::clone(inner), caller }
    }

    /// Returns a weak reference pointing to the same underlying data.
    pub fn downgrade(Self { inner }: &Self) -> Weak<T> {
        Weak(alloc::sync::Arc::downgrade(inner))
    }

    /// Returns true if the two pointers point to the same allocation.
    pub fn ptr_eq(
        Self { inner: this }: &Self,
        Strong { inner: other, caller: _ }: &Strong<T>,
    ) -> bool {
        alloc::sync::Arc::ptr_eq(this, other)
    }

    /// Returns [`Debug`] implementation that is stable and unique
    /// for the data held behind this [`Primary`].
    pub fn debug_id(&self) -> impl Debug + '_ {
        let Self { inner } = self;

        // The lifetime of the returned `DebugId` is bound to the lifetime
        // of `self`.
        let token = inner.resource_token.token().extend_lifetime();

        debug_id::DebugId::WithToken { ptr: alloc::sync::Arc::as_ptr(inner), token }
    }

    fn mark_for_destruction_and_take_inner(mut this: Self) -> alloc::sync::Arc<Inner<T>> {
        // Prepare for destruction.
        assert!(!this.mark_for_destruction());
        let Self { inner } = &mut this;
        // SAFETY: Safe because inner can't be used after this. We forget
        // our Primary reference to prevent its Drop impl from running.
        let inner = unsafe { core::mem::ManuallyDrop::take(inner) };
        core::mem::forget(this);
        inner
    }

    fn try_unwrap(this: Self) -> Result<T, alloc::sync::Arc<Inner<T>>> {
        let inner = Self::mark_for_destruction_and_take_inner(this);
        alloc::sync::Arc::try_unwrap(inner).map(Inner::unwrap)
    }

    /// Returns the inner value if no [`Strong`] references are held.
    ///
    /// # Panics
    ///
    /// Panics if [`Strong`] references are held when this function is called.
    pub fn unwrap(this: Self) -> T {
        Self::try_unwrap(this).unwrap_or_else(|inner| {
            let callers = &inner.callers;
            let refs = alloc::sync::Arc::strong_count(&inner).checked_sub(1).unwrap();
            panic!("can't unwrap, still had {refs} strong refs: {callers:?}");
        })
    }

    /// Marks this [`Primary`] for destruction and uses `notifier` as a signaler
    /// for when destruction of all strong references is terminated. After
    /// calling `unwrap_with_notifier` [`Weak`] references can no longer be
    /// upgraded.
    pub fn unwrap_with_notifier<N: Notifier<T> + 'static>(this: Self, notifier: N) {
        let inner = Self::mark_for_destruction_and_take_inner(this);
        inner.set_notifier(notifier);
        // Now we can drop our inner reference, if we were the last this will
        // trigger the notifier.
        core::mem::drop(inner);
    }

    /// Marks this [`Primary`] for destruction and returns `Ok` if this was the
    /// last strong reference standing for it. Otherwise `new_notifier` is
    /// called to create a new notifier to observe deferred destruction.
    ///
    /// Like [`Primary::unwrap_with_notifier`], [`Weak`] references can no
    /// longer be upgraded after calling `unwrap_or_notify_with`.
    pub fn unwrap_or_notify_with<N: Notifier<T> + 'static, O, F: FnOnce() -> (N, O)>(
        this: Self,
        new_notifier: F,
    ) -> Result<T, O> {
        Self::try_unwrap(this).map_err(move |inner| {
            let (notifier, output) = new_notifier();
            inner.set_notifier(notifier);
            output
        })
    }

    /// Creates a [`DebugReferences`] instance.
    pub fn debug_references(this: &Self) -> DebugReferences<T> {
        let Self { inner } = this;
        DebugReferences(alloc::sync::Arc::downgrade(&*inner))
    }
}

/// A strongly-held reference.
///
/// Similar to an [`alloc::sync::Arc`] but holding a `Strong` acts as a witness
/// to the live-ness of the underlying data. That is, holding a `Strong` implies
/// that the underlying data has not yet been destroyed.
///
/// Note that `Strong`'s implementation of [`Hash`] and [`PartialEq`] operate on
/// the pointer itself and not the underlying data.
#[derive(Debug, Derivative)]
pub struct Strong<T> {
    inner: alloc::sync::Arc<Inner<T>>,
    caller: caller::TrackedCaller,
}

impl<T> Drop for Strong<T> {
    fn drop(&mut self) {
        let Self { inner, caller } = self;
        let Inner { marked_for_destruction: _, callers, data: _, notifier: _, resource_token: _ } =
            &**inner;
        caller.release(callers);
    }
}

impl<T> AsRef<T> for Strong<T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

impl<T> Deref for Strong<T> {
    type Target = T;

    fn deref(&self) -> &T {
        let Self { inner, caller: _ } = self;
        let Inner { marked_for_destruction: _, data, callers: _, notifier: _, resource_token: _ } =
            inner.deref();
        data
    }
}

impl<T> core::cmp::Eq for Strong<T> {}

impl<T> core::cmp::PartialEq for Strong<T> {
    fn eq(&self, other: &Self) -> bool {
        Self::ptr_eq(self, other)
    }
}

impl<T> Hash for Strong<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let Self { inner, caller: _ } = self;
        alloc::sync::Arc::as_ptr(inner).hash(state)
    }
}

impl<T> Clone for Strong<T> {
    #[cfg_attr(feature = "rc-debug-names", track_caller)]
    fn clone(&self) -> Self {
        let Self { inner, caller: _ } = self;
        let Inner { data: _, marked_for_destruction: _, callers, notifier: _, resource_token: _ } =
            &**inner;
        let caller = callers.insert(Location::caller());
        Self { inner: alloc::sync::Arc::clone(inner), caller }
    }
}

impl<T> Strong<T> {
    /// Returns a weak reference pointing to the same underlying data.
    pub fn downgrade(Self { inner, caller: _ }: &Self) -> Weak<T> {
        Weak(alloc::sync::Arc::downgrade(inner))
    }

    /// Returns [`Debug`] implementation that is stable and unique
    /// for the data held behind this [`Strong`].
    pub fn debug_id(&self) -> impl Debug + '_ {
        let Self { inner, caller: _ } = self;

        // The lifetime of the returned `DebugId` is bound to the lifetime
        // of `self`.
        let token = inner.resource_token.token().extend_lifetime();

        debug_id::DebugId::WithToken { ptr: alloc::sync::Arc::as_ptr(inner), token }
    }

    /// Returns a [`ResourceToken`] that corresponds to this object.
    pub fn resource_token(&self) -> ResourceToken<'_> {
        self.inner.resource_token.token()
    }

    /// Returns true if the inner value has since been marked for destruction.
    pub fn marked_for_destruction(Self { inner, caller: _ }: &Self) -> bool {
        let Inner { marked_for_destruction, data: _, callers: _, notifier: _, resource_token: _ } =
            inner.as_ref();
        // `Ordering::Acquire` because we want to synchronize with with the
        // `Ordering::Release` write to `marked_for_destruction` so that all
        // memory writes before the reference was marked for destruction is
        // visible here.
        marked_for_destruction.load(Ordering::Acquire)
    }

    /// Returns true if the two pointers point to the same allocation.
    pub fn weak_ptr_eq(Self { inner: this, caller: _ }: &Self, Weak(other): &Weak<T>) -> bool {
        core::ptr::eq(alloc::sync::Arc::as_ptr(this), other.as_ptr())
    }

    /// Returns true if the two pointers point to the same allocation.
    pub fn ptr_eq(
        Self { inner: this, caller: _ }: &Self,
        Self { inner: other, caller: _ }: &Self,
    ) -> bool {
        alloc::sync::Arc::ptr_eq(this, other)
    }

    /// Compares the two pointers.
    pub fn ptr_cmp(
        Self { inner: this, caller: _ }: &Self,
        Self { inner: other, caller: _ }: &Self,
    ) -> core::cmp::Ordering {
        let this = alloc::sync::Arc::as_ptr(this);
        let other = alloc::sync::Arc::as_ptr(other);
        this.cmp(&other)
    }

    /// Creates a [`DebugReferences`] instance.
    pub fn debug_references(this: &Self) -> DebugReferences<T> {
        let Self { inner, caller: _ } = this;
        DebugReferences(alloc::sync::Arc::downgrade(inner))
    }
}

/// A weakly-held reference.
///
/// Similar to an [`alloc::sync::Weak`].
///
/// A `Weak` does not make any claim to the live-ness of the underlying data.
/// Holders of a `Weak` must attempt to upgrade to a [`Strong`] through
/// [`Weak::upgrade`] to access the underlying data.
///
/// Note that `Weak`'s implementation of [`Hash`] and [`PartialEq`] operate on
/// the pointer itself and not the underlying data.
#[derive(Debug)]
pub struct Weak<T>(alloc::sync::Weak<Inner<T>>);

impl<T> core::cmp::Eq for Weak<T> {}

impl<T> core::cmp::PartialEq for Weak<T> {
    fn eq(&self, other: &Self) -> bool {
        Self::ptr_eq(self, other)
    }
}

impl<T> Hash for Weak<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let Self(this) = self;
        this.as_ptr().hash(state)
    }
}

impl<T> Clone for Weak<T> {
    fn clone(&self) -> Self {
        let Self(this) = self;
        Weak(this.clone())
    }
}

impl<T> Weak<T> {
    /// Returns true if the two pointers point to the same allocation.
    pub fn ptr_eq(&self, Self(other): &Self) -> bool {
        let Self(this) = self;
        this.ptr_eq(other)
    }

    /// Returns [`Debug`] implementation that is stable and unique
    /// for the data held behind this [`Weak`].
    pub fn debug_id(&self) -> impl Debug + '_ {
        match self.upgrade() {
            Some(strong) => {
                let Strong { inner, caller: _ } = &strong;

                // The lifetime of the returned `DebugId` is still bound to the
                // lifetime of `self`.
                let token = inner.resource_token.token().extend_lifetime();

                debug_id::DebugId::WithToken { ptr: alloc::sync::Arc::as_ptr(&inner), token }
            }
            None => {
                let Self(this) = self;
                // NB: If we can't upgrade the socket, we can't know the token.
                debug_id::DebugId::WithoutToken { ptr: this.as_ptr() }
            }
        }
    }

    /// Attempts to upgrade to a [`Strong`].
    ///
    /// Returns `None` if the inner value has since been marked for destruction.
    #[cfg_attr(feature = "rc-debug-names", track_caller)]
    pub fn upgrade(&self) -> Option<Strong<T>> {
        let Self(weak) = self;
        let arc = weak.upgrade()?;
        let Inner { marked_for_destruction, data: _, callers, notifier: _, resource_token: _ } =
            arc.deref();

        // `Ordering::Acquire` because we want to synchronize with with the
        // `Ordering::Release` write to `marked_for_destruction` so that all
        // memory writes before the reference was marked for destruction is
        // visible here.
        if !marked_for_destruction.load(Ordering::Acquire) {
            let caller = callers.insert(Location::caller());
            Some(Strong { inner: arc, caller })
        } else {
            None
        }
    }

    /// Gets the number of [`Primary`] and [`Strong`] references to this allocation.
    pub fn strong_count(&self) -> usize {
        let Self(weak) = self;
        weak.strong_count()
    }

    /// Creates a [`DebugReferences`] instance.
    pub fn debug_references(&self) -> DebugReferences<T> {
        let Self(inner) = self;
        DebugReferences(inner.clone())
    }
}

fn debug_refs(
    refs: Option<(usize, &AtomicBool, &caller::Callers)>,
    name: &'static str,
    f: &mut core::fmt::Formatter<'_>,
) -> core::fmt::Result {
    let mut f = f.debug_struct(name);
    match refs {
        Some((strong_count, marked_for_destruction, callers)) => f
            .field("strong_count", &strong_count)
            .field("marked_for_destruction", marked_for_destruction)
            .field("callers", callers)
            .finish(),
        None => {
            let strong_count = 0_usize;
            f.field("strong_count", &strong_count).finish_non_exhaustive()
        }
    }
}

/// Provides a [`Debug`] implementation that contains information helpful for
/// debugging dangling references.
#[derive(Clone)]
pub struct DebugReferences<T>(alloc::sync::Weak<Inner<T>>);

impl<T> Debug for DebugReferences<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(inner) = self;
        let inner = inner.upgrade();
        let refs = inner.as_ref().map(|inner| {
            (alloc::sync::Arc::strong_count(inner), &inner.marked_for_destruction, &inner.callers)
        });
        debug_refs(refs, "DebugReferences", f)
    }
}

impl<T: Send + Sync + 'static> DebugReferences<T> {
    /// Transforms this `DebugReferences` into a [`DynDebugReferences`].
    pub fn into_dyn(self) -> DynDebugReferences {
        let Self(w) = self;
        DynDebugReferences(w)
    }
}

/// Like [`DebugReferences`], but type-erases the contained type.
#[derive(Clone)]
pub struct DynDebugReferences(alloc::sync::Weak<dyn ExposeRefs>);

impl Debug for DynDebugReferences {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let Self(inner) = self;
        let inner = inner.upgrade();
        let refs = inner.as_ref().map(|inner| {
            let (marked_for_destruction, callers) = inner.refs_info();
            (alloc::sync::Arc::strong_count(inner), marked_for_destruction, callers)
        });
        debug_refs(refs, "DynDebugReferences", f)
    }
}

/// A trait allowing [`DynDebugReferences`] to erase the `T` type on [`Inner`].
trait ExposeRefs: Send + Sync + 'static {
    fn refs_info(&self) -> (&AtomicBool, &caller::Callers);
}

impl<T: Send + Sync + 'static> ExposeRefs for Inner<T> {
    fn refs_info(&self) -> (&AtomicBool, &caller::Callers) {
        (&self.marked_for_destruction, &self.callers)
    }
}

/// Provides delegated notification of all strong references of a [`Primary`]
/// being dropped.
///
/// See [`Primary::unwrap_with_notifier`].
pub trait Notifier<T>: Send {
    /// Called when the data contained in the [`Primary`] reference can be
    /// extracted out because there are no more strong references to it.
    fn notify(&mut self, data: T);
}

/// An implementation of [`Notifier`] that stores the unwrapped data in a
/// `Clone` type.
///
/// Useful for tests where completion assertions are possible and useful.
#[derive(Debug, Derivative)]
#[derivative(Clone(bound = ""))]
pub struct ArcNotifier<T>(alloc::sync::Arc<crate::Mutex<Option<T>>>);

impl<T> ArcNotifier<T> {
    /// Creates a new `ArcNotifier`.
    pub fn new() -> Self {
        Self(alloc::sync::Arc::new(crate::Mutex::new(None)))
    }

    /// Takes the notified value, if any.
    pub fn take(&self) -> Option<T> {
        let Self(inner) = self;
        inner.lock().take()
    }
}

impl<T: Send> Notifier<T> for ArcNotifier<T> {
    fn notify(&mut self, data: T) {
        let Self(inner) = self;
        assert!(inner.lock().replace(data).is_none(), "notified twice");
    }
}

/// An implementation of [`Notifier`] that wraps another `Notifier` and applies
/// a function on notified objects.
pub struct MapNotifier<N, F> {
    inner: N,
    map: Option<F>,
}

impl<N, F> MapNotifier<N, F> {
    /// Creates a new [`MapNotifier`] that wraps `notifier` with a mapping
    /// function `F`.
    pub fn new(notifier: N, map: F) -> Self {
        Self { inner: notifier, map: Some(map) }
    }
}

impl<A, B, N: Notifier<B>, F: FnOnce(A) -> B> Notifier<A> for MapNotifier<N, F>
where
    Self: Send,
{
    fn notify(&mut self, data: A) {
        let Self { inner, map } = self;
        let map = map.take().expect("notified twice");
        inner.notify(map(data))
    }
}

/// A handy implementation for the common Infallible "Never" type.
impl<T> Notifier<T> for core::convert::Infallible {
    fn notify(&mut self, _data: T) {
        match *self {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zombie_weak() {
        let primary = Primary::new(());
        let weak = {
            let strong = Primary::clone_strong(&primary);
            Strong::downgrade(&strong)
        };
        core::mem::drop(primary);

        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn rcs() {
        const INITIAL_VAL: u8 = 1;
        const NEW_VAL: u8 = 2;

        let primary = Primary::new(crate::sync::Mutex::new(INITIAL_VAL));
        let strong = Primary::clone_strong(&primary);
        let weak = Strong::downgrade(&strong);

        *primary.lock().unwrap() = NEW_VAL;
        assert_eq!(*primary.deref().lock().unwrap(), NEW_VAL);
        assert_eq!(*strong.deref().lock().unwrap(), NEW_VAL);
        assert_eq!(*weak.upgrade().unwrap().deref().lock().unwrap(), NEW_VAL);
    }

    #[test]
    fn unwrap_primary_without_strong_held() {
        const VAL: u16 = 6;
        let primary = Primary::new(VAL);
        assert_eq!(Primary::unwrap(primary), VAL);
    }

    #[test]
    #[should_panic(expected = "can't unwrap, still had 1 strong refs")]
    fn unwrap_primary_with_strong_held() {
        let primary = Primary::new(8);
        let _strong: Strong<_> = Primary::clone_strong(&primary);
        let _: u16 = Primary::unwrap(primary);
    }

    #[test]
    #[should_panic(expected = "dropped Primary with 1 strong refs remaining")]
    fn drop_primary_with_strong_held() {
        let primary = Primary::new(9);
        let _strong: Strong<_> = Primary::clone_strong(&primary);
        core::mem::drop(primary);
    }

    // This test trips LSAN on Fuchsia for some unknown reason. The host-side
    // test should be enough to protect us against regressing on the panicking
    // check.
    #[cfg(not(target_os = "fuchsia"))]
    #[test]
    #[should_panic(expected = "oopsie")]
    fn double_panic_protect() {
        let primary = Primary::new(9);
        let strong = Primary::clone_strong(&primary);
        // This will cause primary to be dropped before strong and would yield a
        // double panic if we didn't protect against it in Primary's Drop impl.
        let _tuple_to_invert_drop_order = (primary, strong);
        panic!("oopsie");
    }

    #[cfg(feature = "rc-debug-names")]
    #[test]
    fn tracked_callers() {
        let primary = Primary::new(10);
        // Mark this position so we ensure all track_caller marks are correct in
        // the methods that support it.
        let here = Location::caller();
        let strong1 = Primary::clone_strong(&primary);
        let strong2 = strong1.clone();
        let weak = Strong::downgrade(&strong2);
        let strong3 = weak.upgrade().unwrap();

        let Primary { inner } = &primary;
        let Inner { marked_for_destruction: _, callers, data: _, notifier: _, resource_token: _ } =
            &***inner;

        let strongs = [strong1, strong2, strong3];
        let _: &Location<'_> = strongs.iter().enumerate().fold(here, |prev, (i, cur)| {
            let Strong { inner: _, caller: caller::TrackedCaller { location: cur } } = cur;
            assert_eq!(prev.file(), cur.file(), "{i}");
            assert!(prev.line() < cur.line(), "{prev} < {cur}, {i}");
            {
                let callers = callers.callers.lock().unwrap();
                assert_eq!(callers.get(cur).copied(), Some(1));
            }

            cur
        });

        // All callers must be removed from the callers map on drop.
        std::mem::drop(strongs);
        {
            let callers = callers.callers.lock().unwrap();
            let callers = callers.deref();
            assert!(callers.is_empty(), "{callers:?}");
        }
    }
    #[cfg(feature = "rc-debug-names")]
    #[test]
    fn same_location_caller_tracking() {
        fn clone_in_fn<T>(p: &Primary<T>) -> Strong<T> {
            Primary::clone_strong(p)
        }

        let primary = Primary::new(10);
        let strong1 = clone_in_fn(&primary);
        let strong2 = clone_in_fn(&primary);
        assert_eq!(strong1.caller.location, strong2.caller.location);

        let Primary { inner } = &primary;
        let Inner { marked_for_destruction: _, callers, data: _, notifier: _, resource_token: _ } =
            &***inner;

        {
            let callers = callers.callers.lock().unwrap();
            assert_eq!(callers.get(&strong1.caller.location).copied(), Some(2));
        }

        std::mem::drop(strong1);
        std::mem::drop(strong2);

        {
            let callers = callers.callers.lock().unwrap();
            let callers = callers.deref();
            assert!(callers.is_empty(), "{callers:?}");
        }
    }

    #[cfg(feature = "rc-debug-names")]
    #[test]
    #[should_panic(expected = "sync/src/rc.rs")]
    fn callers_in_panic() {
        let primary = Primary::new(10);
        let _strong = Primary::clone_strong(&primary);
        drop(primary);
    }

    #[test]
    fn unwrap_with_notifier() {
        let primary = Primary::new(10);
        let strong = Primary::clone_strong(&primary);
        let notifier = ArcNotifier::new();
        Primary::unwrap_with_notifier(primary, notifier.clone());
        // Strong reference is still alive.
        assert_eq!(notifier.take(), None);
        core::mem::drop(strong);
        assert_eq!(notifier.take(), Some(10));
    }

    #[test]
    fn unwrap_or_notify_with_immediate() {
        let primary = Primary::new(10);
        let result = Primary::unwrap_or_notify_with::<ArcNotifier<_>, (), _>(primary, || {
            panic!("should not try to create notifier")
        });
        assert_eq!(result, Ok(10));
    }

    #[test]
    fn unwrap_or_notify_with_deferred() {
        let primary = Primary::new(10);
        let strong = Primary::clone_strong(&primary);
        let result = Primary::unwrap_or_notify_with(primary, || {
            let notifier = ArcNotifier::new();
            (notifier.clone(), notifier)
        });
        let notifier = result.unwrap_err();
        assert_eq!(notifier.take(), None);
        core::mem::drop(strong);
        assert_eq!(notifier.take(), Some(10));
    }

    #[test]
    fn map_notifier() {
        let primary = Primary::new(10);
        let notifier = ArcNotifier::new();
        let map_notifier = MapNotifier::new(notifier.clone(), |data| (data, data + 1));
        Primary::unwrap_with_notifier(primary, map_notifier);
        assert_eq!(notifier.take(), Some((10, 11)));
    }

    #[test]
    fn new_cyclic() {
        #[derive(Debug)]
        struct Data {
            value: i32,
            weak: Weak<Data>,
        }

        let primary = Primary::new_cyclic(|weak| Data { value: 2, weak });
        assert_eq!(primary.value, 2);
        let strong = primary.weak.upgrade().unwrap();
        assert_eq!(strong.value, 2);
        assert!(Primary::ptr_eq(&primary, &strong));
    }

    macro_rules! assert_debug_id_eq {
        ($id1:expr, $id2:expr) => {
            assert_eq!(alloc::format!("{:?}", $id1), alloc::format!("{:?}", $id2))
        };
    }
    macro_rules! assert_debug_id_ne {
        ($id1:expr, $id2:expr) => {
            assert_ne!(alloc::format!("{:?}", $id1), alloc::format!("{:?}", $id2))
        };
    }

    #[test]
    fn debug_ids_are_stable() {
        // Verify that transforming a given RC doesn't change it's debug_id.
        let primary = Primary::new(1);
        let strong = Primary::clone_strong(&primary);
        let weak_p = Primary::downgrade(&primary);
        let weak_s = Strong::downgrade(&strong);
        let weak_c = weak_p.clone();
        assert_debug_id_eq!(&primary.debug_id(), &strong.debug_id());
        assert_debug_id_eq!(&primary.debug_id(), &weak_p.debug_id());
        assert_debug_id_eq!(&primary.debug_id(), &weak_s.debug_id());
        assert_debug_id_eq!(&primary.debug_id(), &weak_c.debug_id());
    }

    #[test]
    fn debug_ids_are_unique() {
        // Verify that RCs to different data have different debug_ids.
        let primary1 = Primary::new(1);
        let primary2 = Primary::new(1);
        assert_debug_id_ne!(&primary1.debug_id(), &primary2.debug_id());

        // Verify that dropping an RC does not allow it's debug_id to be reused.
        let id1 = format!("{:?}", primary1.debug_id());
        std::mem::drop(primary1);
        let primary3 = Primary::new(1);
        assert_ne!(id1, format!("{:?}", primary3.debug_id()));
    }
}
