// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stack wide suspension blocking mechanism.

use std::sync::Arc;

use futures::Future;
use netstack3_core::sync::Mutex;

/// A guard that holds system suspension while held.
#[cfg_attr(test, derive(Debug))]
pub(crate) struct SuspensionGuard(Arc<Inner>);

impl Drop for SuspensionGuard {
    fn drop(&mut self) {
        let Self(inner) = self;
        let Inner { state, event } = &**inner;
        let mut lock = state.lock();
        let State { guards, allow_guards: _ } = &mut *lock;
        *guards = guards.checked_sub(1).expect("guards underflow");
        if *guards == 0 {
            let _notified: usize = event.notify(usize::MAX);
        }
    }
}

impl SuspensionGuard {
    /// Retrieves a suspension block that can issue more guards.
    pub(crate) fn suspension_block(&self) -> SuspensionBlock {
        let Self(inner) = self;
        SuspensionBlock(inner.clone())
    }
}

/// Vends [`SuspensionGuard`]s that block system suspension.
#[derive(Clone)]
#[cfg_attr(test, derive(Debug))]
pub(super) struct SuspensionBlock(Arc<Inner>);

impl SuspensionBlock {
    /// Creates a new [`SuspensionGuard`].
    ///
    /// The guard will block suspension until dropped.
    ///
    /// Pends until the `SuspensionBlockState` allows new guards.
    pub(crate) async fn guard(&self) -> SuspensionGuard {
        let Self(inner) = self;
        let Inner { state: _, event } = &**inner;

        loop {
            if let Some(g) = self.try_guard() {
                return g;
            }
            event_listener::listener!(event => listener);

            // Check again after registering listener.
            if let Some(g) = self.try_guard() {
                return g;
            }

            listener.await;
        }
    }

    /// Attempts to acquire a guard, returning `Some` if suspension guards are
    /// currently allowed.
    pub(crate) fn try_guard(&self) -> Option<SuspensionGuard> {
        let Self(inner) = self;
        let mut lock = inner.state.lock();
        let State { guards, allow_guards } = &mut *lock;
        if *allow_guards {
            *guards = guards.checked_add(1).expect("guards overflow");
            Some(SuspensionGuard(inner.clone()))
        } else {
            None
        }
    }
}

/// A dual state object that holds either a [`SuspensionGuard`] when it has an
/// active guard on system suspension, or a [`SuspensionBlock`] that can be used
/// to reacquire a guard.
#[cfg_attr(test, derive(Debug))]
pub(crate) enum MaybeSuspensionGuard {
    Guarded(SuspensionGuard),
    NotGuarded(SuspensionBlock),
}

impl MaybeSuspensionGuard {
    /// Creates a new not guarded instance.
    pub(crate) fn new(block: SuspensionBlock) -> Self {
        Self::NotGuarded(block)
    }

    /// Acquires a guard.
    ///
    /// Returns `true` if a new guard was acquired.
    pub(crate) async fn acquire(&mut self) -> bool {
        match self {
            Self::Guarded(_) => false,
            Self::NotGuarded(block) => {
                *self = Self::Guarded(block.guard().await);
                true
            }
        }
    }

    /// Releases a guard.
    ///
    /// Returns `true` if the guard was released.
    pub(crate) fn release(&mut self) -> bool {
        match self {
            Self::Guarded(guard) => {
                *self = Self::NotGuarded(guard.suspension_block());
                true
            }
            Self::NotGuarded(_) => false,
        }
    }
}

/// Provides `suspend` and `resume` control over the suspension block.
///
/// This is a separate type from [`SuspensionBlock`] to ensure _only one
/// instance_ of `SuspensionBlockControl` can exist. Note that the `suspend` and
/// `resume` methods require a mutable reference, further ensuring no shared
/// access.
pub(crate) struct SuspensionBlockControl(Arc<Inner>);

impl SuspensionBlockControl {
    /// Creates a new suspension block controller.
    pub(crate) fn new() -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State { allow_guards: true, guards: 0 }),
            event: event_listener::Event::new(),
        }))
    }

    /// Returns a [`SuspensionBlock`] that can be used to issue new
    /// [`SuspensionGuard`]s.
    pub(crate) fn issuer(&self) -> SuspensionBlock {
        SuspensionBlock(self.0.clone())
    }

    /// Blocks new guards from being issued and returns a future that completes
    /// when no guards are being held.
    ///
    /// Suspension can be cancelled with a call to
    /// [`SuspensionBlockControl::resume`]. The returned future *must not be
    /// polled* after `resume` is called.
    pub(crate) fn suspend(&mut self) -> impl Future<Output = ()> + use<> {
        let Self(inner) = self;
        // Disallow guards synchronously.
        inner.state.lock().allow_guards = false;

        let inner = inner.clone();

        // Create a future to be polled on later that notifies suspension
        // completed. Suspend is not directly async to facilitate the returned
        // future signature, so callers can keep it around without a mutable
        // reference to `SuspensionBlockControl`.
        async move {
            let try_suspend = || {
                let lock = inner.state.lock();
                let State { allow_guards, guards } = &*lock;
                assert_eq!(*allow_guards, false, "must not poll after resume");
                *guards == 0
            };
            loop {
                if try_suspend() {
                    break;
                }
                event_listener::listener!(inner.event => listener);

                // Check again after creating the listener.
                if try_suspend() {
                    break;
                }

                listener.await;
            }
        }
    }

    /// Allows guards to be issued again.
    pub(crate) fn resume(&mut self) {
        let Self(inner) = self;
        let Inner { state, event } = &**inner;
        state.lock().allow_guards = true;
        let _notified: usize = event.notify(usize::MAX);
    }
}

#[cfg_attr(test, derive(Debug))]
struct Inner {
    /// Mutable suspension blocking state.
    state: Mutex<State>,
    /// Holds the wakers for changes to the state.
    event: event_listener::Event,
}

#[cfg_attr(test, derive(Debug))]
struct State {
    /// Allow new guards to be vended.
    ///
    /// `guards` may be nonzero when `allow_guards` is set to `false`, it only
    /// causes new guards to be disallowed until this flips back to `true`.
    allow_guards: bool,
    /// How many [`SuspensionGuard`]s are currently held.
    guards: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::pin::pin;
    use std::task::Poll;

    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use futures::FutureExt;

    #[test]
    fn non_guarded_suspension() {
        let mut ctrl = SuspensionBlockControl::new();
        assert_eq!(ctrl.suspend().now_or_never(), Some(()));
        ctrl.resume();
        assert_eq!(ctrl.suspend().now_or_never(), Some(()));
    }

    #[test]
    fn wait_guards_on_suspended() {
        let mut exec = fasync::TestExecutor::new();
        let mut ctrl = SuspensionBlockControl::new();
        let issuer = ctrl.issuer();
        assert_eq!(ctrl.suspend().now_or_never(), Some(()));
        assert_matches!(issuer.try_guard(), None);
        let mut fut = pin!(issuer.guard());
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        ctrl.resume();
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(_));
    }

    #[test]
    fn suspend_waits_for_guards() {
        let mut exec = fasync::TestExecutor::new();
        let mut ctrl = SuspensionBlockControl::new();
        let issuer = ctrl.issuer();
        let guard = issuer.try_guard().expect("guard");
        let mut fut = pin!(ctrl.suspend());
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Pending);
        drop(guard);
        assert_matches!(exec.run_until_stalled(&mut fut), Poll::Ready(()));
    }

    #[fuchsia::test(logging = false)]
    async fn maybe_guard() {
        let ctrl = SuspensionBlockControl::new();
        let mut guard = MaybeSuspensionGuard::new(ctrl.issuer());
        assert_matches!(guard, MaybeSuspensionGuard::NotGuarded(_));
        assert!(guard.acquire().await);
        assert_matches!(guard, MaybeSuspensionGuard::Guarded(_));
        assert!(!guard.acquire().await);
        assert_matches!(guard, MaybeSuspensionGuard::Guarded(_));
        assert!(guard.release());
        assert_matches!(guard, MaybeSuspensionGuard::NotGuarded(_));
        assert!(!guard.release());
        assert_matches!(guard, MaybeSuspensionGuard::NotGuarded(_));
    }
}
