// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::instrument::Hooks;

use super::{AtomicFutureHandle, Meta, VTable};
use fuchsia_sync::Mutex;
use std::collections::HashMap;
use std::ptr::NonNull;
use std::task::{Context, Poll};

/// We don't want to pay a cost if there are no hooks, so we store a mapping from task ID to
/// HooksWrapper in the executor.
#[derive(Default)]
pub struct HooksMap(Mutex<HashMap<usize, NonNull<()>>>);

unsafe impl Send for HooksMap {}
unsafe impl Sync for HooksMap {}

struct HooksWrapper<H> {
    orig_vtable: &'static VTable,
    hooks: H,
}

impl<H: Hooks> HooksWrapper<H> {
    // # Safety
    //
    // We rely on the fact that all these functions are called whilst we have exclusive
    // access to the underlying future and the associated Hooks object.
    const VTABLE: VTable = VTable {
        drop: Self::drop,
        drop_future: Self::drop_future,
        poll: Self::poll,
        get_result: Self::get_result,
        drop_result: Self::drop_result,
    };

    // Returns a mutable reference to the wrapper. This will be safe from the functions below
    // because they are all called when we have exclusive access.
    unsafe fn wrapper<'a>(meta: NonNull<Meta>) -> &'a mut Self {
        let id = meta.as_ptr() as usize;
        unsafe {
            meta.as_ref()
                .scope()
                .executor()
                .hooks_map
                .0
                .lock()
                .get(&id)
                .unwrap()
                .cast::<Self>()
                .as_mut()
        }
    }

    unsafe fn drop(mut meta: NonNull<Meta>) {
        let meta_ref = unsafe { meta.as_mut() };
        // Remove the hooks entry from the map.
        let id = meta.as_ptr() as usize;
        let hooks = unsafe {
            Box::from_raw(
                meta_ref
                    .scope()
                    .executor()
                    .hooks_map
                    .0
                    .lock()
                    .remove(&id)
                    .unwrap()
                    .cast::<Self>()
                    .as_mut(),
            )
        };
        // Restore the vtable because the drop implementation can call `drop_future` or
        // `drop_result`, but we've removed the hooks from the map now.
        meta_ref.vtable = hooks.orig_vtable;
        unsafe { (hooks.orig_vtable.drop)(meta) };
    }

    unsafe fn poll(meta: NonNull<Meta>, cx: &mut Context<'_>) -> Poll<()> {
        let wrapper = unsafe { Self::wrapper(meta) };
        wrapper.hooks.task_poll_start();
        let result = unsafe { (wrapper.orig_vtable.poll)(meta, cx) };
        wrapper.hooks.task_poll_end();
        if result.is_ready() {
            wrapper.hooks.task_completed();
        }
        result
    }

    unsafe fn drop_future(meta: NonNull<Meta>) {
        unsafe { (Self::wrapper(meta).orig_vtable.drop_future)(meta) };
    }

    unsafe fn get_result(meta: NonNull<Meta>) -> *const () {
        unsafe { (Self::wrapper(meta).orig_vtable.get_result)(meta) }
    }

    unsafe fn drop_result(meta: NonNull<Meta>) {
        unsafe { (Self::wrapper(meta).orig_vtable.drop_result)(meta) };
    }
}

impl AtomicFutureHandle<'_> {
    /// Adds hooks to the future.
    pub fn add_hooks<H: Hooks>(&mut self, hooks: H) {
        let id = self.id();
        // SAFETY: This is safe because we have exclusive access.
        let meta: &mut Meta = unsafe { self.0.as_mut() };
        {
            let mut hooks_map = meta.scope().executor().hooks_map.0.lock();
            // SAFETY: Safe because `Box::into_raw` is guaranteed to give is a non-null pointer. We
            // can use `Box::into_non_null` when it's stabilised.
            assert!(
                hooks_map
                    .insert(
                        id,
                        NonNull::from_mut(Box::leak(Box::new(HooksWrapper {
                            orig_vtable: meta.vtable,
                            hooks,
                        })))
                        .cast::<()>()
                    )
                    .is_none()
            );
        }
        // Inject our vtable.
        meta.vtable = &HooksWrapper::<H>::VTABLE;
    }
}

#[cfg(test)]
mod tests {
    use super::Hooks;
    use crate::runtime::fuchsia::executor::scope::Spawnable;
    use crate::{SpawnableFuture, TestExecutor, yield_now};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    #[test]
    fn test_hooks() {
        let mut executor = TestExecutor::new();
        let scope = executor.global_scope();
        let mut future = SpawnableFuture::new(async {
            yield_now().await;
        })
        .into_task(scope.clone());
        #[derive(Default)]
        struct MyHooks {
            poll_start: AtomicU32,
            poll_end: AtomicU32,
            completed: AtomicBool,
        }
        impl Hooks for Arc<MyHooks> {
            fn task_completed(&mut self) {
                assert!(!self.completed.load(Ordering::Relaxed));
                self.completed.store(true, Ordering::Relaxed);
            }
            fn task_poll_start(&mut self) {
                self.poll_start.fetch_add(1, Ordering::Relaxed);
            }
            fn task_poll_end(&mut self) {
                self.poll_end.fetch_add(1, Ordering::Relaxed);
            }
        }
        let my_hooks = Arc::new(MyHooks::default());
        future.add_hooks(my_hooks.clone());
        scope.insert_task(future, false);
        assert!(executor.run_until_stalled(&mut std::future::pending::<()>()).is_pending());
        assert_eq!(my_hooks.poll_start.load(Ordering::Relaxed), 2);
        assert_eq!(my_hooks.poll_end.load(Ordering::Relaxed), 2);
        assert!(my_hooks.completed.load(Ordering::Relaxed));
    }
}
