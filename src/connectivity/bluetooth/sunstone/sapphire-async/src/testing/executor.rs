// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod waker;

use core::cell::RefCell;
use core::future::Future;
use core::marker::PhantomPinned;
use core::pin::Pin;
use core::ptr::NonNull;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use sapphire_sync::mutex::Mutex;
use sapphire_sync::mutex::raw::{SingleThreadMutex, StdMutex};

use crate::condition::Condition;
use crate::executor::Executor;
use crate::testing::executor::waker::make_waker;

/// A single-threaded scoped async testing executor.
///
/// `TestExecutor` manages concurrent task scheduling and execution inside unit tests without
/// standard reference-counted wakers (`Arc`). By managing task lifetimes statically via the
/// `'env` scope lifetime parameter, the executor remains 100% sound under Rust Stacked Borrows
/// and Miri checks.
///
/// # Examples
///
/// Spawning tasks and driving them via the executor scope:
///
/// ```
/// use sapphire_async::testing::TestExecutor;
/// use sapphire_async::executor::BoundedExecutor;
/// use std::cell::Cell;
///
/// let completed = Cell::new(false);
///
/// BoundedExecutor::new(TestExecutor::new(), |executor| {
///     let mut handle = executor.spawn(async {
///         42
///     });
///
///     assert!(!handle.is_finished()); // Task enqueued but not yet run
///     executor.run_until_stalled();     // Drive the executor queue
///     assert!(handle.is_finished());  // Task successfully executed!
///     assert_eq!(handle.get(), Some(42));
/// });
/// ```
pub struct TestExecutor {
    // Store raw pointers to heap-allocated Tasks using NonNull to prevent Rc moves and unique retags.
    tasks: TaskList,
    // Queue of runnable tasks (stored as indices into the vec)
    run_queue: Rc<Mutex<StdMutex, VecDeque<usize>>>,
    _pinned: PhantomPinned,
}

type TaskList = Rc<RefCell<Vec<Option<NonNull<Task<dyn Future<Output = ()> + 'static>>>>>>;

/// The sized header prefix of a task containing scheduling metadata.
struct TaskHeader {
    id: usize,
    ready_queue: Rc<Mutex<StdMutex, VecDeque<usize>>>,
}

/// A manual heap-allocated wrapper representing a pinned concurrent future and its run queue reference.
#[repr(C)]
struct Task<F: ?Sized> {
    header: Rc<TaskHeader>,
    future: F,
}

impl TestExecutor {
    pub fn new() -> Self {
        Self {
            tasks: Rc::new(RefCell::new(Vec::new())),
            run_queue: Rc::new(Mutex::new(VecDeque::new())),
            _pinned: PhantomPinned,
        }
    }

    pub fn run_until_stalled(&self) {
        // SAFETY: We won't move self
        while let Some(task_id) = { self.run_queue.lock().pop_front() } {
            let Some(mut task_nonnull) = self.tasks.borrow()[task_id] else {
                continue;
            };
            // SAFETY: Obtain header raw pointer to avoid triggering a SharedReadOnly retag invalidation.
            let waker = unsafe {
                let task = task_nonnull.as_ref();
                let header_ptr = Rc::downgrade(&task.header);
                make_waker(header_ptr)
            };
            let mut cx = Context::from_waker(&waker);

            // SAFETY: task_ptr points to a valid pinned heap allocation.
            // No other task can mutably borrow it concurrently because we are single-threaded.
            unsafe {
                let task = task_nonnull.as_mut();
                let pinned = Pin::new_unchecked(&mut task.future);
                let _ = pinned.poll(&mut cx);
            }
        }
    }

    pub fn block_on<'a, F>(&self, mut future: F) -> F::Output
    where
        F: Future + 'a,
    {
        let mut future = unsafe { Pin::new_unchecked(&mut future) };

        struct MainWake {
            woken: Arc<AtomicBool>,
        }
        impl Wake for MainWake {
            fn wake(self: Arc<Self>) {
                self.woken.store(true, Ordering::Release);
            }
        }
        let woken = Arc::new(AtomicBool::new(false));
        let waker = Waker::from(Arc::new(MainWake { woken: woken.clone() }));
        let mut cx = Context::from_waker(&waker);

        loop {
            match future.as_mut().poll(&mut cx) {
                Poll::Ready(out) => return out,
                Poll::Pending => {
                    self.run_until_stalled();
                    if self.run_queue.lock().is_empty() {
                        if !woken.swap(false, Ordering::Acquire) {
                            panic!("Deadlock detected in block_on");
                        }
                    }
                }
            }
        }
    }
}

struct JoinState<T> {
    output: Option<T>,
    // Completion flag needed since the `output` can be taken from the handle.
    completed: bool,
}

pub struct JoinHandle<T> {
    state: Rc<Condition<SingleThreadMutex, JoinState<T>>>,
    tasks: TaskList,
    id: usize,
}

impl<T> JoinHandle<T> {
    pub fn get(&mut self) -> Option<T> {
        self.state.lock().output.take()
    }

    pub fn is_finished(&self) -> bool {
        self.state.lock().completed
    }

    pub async fn join(self) -> T {
        self.state
            .when(|join_state| match join_state.output.take() {
                Some(t) => Poll::Ready(t),
                None => Poll::Pending,
            })
            .await
    }
    /// Polls the task once, returning its Poll status.
    ///
    /// # Panics
    ///
    /// Panics if the task was already cancelled or completed.
    pub fn poll_once(&self) -> Poll<()> {
        let tasks = self.tasks.borrow();
        let Some(&Some(mut task_nonnull)) = tasks.get(self.id) else {
            panic!("Task already cancelled or completed");
        };
        // SAFETY: Same as run_until_stalled.
        // No other task can mutably borrow it concurrently because we are single-threaded.
        unsafe {
            let task = task_nonnull.as_ref();
            let header_ptr = Rc::downgrade(&task.header);
            let waker = make_waker(header_ptr);
            let mut cx = Context::from_waker(&waker);
            let task = task_nonnull.as_mut();
            let pinned = Pin::new_unchecked(&mut task.future);
            pinned.poll(&mut cx)
        }
    }

    /// Cancels the task by taking and dropping its heap allocation.
    pub fn cancel(self) {
        if let Some(nonnull) = self.tasks.borrow_mut()[self.id].take() {
            // SAFETY: Reclaim heap memory manually allocated via Box::into_raw inside spawn()
            let _task = unsafe { Box::from_raw(nonnull.as_ptr()) };
        }
    }
}

impl Executor for TestExecutor {
    type JoinHandle<T> = JoinHandle<T>;

    unsafe fn spawn_unchecked<'a, F, T>(self: Pin<&Self>, future: F) -> Self::JoinHandle<T>
    where
        F: Future<Output = T> + 'a,
        T: 'a,
    {
        let state = Rc::new(Condition::new(JoinState { output: None, completed: false }));
        let state_clone = state.clone();

        let join_wrapper = async move {
            let out = future.await;
            let mut st = state_clone.lock();
            st.output = Some(out);
            st.completed = true;
            state_clone.notify_one();
        };

        // SAFETY: We won't move self
        let id = self.tasks.borrow().len();
        let header = Rc::new(TaskHeader { ready_queue: self.run_queue.clone(), id });
        let task = Box::new(Task { header, future: join_wrapper });

        let task = Box::into_raw(task);
        // SAFETY: Box::into_raw returns a non-null pointer.
        let task = unsafe { NonNull::new_unchecked(task) };
        let task = task as NonNull<Task<dyn Future<Output = ()> + 'a>>;
        // SAFETY: Extend the lifetime to 'static. The caller guarantees this is valid.
        let task = unsafe { core::mem::transmute(task) };
        self.tasks.borrow_mut().push(Some(task));
        self.run_queue.lock().push_back(id);

        JoinHandle { state, tasks: self.tasks.clone(), id }
    }
}

impl<T, F: Future<Output = T> + ?Sized> Future for Task<F> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // SAFETY: Performing a direct pin projection
        let mut future = unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().future) };
        future.as_mut().poll(cx)
    }
}

impl Drop for TestExecutor {
    fn drop(&mut self) {
        let mut tasks = self.tasks.borrow_mut();
        for nonnull in tasks.iter_mut().filter_map(|t| t.take()) {
            // SAFETY: Reclaim heap memory manually allocated via Box::into_raw inside spawn()
            // to prevent memory leaks on executor completion.
            let _task = unsafe { Box::from_raw(nonnull.as_ptr()) };
        }
        tasks.clear();
    }
}
