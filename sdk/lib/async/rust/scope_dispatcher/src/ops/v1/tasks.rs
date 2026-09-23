// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ptr::NonNull;
use core::{cmp, fmt};
use fuchsia_async::MonotonicInstant;
use libasync_sys::{async_dispatcher_t, async_task_t};
use std::collections::{BinaryHeap, VecDeque};
use std::sync::Arc;
use zx::Status;
use zx::sys::{ZX_OK, zx_status_t};

use crate::ScopeDispatcher;

/// An implementation of a queue for holding tasks in the order they need to be processed.
///
/// Pending tasks are stored as a binary heap of tasks with a custom ordering that makes sure the
/// next task popped off of it will be the correct task to run next. This ordering is:
/// - Any task with a timestamp in the past, *other than* the "infinite past", which is a special
///   value.
/// - Any task with the timestamp set to the "infinite past", which means it should get processed
///   after any other expired timestamps.
/// - Any task with the timestamp set to the "infinite future", which means it should only ever be
///   processed at dispatcher shutdown (as a cancelation).
#[derive(Default, Debug)]
pub struct TaskQueue {
    /// tasks that have a timestamp in the future as of when they were inserted to the queue.
    delayed_tasks: BinaryHeap<DelayedTask>,
    /// tasks that have a timestamp of the infinite past (meaning they should go after all delayed
    /// tasks that have been expired have been processed).
    pending_tasks: VecDeque<Task>,
}

/// The representation of a task that needs to be run.
#[derive(Eq, PartialEq)]
pub struct Task(NonNull<async_task_t>);

// SAFETY: The task structure is owned by the dispatcher once it's been queued, and the executor
// treats it as read only and does not modify it while owning it, so it is thread safe.
unsafe impl Send for Task {}
// SAFETY: The task structure is owned by the dispatcher once it's been queued, and the executor
// treats it as read only and does not modify it while owning it, so it is thread safe.
unsafe impl Sync for Task {}

/// A delayed task has a timestamp in the future as of when it was queued.
#[derive(Eq, PartialEq, Debug)]
struct DelayedTask(Task);

impl TaskQueue {
    pub fn queue_task(&mut self, task: Task) {
        // if the deadline is in the future, add it to the delayed tasks, otherwise it is pending.
        if task.deadline().is_some() {
            self.delayed_tasks.push(DelayedTask(task));
        } else {
            self.pending_tasks.push_back(task);
        }
    }

    pub fn next_task(&mut self, as_of: MonotonicInstant) -> Option<Task> {
        // try to find an expired deadline, but if there aren't any then just return the next
        // pending task.
        // TODO(543111947): This will not return delayed tasks with the same deadline in the same
        // order as they were enqueued, but it should.
        if let Some(task) = self.delayed_tasks.peek()
            && task.deadline() <= as_of
        {
            self.delayed_tasks.pop().map(|task| task.0)
        } else {
            self.pending_tasks.pop_front()
        }
    }

    pub fn peek_next_task(&self) -> Option<&Task> {
        if let Some(task) = self.delayed_tasks.peek() {
            return Some(&task.0);
        }
        self.pending_tasks.front()
    }

    /// tries to find the task in either delayed or pending and return it if it's in either.
    pub fn take_pending_task(&mut self, task: &Task) -> Option<Task> {
        if let Some(idx) = self.pending_tasks.iter().position(|it| it == task) {
            return self.pending_tasks.remove(idx);
        }
        if let Some(idx) = self.delayed_tasks.iter().position(|it| &it.0 == task) {
            // Swap out the heap so we can temporarily steal its vector.
            // This is not very efficient because it will have to re-heap with every removal,
            // but this should be the rarer operation, and we are at least not doing any new
            // allocations.
            let mut working = BinaryHeap::new();
            core::mem::swap(&mut working, &mut self.delayed_tasks);
            let mut working_vec = working.into_vec();
            let res = working_vec.remove(idx);
            self.delayed_tasks = working_vec.into();
            return Some(res.0);
        }
        None
    }
}

impl Task {
    /// Returns the deadline if it has one (ie. it does not have a deadline of
    /// [`MonotonicInstant::INFINITE_PAST`]).
    pub fn deadline(&self) -> Option<MonotonicInstant> {
        // SAFETY: The caller provided a valid non-null task to `post_task`, and is expected to keep
        // it alive until either it is successfully canceled or its callback is called.
        let deadline = MonotonicInstant::from_nanos(unsafe { self.0.as_ref() }.deadline);
        if deadline != MonotonicInstant::INFINITE_PAST { Some(deadline) } else { None }
    }

    /// Runs the task with the dispatcher pointer of the dispatcher given.
    pub fn run(self, dispatcher: Arc<ScopeDispatcher>, status: Result<(), Status>) {
        // SAFETY: The caller provided a valid non-null task to `post_task`, and is expected to keep
        // it alive until either it is successfully canceled or its callback is called.
        let Some(callback) = unsafe { self.0.as_ref() }.handler else { return };
        // SAFETY: The caller is expected to provide a valid function with the correct signature.
        unsafe {
            callback(
                dispatcher.as_ptr() as *mut _,
                self.0.as_ptr(),
                Status::result_into_raw(status),
            )
        };
    }
}

// manually implement debug because the bindgen impl doesn't include the deadline, which is very
// useful.
impl fmt::Debug for Task {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // SAFETY: The caller provided a valid non-null task to `post_task`, and is expected to keep
        // it alive until either it is successfully canceled or its callback is called.
        let async_task_t { deadline, handler, state } = unsafe { self.0.as_ref() };
        write!(
            f,
            "Task {{ async_task@{:?} {{ deadline: {deadline:?}, handler: {handler:?}, state: {state:?} }} }}",
            self.0,
        )
    }
}

impl DelayedTask {
    /// Returns the deadline of this task
    pub fn deadline(&self) -> MonotonicInstant {
        // SAFETY: The caller provided a valid non-null task to `post_task`, and is expected to keep
        // it alive until either it is successfully canceled or its callback is called.
        MonotonicInstant::from_nanos(unsafe { self.0.0.as_ref() }.deadline)
    }
}

/// We treat the task's ordering as reversed from chronological order so that the heap
/// pop operation will consider the 'oldest' item to be the next one.
impl Ord for DelayedTask {
    fn cmp(&self, other: &DelayedTask) -> cmp::Ordering {
        self.deadline().cmp(&other.deadline()).reverse()
    }
}

impl PartialOrd for DelayedTask {
    fn partial_cmp(&self, other: &DelayedTask) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Posts a task to run on or after its deadline following all posted
/// tasks with lesser or equal deadlines.
///
/// The task's handler will be invoked exactly once unless the task is canceled.
/// When the dispatcher is shutting down (being destroyed), the handlers of
/// all remaining tasks will be invoked with a status of |ZX_ERR_CANCELED|.
///
/// Dispatcher implementations must treat |ZX_TIME_INFINITE_PAST| as a sentinel
/// value meaning "no deadline". Tasks with expired deadlines must always be
/// processed before tasks without deadlines (those posted with
/// |ZX_TIME_INFINITE_PAST|).
///
/// Note: If there are always tasks with expired deadlines, tasks without
/// deadlines may be starved.
///
/// Returns |ZX_OK| if the task was successfully posted.
/// Returns |ZX_ERR_BAD_STATE| if the dispatcher is shutting down.
/// Returns |ZX_ERR_NOT_SUPPORTED| if not supported by the dispatcher.
///
/// This operation is thread-safe.
pub unsafe extern "C" fn post_task(
    dispatcher_ptr: *mut async_dispatcher_t,
    task_ptr: *mut async_task_t,
) -> zx_status_t {
    // Safety: The caller is responsible for ensuring this is only ever called on a dispatcher
    // object that was originally obtained from [`ScopeDispatcher::as_ptr`].
    let dispatcher = unsafe { ScopeDispatcher::from_ptr(dispatcher_ptr) };
    let task_ptr = NonNull::new(task_ptr).expect("invalid task pointer");
    let task = Task(task_ptr);

    if let Err(err) = dispatcher.post_task(task) {
        return err.into_raw();
    }
    ZX_OK
}

/// Cancels the task associated with |task|.
///
/// If successful, the task's handler will not run.
///
/// Returns |ZX_OK| if the task was pending and it has been successfully
/// canceled; its handler will not run again and can be released immediately.
/// Returns |ZX_ERR_NOT_FOUND| if there was no pending task either because it
/// already ran, had not been posted, or has been dequeued and is pending
/// execution (perhaps on another thread).
/// Returns |ZX_ERR_NOT_SUPPORTED| if not supported by the dispatcher.
///
/// This operation is thread-safe.
pub unsafe extern "C" fn cancel_task(
    dispatcher_ptr: *mut async_dispatcher_t,
    task_ptr: *mut async_task_t,
) -> zx_status_t {
    // Safety: The caller is responsible for ensuring this is only ever called on a dispatcher
    // object that was originally obtained from [`ScopeDispatcher::as_ptr`].
    let dispatcher = unsafe { ScopeDispatcher::from_ptr(dispatcher_ptr) };
    let task_ptr = NonNull::new(task_ptr).expect("invalid task pointer");
    let task = Task(task_ptr);

    if let Err(err) = dispatcher.cancel_task(task) {
        return err.into_raw();
    }
    ZX_OK
}

#[cfg(test)]
mod test {
    use core::iter::repeat_with;
    use core::task::Poll;
    use fuchsia_async::{MonotonicInstant, TestExecutor};
    use futures::channel::mpsc;
    use futures::{SinkExt, StreamExt};
    use libasync::DispatcherTimerExt;
    use libasync_dispatcher::OnDispatcher;
    use std::sync::Arc;

    use fuchsia_async::MonotonicDuration;
    use libasync_sys::async_state_t;

    use super::*;

    struct TestingTask(Arc<async_task_t>);

    impl TestingTask {
        fn new(
            deadline: MonotonicInstant,
            f: unsafe extern "C" fn(*mut async_dispatcher_t, *mut async_task_t, zx_status_t),
        ) -> Self {
            Self(Arc::new(async_task_t {
                handler: Some(f),
                deadline: deadline.into_nanos(),
                state: async_state_t { reserved: [0, 0] },
            }))
        }

        fn task(&self) -> Task {
            // SAFETY: We don't modify the task struct itself, only read, but NonNull only understands
            // *mut.
            Task(unsafe { NonNull::new_unchecked(&*self.0 as *const _ as *mut _) })
        }
    }

    impl fmt::Debug for TestingTask {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_tuple("TestingTask").field(&self.task()).finish()
        }
    }

    extern "C" fn never_call_task_fn(
        _: *mut async_dispatcher_t,
        _: *mut async_task_t,
        _: zx_status_t,
    ) {
        panic!("never call callback should never be called!")
    }

    #[test]
    fn task_queue_empty() {
        let infinite_future = TestingTask::new(MonotonicInstant::INFINITE, never_call_task_fn);
        let mut queue = TaskQueue::default();
        assert_eq!(queue.next_task(MonotonicInstant::INFINITE), None);
        assert_eq!(queue.peek_next_task(), None);
        assert_eq!(queue.take_pending_task(&infinite_future.task()), None);
    }

    #[test]
    fn only_infinite_future_task() {
        let infinite_future = TestingTask::new(MonotonicInstant::INFINITE, never_call_task_fn);
        let mut queue = TaskQueue::default();
        queue.queue_task(infinite_future.task());
        assert_eq!(queue.next_task(MonotonicInstant::INFINITE_PAST), None);
        assert_eq!(queue.peek_next_task(), Some(&infinite_future.task()));
        assert_eq!(queue.next_task(MonotonicInstant::from_nanos(0)), None);
        assert_eq!(queue.peek_next_task(), Some(&infinite_future.task()));
        assert_eq!(
            queue.next_task(MonotonicInstant::INFINITE).as_ref(),
            Some(&infinite_future.task())
        );
        assert_eq!(queue.peek_next_task(), None);

        queue.queue_task(infinite_future.task());
        assert_eq!(queue.peek_next_task(), Some(&infinite_future.task()));
        assert_eq!(
            queue.take_pending_task(&infinite_future.task()).as_ref(),
            Some(&infinite_future.task())
        );
    }

    #[test]
    fn some_tasks_with_different_times() {
        let mut now = MonotonicInstant::from_nanos(0);
        let step = MonotonicDuration::from_nanos(10);

        let do_it_when_you_can = Vec::from_iter(
            repeat_with(|| TestingTask::new(MonotonicInstant::INFINITE_PAST, never_call_task_fn))
                .take(5),
        );
        let do_it_now = TestingTask::new(now, never_call_task_fn);
        let do_it_soon = TestingTask::new(now + step, never_call_task_fn);
        let do_it_later = TestingTask::new(now + (step * 2), never_call_task_fn);
        let do_it_way_later = TestingTask::new(now + (step * 3), never_call_task_fn);
        let do_it_far_future = TestingTask::new(MonotonicInstant::INFINITE, never_call_task_fn);

        println!("{do_it_when_you_can:#?}");

        let mut queue = TaskQueue::default();
        queue.queue_task(do_it_when_you_can[0].task());
        queue.queue_task(do_it_now.task());
        queue.queue_task(do_it_soon.task());
        queue.queue_task(do_it_later.task());
        queue.queue_task(do_it_way_later.task());
        queue.queue_task(do_it_far_future.task());

        // the first task to dequeue should be the now task, because it's ready now. The do_it_when_you_can
        // task will defer to any ready tasks.
        assert_eq!(queue.next_task(now), Some(do_it_now.task()));

        // then we should get the do_it_when_you_can task as our next task because all other tasks are
        // still waiting for time to catch up to them.
        assert_eq!(queue.next_task(now), Some(do_it_when_you_can[0].task()));

        // then we should get nothing because we've exhausted available tasks.
        assert_eq!(queue.next_task(now), None);

        // we'll put the other two do_it_when_you_can tasks on now so they get picked up as we go
        queue.queue_task(do_it_when_you_can[1].task());
        queue.queue_task(do_it_when_you_can[2].task());
        queue.queue_task(do_it_when_you_can[3].task());
        queue.queue_task(do_it_when_you_can[4].task());

        // now that we've advanced the clock we should get do_it_soon.
        now += step;
        assert_eq!(queue.next_task(now), Some(do_it_soon.task()));

        // we'll cancel do_it_when_you_can[2] and make sure it doesn't come up later
        assert_eq!(
            queue.take_pending_task(&do_it_when_you_can[2].task()),
            Some(do_it_when_you_can[2].task())
        );

        // then we should get another of the do_it_when_you_cans because we've exhausted this time
        // step
        assert_eq!(queue.next_task(now), Some(do_it_when_you_can[1].task()));

        // and then we should get do_it_later after this time step
        now += step;
        assert_eq!(queue.next_task(now), Some(do_it_later.task()));

        // and then we should get the second-last do_it_when_you_can
        assert_eq!(queue.next_task(now), Some(do_it_when_you_can[3].task()));

        // next we'll cancel our do_it_way_later task so we won't get it while draining
        assert_eq!(queue.take_pending_task(&do_it_way_later.task()), Some(do_it_way_later.task()));

        // and now we'll drain the queue for both do_it_when_you_can and far_future tasks that we
        // would be cancelling at shutdown.
        now = MonotonicInstant::INFINITE;
        assert_eq!(queue.next_task(now), Some(do_it_far_future.task()));
        // note that it may be surprising that this one comes last, but that's because infinite past
        // tasks are always treated as lower priority than any with any timestamp.
        assert_eq!(queue.next_task(now), Some(do_it_when_you_can[4].task()));

        // and then nothing.
        assert_eq!(queue.next_task(now), None);
    }

    #[fuchsia::test]
    async fn test_spawn_task() {
        let scope_dispatcher = ScopeDispatcher::new();
        assert_eq!(scope_dispatcher.compute(async { 1 + 1 }).await, Ok(2));
        scope_dispatcher.shutdown().await;
    }

    async fn ping(mut tx: mpsc::Sender<u8>, mut rx: mpsc::Receiver<u8>) {
        println!("starting ping!");
        tx.send(0).await.unwrap();
        while let Some(next) = rx.next().await {
            println!("ping! {next}");
            tx.send(next + 1).await.unwrap();
        }
    }

    async fn pong(mut tx: mpsc::Sender<u8>, mut rx: mpsc::Receiver<u8>) {
        println!("starting pong!");
        while let Some(next) = rx.next().await {
            println!("pong! {next}");
            if next > 10 {
                println!("bye!");
                break;
            }
            tx.send(next + 1).await.unwrap();
        }
    }

    #[fuchsia::test]
    async fn async_ping_pong() {
        let scope_dispatcher = ScopeDispatcher::new();
        let (ping_tx, pong_rx) = mpsc::channel(10);
        let (pong_tx, ping_rx) = mpsc::channel(10);
        scope_dispatcher.spawn(ping(ping_tx, ping_rx));
        scope_dispatcher.spawn(pong(pong_tx, pong_rx)).await.unwrap();
        scope_dispatcher.shutdown().await;
    }

    #[test]
    fn timeouts() {
        let mut test_executor = TestExecutor::new_with_fake_time();
        test_executor.set_fake_time(MonotonicInstant::from_nanos(1000));
        let scope_dispatcher =
            ScopeDispatcher::new_on_executor(test_executor.global_handle().clone());

        async fn signal_finish_after_timeout(dispatcher: Arc<ScopeDispatcher>) {
            dispatcher.after_deadline(zx::MonotonicInstant::from_nanos(2000)).await.unwrap()
        }
        let mut completion =
            scope_dispatcher.spawn(signal_finish_after_timeout(scope_dispatcher.clone()));
        assert_eq!(test_executor.run_until_stalled(&mut completion), Poll::Pending);

        test_executor.set_fake_time(MonotonicInstant::from_nanos(2000));
        assert_eq!(test_executor.run_until_stalled(&mut completion), Poll::Ready(Ok(())));

        assert_eq!(
            test_executor.run_until_stalled(&mut scope_dispatcher.shutdown()),
            Poll::Ready(())
        );
    }

    #[test]
    fn shutdown_cancel() {
        let mut test_executor = TestExecutor::new();
        let scope_dispatcher =
            ScopeDispatcher::new_on_executor(test_executor.global_handle().clone());
        let mut fut = scope_dispatcher.after_deadline(zx::MonotonicInstant::INFINITE);
        assert_eq!(test_executor.run_until_stalled(&mut fut), Poll::Pending);
        assert_eq!(
            test_executor.run_until_stalled(&mut scope_dispatcher.shutdown()),
            Poll::Ready(())
        );
        assert_eq!(test_executor.run_until_stalled(&mut fut), Poll::Ready(Err(Status::CANCELED)));
    }
}
