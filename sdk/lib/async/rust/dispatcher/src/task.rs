// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Safe bindings for the C libasync async dispatcher library

use core::task::Context;
use fuchsia_sync::Mutex;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::task::{Poll, Wake, Waker};

use zx_status::Status;

use futures::future::{BoxFuture, FutureExt};
use futures::task::AtomicWaker;

use crate::{AsAsyncDispatcherRef, AsyncDispatcher};

/// The future returned by [`crate::OnDispatcher::compute`]. If this is dropped, the task will be
/// cancelled.
#[must_use]
#[derive(Debug)]
pub struct Task<T> {
    state: Arc<TaskFutureState>,
    result_receiver: mpsc::Receiver<Result<T, Status>>,
    detached: bool,
}

impl<T: Send + 'static> Task<T> {
    fn new(
        future: impl Future<Output = T> + Send + 'static,
        dispatcher: AsyncDispatcher,
    ) -> (Self, Arc<TaskWakerState<T>>) {
        let future_state = Arc::new(TaskFutureState {
            waker: AtomicWaker::new(),
            aborted: AtomicBool::new(false),
        });
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        let state = Arc::new(TaskWakerState {
            result_sender,
            future_state: future_state.clone(),
            future: Mutex::new(Some(future.boxed())),
            dispatcher,
        });
        let future = Task { state: future_state, result_receiver, detached: false };
        (future, state)
    }

    /// Constructs a task that never runs, but immediately fails with the given error status.
    pub fn new_failed(status: Status) -> Self {
        let state = Arc::new(TaskFutureState {
            waker: AtomicWaker::new(),
            aborted: AtomicBool::new(false),
        });
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        // send the error to the result receiver. This should never fail, since
        // we just created both ends.
        result_sender.try_send(Err(status)).unwrap();
        Task { state, result_receiver, detached: false }
    }

    pub(crate) fn start(
        future: impl Future<Output = T> + Send + 'static,
        dispatcher: AsyncDispatcher,
    ) -> Self {
        let (future, state) = Self::new(future, dispatcher);

        // try to queue the task and if it fails short circuit the delivery of failure to the
        // caller.
        if let Err(err) = state.queue() {
            // drop the future we were given
            drop(state.future.lock().take());
            // send the error to the result receiver. This should never fail, since
            // we just created both ends and the task queuing failed.
            state.result_sender.try_send(Err(err)).unwrap();
        }

        future
    }
}

impl<T> Task<T> {
    /// Detaches this future from the task so that it will continue executing without waiting
    /// on the future. If this is not called, and the future is dropped, the task will be aborted
    /// the next time it is awoken.
    pub fn detach(self) {
        drop(self.detach_on_drop());
    }

    /// Detaches this future from the task so that it will continue executing without waiting
    /// on the future. If this is not called, and the future is dropped, the task will be aborted
    /// the next time it is awoken.
    ///
    /// Returns a future that can be awaited on or dropped without affecting the task.
    pub fn detach_on_drop(mut self) -> JoinHandle<T> {
        self.detached = true;
        JoinHandle(self)
    }

    /// Aborts the task and returns a future that can be used to wait for the task to either
    /// complete or cancel. If the task was canceled the result of the future will be
    /// [`Status::CANCELED`].
    pub fn abort(&self) {
        self.state.aborted.store(true, Ordering::Relaxed);
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        if !self.detached {
            self.state.aborted.store(true, Ordering::Relaxed);
        }
    }
}

#[derive(Debug)]
struct TaskFutureState {
    waker: AtomicWaker,
    aborted: AtomicBool,
}

impl<T> Future for Task<T> {
    type Output = Result<T, Status>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        use std::sync::mpsc::TryRecvError;
        self.state.waker.register(cx.waker());
        match self.result_receiver.try_recv() {
            Ok(res) => Poll::Ready(res),
            Err(TryRecvError::Disconnected) => Poll::Ready(Err(Status::CANCELED)),
            Err(TryRecvError::Empty) => Poll::Pending,
        }
    }
}

/// A handle for a task that will detach on drop. Returned by [`OnDispatcher::spawn`].
#[derive(Debug)]
pub struct JoinHandle<T>(Task<T>);

impl<T> JoinHandle<T> {
    /// Aborts the task and returns a future that can be used to wait for the task to either
    /// complete or cancel. If the task was canceled the result of the future will be
    /// [`Status::CANCELED`].
    pub fn abort(&self) {
        self.0.abort()
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, Status>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.poll_unpin(cx)
    }
}

struct TaskWakerState<T> {
    result_sender: mpsc::SyncSender<Result<T, Status>>,
    future_state: Arc<TaskFutureState>,
    future: Mutex<Option<BoxFuture<'static, T>>>,
    dispatcher: AsyncDispatcher,
}

impl<T: Send + 'static> Wake for TaskWakerState<T> {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        match self.queue() {
            Err(e) if e == Status::BAD_STATE => {
                // the dispatcher is shutting down so drop the future, if there
                // is one, to cancel it.
                let future_slot = self.future.lock().take();
                drop(future_slot);
                self.send_result(Err(e));
            }
            res => res.expect("Unexpected error waking dispatcher task"),
        }
    }
}

impl<T: Send + 'static> TaskWakerState<T> {
    /// Sends the result to the future end of this task, if it still exists.
    fn send_result(&self, res: Result<T, Status>) {
        // send the result and wake the waker if any has been registered.
        // We ignore the result here because if the other end has dropped it's
        // fine for the result to go nowhere.
        self.result_sender.try_send(res).ok();
        self.future_state.waker.wake();
    }

    /// Posts a task to progress the currently stored future. The task will
    /// consume the future if the future is ready after the next poll.
    /// Otherwise, the future is kept to be polled again after being woken.
    pub(crate) fn queue(self: &Arc<Self>) -> Result<(), Status> {
        let arc_self = self.clone();
        self.dispatcher
            .post_task_sync(move |status| {
                let mut future_slot = arc_self.future.lock();
                // if the executor is shutting down, drop the future we're waiting on and pass
                // on the error.
                if let Err(status) = status {
                    drop(future_slot.take());
                    arc_self.send_result(Err(status));
                    return;
                }

                // if the future has been dropped without being detached, drop the future and
                // send an Err(Status::CANCELED) if the caller is still listening.
                if arc_self.future_state.aborted.load(Ordering::Relaxed) {
                    drop(future_slot.take());
                    arc_self.send_result(Err(Status::CANCELED));
                    return;
                }

                let Some(mut future) = future_slot.take() else {
                    return;
                };
                let waker = Waker::from(arc_self.clone());
                let context = &mut Context::from_waker(&waker);
                match future.as_mut().poll(context) {
                    Poll::Pending => *future_slot = Some(future),
                    Poll::Ready(res) => {
                        arc_self.send_result(Ok(res));
                    }
                }
            })
            .map(|_| ())
    }
}
