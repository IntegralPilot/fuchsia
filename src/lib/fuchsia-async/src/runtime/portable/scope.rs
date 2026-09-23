// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This implements a *very* limited version of the fuchsia Scope API.  More can be added
// as needed.

use crate::condition::{Condition, ConditionGuard};
use std::collections::HashMap;
use std::future::{Future, poll_fn};
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::task::{Poll, Waker};
use std::{fmt, hash};
use tokio::task::AbortHandle;

/// A scope for async tasks. This scope is cancelled when dropped.
///
/// See the [Fuchsia target documentation on this type][docs] for more
/// information about scopes. This version of the Scope API is a limited subset
/// of that API.
///
/// [docs]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_async/struct.Scope.html
#[derive(Debug)]
pub struct Scope {
    inner: ScopeHandle,
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

impl Scope {
    /// Returns a new scope that is a child of the root scope of the executor.
    pub fn new() -> Self {
        Self {
            inner: ScopeHandle {
                inner: Arc::new(ScopeInner {
                    state: Condition::new(ScopeState {
                        all_tasks: HashMap::new(),
                        state: State::Active,
                        guards: 0,
                    }),
                    name: String::new(),
                }),
            },
        }
    }

    /// Returns a new scope that is a child of the root scope of the executor.
    pub fn new_with_name(name: &str) -> Self {
        Self {
            inner: ScopeHandle {
                inner: Arc::new(ScopeInner {
                    state: Condition::new(ScopeState {
                        all_tasks: HashMap::new(),
                        state: State::Active,
                        guards: 0,
                    }),
                    name: name.to_string(),
                }),
            },
        }
    }

    /// Returns the name of the scope.
    pub fn name(&self) -> &str {
        &self.inner.inner.name
    }

    /// Creates a [`ScopeHandle`] to this scope.
    pub fn to_handle(&self) -> ScopeHandle {
        self.inner.clone()
    }

    /// Cancel all tasks cooperatively in the scope.
    ///
    /// `cancel` first gives a chance to all child tasks (including tasks of
    /// child scopes) to shutdown cleanly if they're holding on to a
    /// [`ScopeActiveGuard`]. Once no child tasks are holding on to guards, then
    /// `cancel` behaves like [`Scope::abort`], dropping all tasks and stopping
    /// them from running at the next yield point. A [`ScopeActiveGuard`]
    /// provides a cooperative cancellation signal that is triggered by this
    /// call, see its documentation for more details.
    ///
    /// Once the returned future resolves, no task on the scope will be polled
    /// again.
    ///
    /// Cancelling a scope _does not_ immediately prevent new tasks from being
    /// accepted. New tasks are accepted as long as there are
    /// `ScopeActiveGuard`s for this scope.
    pub fn cancel(self) -> impl Future<Output = ()> {
        self.inner.cancel_all_tasks();
        async move {
            self.inner.on_no_tasks().await;
        }
    }

    /// Aborts all the scope's tasks.
    ///
    /// Note that if this is called from within a task running on the scope, the
    /// task will not resume from the next await point.
    pub fn abort(self) -> impl Future<Output = ()> {
        self.inner.inner.state.lock().abort_tasks_and_mark_finished();
        async move {
            self.inner.on_no_tasks().await;
        }
    }

    /// Waits for all tasks in the scope to complete.
    pub async fn join(self) {
        self.inner.on_no_tasks().await;
    }

    /// Detach the scope, allowing its tasks to continue running in the
    /// background.
    pub fn detach(self) {
        // Use ManuallyDrop to destructure self, because Rust doesn't allow this
        // for types which implement Drop.
        let this = ManuallyDrop::new(self);
        // SAFETY: this.inner is obviously valid, and we don't access `this`
        // after moving.
        drop(unsafe { std::ptr::read(&this.inner) });
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        for task in self.inner.inner.state.lock().all_tasks.values() {
            task.abort_handle.abort();
        }
    }
}

impl Deref for Scope {
    type Target = ScopeHandle;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

/// A handle to a scope, which may be used to spawn tasks.
#[derive(Clone)]
pub struct ScopeHandle {
    inner: Arc<ScopeInner>,
}

impl ScopeHandle {
    /// Spawns a task on the scope.
    pub fn spawn(
        &self,
        future: impl Future<Output = ()> + Send + 'static,
    ) -> crate::JoinHandle<()> {
        self.compute(future).detach_on_drop()
    }

    /// Spawns a task on the scope.
    pub fn spawn_local(&self, future: impl Future<Output = ()> + 'static) -> crate::JoinHandle<()> {
        self.compute_local(future).detach_on_drop()
    }

    /// Spawns a task on the scope, but unlike `spawn` returns a handle that will drop the future
    /// when dropped.
    pub fn compute<T: Send + 'static>(
        &self,
        future: impl Future<Output = T> + Send + 'static,
    ) -> crate::Task<T> {
        let (task_id, fut) = self.mk_task_fut(future);
        // Take the lock first so that the insert below happens before the task tries to take the
        // lock to set the waker.
        let mut state = self.inner.state.lock();
        let task = crate::Task::spawn(fut);
        state.all_tasks.insert(task_id, ScopeTask::new(task.abort_handle()));
        task
    }

    /// Spawns a task on the scope, but unlike `spawn` returns a handle that will drop the future
    /// when dropped.
    pub fn compute_local<T: 'static>(
        &self,
        future: impl Future<Output = T> + 'static,
    ) -> crate::Task<T> {
        let (task_id, fut) = self.mk_task_fut(future);
        // Take the lock first so that the insert below happens before the task tries to take the
        // lock to set the waker.
        let mut state = self.inner.state.lock();
        let task = crate::Task::local(fut);
        state.all_tasks.insert(task_id, ScopeTask::new(task.abort_handle()));
        task
    }

    fn mk_task_fut<T: 'static, F: Future<Output = T> + 'static>(
        &self,
        future: F,
    ) -> (u64, impl Future<Output = T> + use<T, F>) {
        static TASK_ID: AtomicU64 = AtomicU64::new(0);

        let task_id = TASK_ID.fetch_add(1, Ordering::Relaxed);

        // When the task is dropped, we must clean up our entry in `all_tasks`.
        struct OnDrop {
            scope: WeakScopeRef,
            task_id: u64,
        }

        impl Drop for OnDrop {
            fn drop(&mut self) {
                if let Some(scope) = self.scope.upgrade() {
                    let wakers = {
                        let mut state = scope.inner.state.lock();
                        let ScopeTask { has_active_guard, .. } =
                            state.all_tasks.remove(&self.task_id).unwrap();
                        let wakers = if state.all_tasks.is_empty() {
                            state.drain_wakers().collect()
                        } else {
                            Vec::new()
                        };
                        if has_active_guard {
                            // There can't be any more wakers since we took them all above and we
                            // haven't dropped the lock.
                            assert!(ScopeState::release_cancel_guard(&mut state).is_empty());
                        }
                        wakers
                    };
                    for waker in wakers {
                        waker.wake();
                    }
                }
            }
        }

        let on_drop = OnDrop { scope: self.downgrade(), task_id };

        let weak = self.downgrade();
        let fut = async move {
            let _on_drop = on_drop;
            let mut future = std::pin::pin!(future);
            // Every time we poll, we must set the waker so that `wake_all` works.
            poll_fn(|cx| {
                let release_scope = weak.upgrade().and_then(|scope| {
                    let has_active_guard = {
                        let mut state = scope.inner.state.lock();
                        let task = state.all_tasks.get_mut(&task_id)?;
                        task.waker = Some(cx.waker().clone());
                        std::mem::take(&mut task.has_active_guard)
                    };
                    has_active_guard.then_some(scope)
                });
                let result = future.as_mut().poll(cx);
                if let Some(mut scope) = release_scope {
                    scope.release_cancel_guard();
                }
                result
            })
            .await
        };

        (task_id, fut)
    }

    /// Creates a [`WeakScopeRef`] for this scope.
    fn downgrade(&self) -> WeakScopeRef {
        WeakScopeRef { inner: Arc::downgrade(&self.inner) }
    }

    /// Waits for there to be no tasks.  This is racy: as soon as this returns it is possible for
    /// another task to have been spawned on this scope.
    pub async fn on_no_tasks(&self) {
        self.inner
            .state
            .when(|state| if state.all_tasks.is_empty() { Poll::Ready(()) } else { Poll::Pending })
            .await;
    }

    /// Wait for there to be no tasks and no guards. This is racy: as soon as this returns it is
    /// possible for another task to have been spawned on this scope, or for there to be guards.
    pub async fn on_no_tasks_and_guards(&self) {
        self.inner
            .state
            .when(|state| {
                if state.all_tasks.is_empty() && state.guards == 0 {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
    }

    /// Wakes all the scope's futures.
    pub fn wake_all_with_active_guard(&self) {
        let mut wakers = Vec::new();
        {
            let mut state = self.inner.state.lock();
            let mut guards = 0;
            for task in state.all_tasks.values_mut() {
                wakers.extend(task.waker.take());
                if !task.has_active_guard {
                    task.has_active_guard = true;
                    guards += 1;
                }
            }
            state.guards += guards;
        }
        for waker in wakers {
            waker.wake();
        }
    }

    /// Retrieves a [`ScopeActiveGuard`] for this scope.
    ///
    /// Note that this may fail if cancellation has already started for this
    /// scope. In that case, the caller must assume any tasks from this scope
    /// may be dropped at any yield point.
    ///
    /// Creating a [`ScopeActiveGuard`] is substantially more expensive than
    /// just polling it, so callers should maintain the returned guard when
    /// success is observed from this call for best performance.
    ///
    /// See [`Scope::cancel`] for details on cooperative cancellation behavior.
    #[must_use]
    pub fn active_guard(&self) -> Option<ScopeActiveGuard> {
        ScopeActiveGuard::new(self)
    }

    /// Cancel all the scope's tasks.
    ///
    /// Note that if this is called from within a task running on the scope, the
    /// task will not resume from the next await point.
    pub fn cancel(self) -> impl Future<Output = ()> {
        self.cancel_all_tasks();
        async move {
            self.on_no_tasks().await;
        }
    }

    /// Aborts all the scope's tasks.
    ///
    /// Note that if this is called from within a task running on the scope, the
    /// task will not resume from the next await point.
    pub fn abort(self) -> impl Future<Output = ()> {
        self.inner.state.lock().abort_tasks_and_mark_finished();
        async move {
            self.on_no_tasks().await;
        }
    }

    fn release_cancel_guard(&mut self) {
        let wakers = ScopeState::release_cancel_guard(&mut self.inner.state.lock());
        for waker in wakers {
            waker.wake();
        }
    }

    fn cancel_all_tasks(&self) {
        let mut state = self.inner.state.lock();
        if state.guards == 0 {
            state.abort_tasks_and_mark_finished();
        } else {
            state.state = State::PendingCancellation;
        }
    }
}

impl fmt::Debug for ScopeHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scope").field("name", &self.inner.name).finish()
    }
}

/// A weak reference to a scope.
#[derive(Clone)]
struct WeakScopeRef {
    inner: Weak<ScopeInner>,
}

impl WeakScopeRef {
    /// Upgrades to a [`ScopeRef`] if the scope still exists.
    pub fn upgrade(&self) -> Option<ScopeHandle> {
        self.inner.upgrade().map(|inner| ScopeHandle { inner })
    }
}

impl hash::Hash for WeakScopeRef {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        Weak::as_ptr(&self.inner).hash(state);
    }
}

impl PartialEq for WeakScopeRef {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for WeakScopeRef {
    // Weak::ptr_eq should return consistent results, even when the inner value
    // has been dropped.
}

struct ScopeInner {
    state: Condition<ScopeState>,
    name: String,
}

struct ScopeState {
    all_tasks: HashMap<u64, ScopeTask>,
    state: State,
    guards: u32,
}

impl ScopeState {
    fn abort_tasks_and_mark_finished(&mut self) {
        self.state = State::Finished;
        for task in self.all_tasks.values() {
            task.abort_handle.abort();
        }
    }

    #[must_use]
    fn release_cancel_guard(this: &mut ConditionGuard<'_, ScopeState>) -> Vec<Waker> {
        this.guards = this.guards.checked_sub(1).expect("released non-acquired guard");
        if this.guards == 0 {
            if this.state == State::PendingCancellation {
                this.abort_tasks_and_mark_finished();
            }
            this.drain_wakers().collect()
        } else {
            Vec::new()
        }
    }
}

struct ScopeTask {
    abort_handle: AbortHandle,
    waker: Option<Waker>,
    has_active_guard: bool,
}

impl ScopeTask {
    fn new(abort_handle: AbortHandle) -> Self {
        Self { abort_handle, waker: None, has_active_guard: false }
    }
}

#[derive(Eq, PartialEq)]
enum State {
    Active,
    PendingCancellation,
    Finished,
}

/// Holds a guard on the creating scope, holding off cancelation.
///
/// `ScopeActiveGuard` allows [`Scope`]s to perform cooperative cancellation.
/// [`ScopeActiveGuard::on_cancel`] returns a future that resolves when
/// [`Scope::cancel`] and [`ScopeHandle::cancel`] are called. That is the signal
/// sent to cooperative tasks to stop doing work and finish.
///
/// A `ScopeActiveGuard` is obtained via [`ScopeHandle::active_guard`].
/// `ScopeActiveGuard` releases the guard on the originating scope on drop.
#[derive(Debug)]
#[must_use]
pub struct ScopeActiveGuard(ScopeHandle);

impl Deref for ScopeActiveGuard {
    type Target = ScopeHandle;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for ScopeActiveGuard {
    fn drop(&mut self) {
        let Self(scope) = self;
        scope.release_cancel_guard();
    }
}

impl Clone for ScopeActiveGuard {
    fn clone(&self) -> Self {
        self.0.inner.state.lock().guards += 1;
        Self(self.0.clone())
    }
}

impl ScopeActiveGuard {
    /// Returns a borrow of the scope handle associated with this guard.
    pub fn as_handle(&self) -> &ScopeHandle {
        &self.0
    }

    /// Returns a clone of the scope handle associated with this guard.
    pub fn to_handle(&self) -> ScopeHandle {
        self.0.clone()
    }

    fn new(scope: &ScopeHandle) -> Option<Self> {
        let mut state = scope.inner.state.lock();
        if state.state == State::Finished {
            return None;
        }
        state.guards += 1;
        Some(Self(scope.clone()))
    }
}

// Tests are in runtime/scope.rs.
