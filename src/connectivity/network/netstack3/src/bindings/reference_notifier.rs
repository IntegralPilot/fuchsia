// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Reference drop notifier implementations for bindings.

use std::fmt::Debug;

use fuchsia_async as fasync;
use futures::channel::oneshot;
use futures::{Future, FutureExt as _, StreamExt as _};
use log::{debug, warn};
use netstack3_core::ReferenceNotifiers;
use netstack3_core::sync::{DynDebugReferences, RcNotifier};

use crate::bindings::{BindingsCtx, Ctx};

/// Implements `RcNotifier` for futures oneshot channels.
///
/// We need a newtype here because of orphan rules.
pub(crate) struct ReferenceNotifier<T>(Option<oneshot::Sender<T>>);

impl<T> ReferenceNotifier<T> {
    fn new(debug_references: DynDebugReferences) -> (Self, ReferenceReceiver<T>) {
        let (sender, receiver) = oneshot::channel();
        (ReferenceNotifier(Some(sender)), ReferenceReceiver { receiver, debug_references })
    }
}

impl<T: Send> RcNotifier<T> for ReferenceNotifier<T> {
    fn notify(&mut self, data: T) {
        let Self(inner) = self;
        inner.take().expect("notified twice").send(data).unwrap_or_else(|_: T| {
            panic!(
                "receiver was dropped before notifying for {}",
                // Print the type name so we don't need Debug bounds.
                core::any::type_name::<T>()
            )
        })
    }
}

pub(crate) struct ReferenceReceiver<T> {
    pub(crate) receiver: oneshot::Receiver<T>,
    pub(crate) debug_references: DynDebugReferences,
}

impl<T> ReferenceReceiver<T> {
    pub(crate) fn into_future<'a>(
        self,
        resource_name: &'static str,
        resource_id: &'a impl Debug,
        ctx: &impl ReferenceReceiverIntoFutureContext<T>,
    ) -> impl Future<Output = T> + 'a
    where
        T: 'a,
    {
        let Self { receiver: _, debug_references: refs } = &self;
        debug!("{resource_name} {resource_id:?} removal is pending references: {refs:?}");

        // Reference notifier panics if we drop the receiver, which is a misuse
        // detector. But if this future gets dropped due to scope cancelation
        // we could end up in a bad place. So acquire a scope guard here in
        // order to prevent cancelation. If the scope is already cancelled then
        // there's unprotected shutdown happening and we just defer the removal
        // to the worker instead.

        let Some(scope_guard) = fasync::Scope::current().active_guard() else {
            warn!(
                "deferred {resource_name} {resource_id:?} removal; \
                    scope can't acquire active guards."
            );
            ctx.defer(self);
            // We can't get a scope guard, so this task must have been canceled
            // already and will exit at the next await point.
            return futures::future::pending().left_future();
        };

        let Self { receiver, debug_references: refs } = self;

        // If we get stuck trying to remove the resource, log the remaining refs
        // at a low frequency to aid debugging.
        let interval_logging = fasync::Interval::new(zx::MonotonicDuration::from_seconds(30))
            .map(move |()| {
                warn!("{resource_name} {resource_id:?} removal is pending references: {refs:?}")
            })
            .collect::<()>();

        futures::future::select(receiver, interval_logging)
            .map(move |r| match r {
                futures::future::Either::Left((rcv, _)) => {
                    // Resource has been dropped, we can drop the scope guard.
                    drop(scope_guard);
                    rcv.expect("sender dropped without notifying")
                }
                futures::future::Either::Right(((), _)) => {
                    unreachable!("interval channel never completes")
                }
            })
            .right_future()
    }
}

/// A trait abstracting away [`Ctx`] for [`ReferenceReceiver::into_future`] so
/// we can write tests without building a full stack context.
///
/// Implemented for [`Ctx`] outside of tests.
pub(crate) trait ReferenceReceiverIntoFutureContext<T> {
    /// Defers removal notifying on `receiver` due to scope cancelation.
    fn defer(&self, receiver: ReferenceReceiver<T>);
}

impl<T: Send + 'static> ReferenceReceiverIntoFutureContext<T> for Ctx {
    fn defer(&self, receiver: ReferenceReceiver<T>) {
        self.bindings_ctx().resource_removal.defer_removal_with_receiver(receiver);
    }
}

impl ReferenceNotifiers for BindingsCtx {
    type ReferenceReceiver<T: 'static> = ReferenceReceiver<T>;

    type ReferenceNotifier<T: Send + 'static> = ReferenceNotifier<T>;

    fn new_reference_notifier<T: Send + 'static>(
        debug_references: DynDebugReferences,
    ) -> (Self::ReferenceNotifier<T>, Self::ReferenceReceiver<T>) {
        ReferenceNotifier::new(debug_references)
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::sync::Arc;
    use std::task::Poll;

    use netstack3_core::sync::{Mutex, PrimaryRc};

    use super::*;

    struct Resource;

    impl Resource {
        fn disarm(self) {
            std::mem::forget(self);
        }
    }

    impl Drop for Resource {
        fn drop(&mut self) {
            panic!("resource dropped");
        }
    }

    #[derive(Clone, Default)]
    struct TestDeferRemovalContext(Arc<Mutex<Option<ReferenceReceiver<Resource>>>>);

    impl TestDeferRemovalContext {
        fn take(&self) -> Option<ReferenceReceiver<Resource>> {
            let Self(inner) = self;
            inner.lock().take()
        }
    }

    impl ReferenceReceiverIntoFutureContext<Resource> for TestDeferRemovalContext {
        fn defer(&self, receiver: ReferenceReceiver<Resource>) {
            let Self(inner) = self;
            assert!(inner.lock().replace(receiver).is_none(), "deferred twice");
        }
    }

    #[test]
    fn reference_receiver_future_holds_scope_guard() {
        let mut executor = fasync::TestExecutor::new();
        let primary = PrimaryRc::new(Resource);
        let refs = PrimaryRc::debug_references(&primary).into_dyn();
        let (notifier, receiver) = ReferenceNotifier::<Resource>::new(refs);
        let debug_id = "resource".to_string();

        let ctx = TestDeferRemovalContext::default();
        let ctx_clone = ctx.clone();

        let scope = executor.global_scope().new_child_with_name("test");
        let mut join_handle = pin!(scope.spawn(async move {
            let resource = receiver.into_future("resource", &debug_id, &ctx_clone).await;
            resource.disarm();
        }));
        assert_eq!(executor.run_until_stalled(&mut join_handle), Poll::Pending);
        let mut cancel = pin!(scope.cancel());
        assert_eq!(executor.run_until_stalled(&mut cancel), Poll::Pending);
        PrimaryRc::unwrap_with_notifier(primary, notifier);
        assert_eq!(executor.run_until_stalled(&mut cancel), Poll::Ready(()));
        // Did not defer.
        assert!(ctx.take().is_none());
    }

    #[test]
    fn reference_receiver_defers_on_cancelled_scope() {
        let mut executor = fasync::TestExecutor::new();
        let primary = PrimaryRc::new(Resource);
        let refs = PrimaryRc::debug_references(&primary).into_dyn();
        let (notifier, receiver) = ReferenceNotifier::<Resource>::new(refs);
        let debug_id = "resource".to_string();

        let ctx = TestDeferRemovalContext::default();
        let ctx_clone = ctx.clone();

        let scope = executor.global_scope().new_child_with_name("test");
        let guard = scope.active_guard().unwrap();
        let mut cancel = pin!(scope.cancel());

        let _: fasync::JoinHandle<()> = guard.to_handle().spawn(async move {
            // Drop the guard which should cause the scope to go down, guard
            // won't be able to be reacquired.
            drop(guard);
            let _resource = receiver.into_future("resource", &debug_id, &ctx_clone).await;
            panic!("should not have resolved");
        });
        assert_eq!(executor.run_until_stalled(&mut cancel), Poll::Ready(()));
        PrimaryRc::unwrap_with_notifier(primary, notifier);
        let mut receiver = ctx.take().expect("deferred");
        let resource = receiver
            .receiver
            .try_recv()
            .expect("sender dropped before notifying")
            .expect("resource not in receiver");
        resource.disarm();
    }
}
