// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::CurrentTask;
use fuchsia_rcu::rcu_run_callbacks;
use starnix_logging::log_warn;
use starnix_types::ownership::Releasable;
use std::cell::RefCell;
use std::ops::DerefMut;

/// Register the container to be deferred released.
pub fn register_delayed_release<T: for<'a> Releasable<Context<'a> = &'a CurrentTask> + 'static>(
    to_release: T,
) {
    RELEASERS.with(|cell| {
        let mut cell = cell.borrow_mut();
        let list = &mut cell.as_mut().expect("DelayedReleaser hasn't been finalized").releasables;
        list.push(Box::new(Some(to_release)));
    });
}

impl<T> CurrentTaskReleasable for Option<T>
where
    for<'a> T: Releasable<Context<'a> = &'a CurrentTask>,
{
    fn release_with_context(&mut self, context: &CurrentTask) {
        if let Some(this) = self.take() {
            <T as Releasable>::release(this, context);
        }
    }
}

/// An object-safe/dyn-compatible trait to wrap `Releasable` types.
pub trait CurrentTaskReleasable {
    fn release_with_context(&mut self, context: &CurrentTask);
}

struct DelayedCallback<F>(F);

impl<F> Releasable for DelayedCallback<F>
where
    F: FnOnce(&CurrentTask) + 'static,
{
    type Context<'a> = &'a CurrentTask;

    fn release<'a>(self, context: Self::Context<'a>) {
        (self.0)(context);
    }
}

/// Register a callback to be run during delayed release.
pub fn register_delayed_call<F>(callback: F)
where
    F: FnOnce(&CurrentTask) + 'static,
{
    register_delayed_release(DelayedCallback(callback));
}

thread_local! {
    /// Container of all `FileObject` that are not used anymore, but have not been closed yet.
    static RELEASERS: RefCell<Option<LocalReleasers>> =
        RefCell::new(Some(LocalReleasers::default()));
}

#[derive(Default)]
struct LocalReleasers {
    /// The list of entities to be deferred released.
    releasables: Vec<Box<dyn CurrentTaskReleasable>>,
}

impl LocalReleasers {
    fn is_empty(&self) -> bool {
        self.releasables.is_empty()
    }
}

impl Releasable for LocalReleasers {
    type Context<'a> = &'a CurrentTask;

    fn release<'a>(self, context: &'a CurrentTask) {
        let current_task = context;
        for mut releasable in self.releasables {
            releasable.release_with_context(current_task);
        }
    }
}

/// Service to handle delayed releases.
///
/// Delayed releases are cleanup code that is run at specific point where the lock level is
/// known. The starnix kernel must ensure that delayed releases are run regularly.
#[derive(Debug, Default)]
pub struct DelayedReleaser {}

impl DelayedReleaser {
    /// Run all current delayed releases for the current thread.
    pub fn apply(&self, current_task: &CurrentTask) {
        let mut counter = 0u32;
        loop {
            let ran_rcu = rcu_run_callbacks();
            let releasers = RELEASERS.with(|cell| {
                std::mem::take(
                    cell.borrow_mut()
                        .as_mut()
                        .expect("DelayedReleaser hasn't been finalized yet")
                        .deref_mut(),
                )
            });
            if releasers.is_empty() && !ran_rcu {
                return;
            }
            releasers.release(current_task);
            counter += 1;
            if counter == 100 {
                log_warn!("DelayedReleaser: applied >=100 delayed releases");
            }
            if counter > 10000 {
                panic!("DelayedReleaser: applied >10000 delayed releases");
            }
        }
    }

    /// Prevent any further releasables from being registered on this thread.
    ///
    /// This function should be called during thread teardown to ensure that we do not
    /// register any new releasables on this thread after we have finalized the delayed
    /// releasables for the last time.
    pub fn finalize() {
        RELEASERS.with(|cell| {
            let list = cell.borrow_mut().take().expect("DelayedReleaser hasn't been finalized");
            assert!(list.is_empty());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::spawn_kernel_and_run;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[::fuchsia::test]
    async fn test_register_delayed_call() {
        spawn_kernel_and_run(async move |current_task| {
            let flag = Arc::new(AtomicBool::new(false));
            let flag_clone = flag.clone();
            register_delayed_call(move |_current_task| {
                flag_clone.store(true, Ordering::SeqCst);
            });

            assert!(!flag.load(Ordering::SeqCst));
            DelayedReleaser::default().apply(current_task);
            assert!(flag.load(Ordering::SeqCst));
        })
        .await;
    }
}
