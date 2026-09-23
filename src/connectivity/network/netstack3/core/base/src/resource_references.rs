// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Helper types, traits, and aliases for dealing with resource references.

use core::convert::Infallible as Never;

use crate::sync::{DynDebugReferences, MapRcNotifier, PrimaryRc, RcNotifier};

/// A context trait determining the types to be used for reference
/// notifications.
pub trait ReferenceNotifiers {
    /// The receiver for shared reference destruction notifications.
    type ReferenceReceiver<T: 'static>: 'static;
    /// The notifier for shared reference destruction notifications.
    type ReferenceNotifier<T: Send + 'static>: RcNotifier<T> + 'static;

    /// Creates a new Notifier/Receiver pair for `T`.
    ///
    /// `debug_references` is given to provide information on outstanding
    /// references that caused the notifier to be requested.
    fn new_reference_notifier<T: Send + 'static>(
        debug_references: DynDebugReferences,
    ) -> (Self::ReferenceNotifier<T>, Self::ReferenceReceiver<T>);
}

/// A context trait that allows core to defer observing proper resource cleanup
/// to bindings.
///
/// This trait exists as a debug utility to expose resources that are not
/// removed from the system as expected.
pub trait DeferredResourceRemovalContext: ReferenceNotifiers {
    /// Defers the removal of some resource `T` to bindings.
    ///
    /// Bindings can watch `receiver` and notify when resource removal is not
    /// completing in a timely manner.
    fn defer_removal<T: Send + 'static>(&mut self, receiver: Self::ReferenceReceiver<T>);

    /// A shorthand for [`defer_removal`] that takes a `ReferenceReceiver` from
    /// the `Deferred` variant of a [`RemoveResourceResult`].
    ///
    /// The default implementation is `track_caller` when the `instrumented`
    /// feature is available so implementers can track the caller location when
    /// needed.
    #[cfg_attr(feature = "instrumented", track_caller)]
    fn defer_removal_result<T: Send + 'static>(
        &mut self,
        result: RemoveResourceResultWithContext<T, Self>,
    ) {
        match result {
            // We don't need to do anything for synchronously removed resources.
            RemoveResourceResult::Removed(_) => (),
            RemoveResourceResult::Deferred(d) => self.defer_removal(d),
        }
    }
}

/// The result of removing some reference-counted resource from core.
#[derive(Debug)]
pub enum RemoveResourceResult<R, D> {
    /// The resource was synchronously removed and no more references to it
    /// exist.
    Removed(R),
    /// The resource was marked for destruction but there are still references
    /// to it in existence. The provided receiver can be polled on to observe
    /// resource destruction completion.
    Deferred(D),
}

impl<R, D> RemoveResourceResult<R, D> {
    /// Maps the `Removed` variant to a different type.
    pub fn map_removed<N, F: FnOnce(R) -> N>(self, f: F) -> RemoveResourceResult<N, D> {
        match self {
            Self::Removed(r) => RemoveResourceResult::Removed(f(r)),
            Self::Deferred(d) => RemoveResourceResult::Deferred(d),
        }
    }

    /// Maps the `Deferred` variant to a different type.
    pub fn map_deferred<N, F: FnOnce(D) -> N>(self, f: F) -> RemoveResourceResult<R, N> {
        match self {
            Self::Removed(r) => RemoveResourceResult::Removed(r),
            Self::Deferred(d) => RemoveResourceResult::Deferred(f(d)),
        }
    }
}

impl<R> RemoveResourceResult<R, Never> {
    /// A helper function to unwrap a [`RemoveResourceResult`] that can never be
    /// [`RemoveResourceResult::Deferred`].
    pub fn into_removed(self) -> R {
        match self {
            Self::Removed(r) => r,
        }
    }
}

/// An alias for [`RemoveResourceResult`] that extracts the receiver type from
/// the bindings context.
pub type RemoveResourceResultWithContext<S, BT> =
    RemoveResourceResult<S, <BT as ReferenceNotifiers>::ReferenceReceiver<S>>;

/// An extension of [`ReferenceNotifiers`] providing extra functionality.
pub trait ReferenceNotifiersExt: ReferenceNotifiers {
    /// Unwraps a [`PrimaryRc`] if it has no pending references, otherwise
    /// notifying with [`Self::ReferenceNotifier`].
    ///
    /// S: Is the state held behind the [`PrimaryRc`].
    /// O: Is the desired output state, once references have been destructed.
    /// F: Is a function that converts S to O.
    fn unwrap_or_notify_with_new_reference_notifier<
        S: Send + Sync + 'static,
        O: Send,
        F: Send + Clone + 'static + FnOnce(S) -> O,
    >(
        primary: PrimaryRc<S>,
        map: F,
    ) -> RemoveResourceResultWithContext<O, Self> {
        let debug_references = PrimaryRc::debug_references(&primary).into_dyn();
        let map_clone = map.clone();
        match PrimaryRc::unwrap_or_notify_with(primary, move || {
            let (notifier, receiver) = Self::new_reference_notifier::<O>(debug_references);
            let notifier = MapRcNotifier::new(notifier, map_clone);
            (notifier, receiver)
        }) {
            Ok(state) => RemoveResourceResult::Removed(map(state)),
            Err(receiver) => RemoveResourceResult::Deferred(receiver),
        }
    }
}

impl<N: ReferenceNotifiers> ReferenceNotifiersExt for N {}
