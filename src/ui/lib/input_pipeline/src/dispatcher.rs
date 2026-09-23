// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::task::Context;
use fidl_next::{ClientEnd, ServerEnd};
use futures::prelude::*;
use futures::task::Poll;
use pin_project_lite::pin_project;
use std::pin::Pin;

#[cfg(feature = "dso")]
pub use dso::*;

#[cfg(not(feature = "dso"))]
pub use elf::*;

pin_project! {
    #[derive(Debug)]
    #[must_use = "futures do nothing unless polled"]
    pub struct OnTimeout<F, T, OT> {
        #[pin]
        timer: T,
        #[pin]
        future: F,
        on_timeout: Option<OT>,
    }
}

impl<F: Future, T, OT> Future for OnTimeout<F, T, OT>
where
    T: Future<Output = ()> + 'static,
    OT: FnOnce() -> F::Output,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        if let Poll::Ready(item) = this.future.poll(cx) {
            return Poll::Ready(item);
        }
        if let Poll::Ready(()) = this.timer.poll(cx) {
            let ot = this.on_timeout.take().expect("polled with timeout after completion");
            let item = (ot)();
            return Poll::Ready(item);
        }
        Poll::Pending
    }
}

/// A wrapper for a future which will complete with a provided closure when a timeout occurs. This
/// is forked from [`fuchsia_async::OnTimeout`] because that has a fixed dependency on
/// [`fuchsia_async::Timer`] which driver dispatcher does not support.
pub trait TimeoutExt: Future + Sized {
    fn on_timeout<T, OT>(self, timer: T, on_timeout: OT) -> OnTimeout<Self, T, OT>
    where
        T: Future<Output = ()> + 'static,
        OT: FnOnce() -> Self::Output,
    {
        OnTimeout { timer, future: self, on_timeout: Some(on_timeout) }
    }
}

impl<F: Future + Sized> TimeoutExt for F {}

#[derive(Clone, Default)]
pub struct Dispatcher {}

mod dso {
    #![cfg(feature = "dso")]

    pub use super::*;
    use fdf::{AsyncDispatcher, OnDriverDispatcher};
    use libasync::DispatcherTimerExt;

    #[derive(Debug, Clone, Copy, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
    #[repr(transparent)]
    pub struct MonotonicInstant(zx::MonotonicInstant);

    impl From<zx::MonotonicInstant> for MonotonicInstant {
        fn from(o: zx::MonotonicInstant) -> Self {
            Self(o)
        }
    }

    impl From<MonotonicInstant> for zx::MonotonicInstant {
        fn from(o: MonotonicInstant) -> Self {
            o.0
        }
    }

    impl MonotonicInstant {
        pub fn now() -> Self {
            Self(zx::MonotonicInstant::get())
        }

        pub fn into_nanos(&self) -> i64 {
            self.0.into_nanos()
        }

        pub fn into_zx(self) -> zx::MonotonicInstant {
            self.0
        }

        pub fn after(duration: zx::MonotonicDuration) -> Self {
            Self(zx::MonotonicInstant::after(duration))
        }
    }

    pub type Transport = libasync_fidl::AsyncChannel<Dispatcher>;
    pub type DriverTransport = fdf_fidl::DriverChannel<fdf::CurrentDispatcher>;

    #[derive(Debug)]
    enum TaskHandleInner<T> {
        Join(::libasync::JoinHandle<T>),
        Local(::libasync::JoinHandle<()>, std::rc::Rc<std::cell::RefCell<Option<T>>>),
    }

    #[derive(Debug)]
    pub struct TaskHandle<T> {
        inner: Option<TaskHandleInner<T>>,
        detached: bool,
    }

    impl<T> Drop for TaskHandle<T> {
        fn drop(&mut self) {
            if !self.detached {
                if let Some(inner) = self.inner.as_mut() {
                    match inner {
                        TaskHandleInner::Join(h) => {
                            _ = h.abort();
                        }
                        TaskHandleInner::Local(h, _) => {
                            _ = h.abort();
                        }
                    }
                }
            }
        }
    }

    impl TaskHandle<()> {
        pub fn detach(mut self) {
            self.detached = true;
        }
    }

    impl<T: 'static> Future for TaskHandle<T> {
        type Output = T;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            match self.inner.as_mut().unwrap() {
                TaskHandleInner::Join(h) => match h.poll_unpin(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Ok(t)) => Poll::Ready(t),
                    Poll::Ready(Err(e)) => panic!("TaskHandle: polled unexpected error {e:?}"),
                },
                TaskHandleInner::Local(h, result) => match h.poll_unpin(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Ok(())) => {
                        let res = result
                            .borrow_mut()
                            .take()
                            .expect("TaskHandle completed but result missing");
                        Poll::Ready(res)
                    }
                    Poll::Ready(Err(e)) => panic!("TaskHandle: polled unexpected error {e:?}"),
                },
            }
        }
    }

    impl Dispatcher {
        #[must_use]
        pub fn spawn_local(future: impl Future<Output = ()> + 'static) -> TaskHandle<()>
        where
            Self: 'static,
        {
            // This should never panic if the dispatcher is valid.
            TaskHandle {
                inner: Some(TaskHandleInner::Join(fdf::CurrentDispatcher.spawn_local(future))),
                detached: false,
            }
        }

        pub fn after_deadline(deadline: MonotonicInstant) -> impl Future<Output = ()> + 'static {
            let f = fdf::CurrentDispatcher.after_deadline(deadline.into());
            async move {
                // This should never panic if the dispatcher is valid.
                f.await.expect("Dispatcher::after_deadline");
            }
        }

        pub fn client_from_zx_channel<P>(
            client_end: ClientEnd<P, zx::Channel>,
        ) -> ClientEnd<P, Transport> {
            libasync_fidl::AsyncChannel::<Dispatcher>::client_from_zx_channel(client_end)
        }
        pub fn server_from_zx_channel<P>(
            server_end: ServerEnd<P, zx::Channel>,
        ) -> ServerEnd<P, Transport> {
            libasync_fidl::AsyncChannel::<Dispatcher>::server_from_zx_channel(server_end)
        }
    }

    #[derive(Clone, Copy, Default, Debug)]
    pub struct LocalDriverExecutor;

    impl fidl_next::Executor for LocalDriverExecutor {
        type JoinHandle<T: 'static> = TaskHandle<T>;

        fn spawn<F>(&self, future: F) -> Self::JoinHandle<F::Output>
        where
            F: Future + Send + 'static,
            F::Output: Send + 'static,
        {
            use fdf::OnDispatcher;
            TaskHandle {
                inner: Some(TaskHandleInner::Join(
                    fdf::CurrentDispatcher.compute(future).detach_on_drop(),
                )),
                detached: false,
            }
        }
    }

    impl fidl_next::LocalExecutor for LocalDriverExecutor {
        fn spawn_local<F>(&self, future: F) -> Self::JoinHandle<F::Output>
        where
            F: Future + 'static,
            F::Output: 'static,
        {
            use fdf::OnDriverDispatcher;
            let result = std::rc::Rc::new(std::cell::RefCell::new(None));
            let result_clone = result.clone();
            let handle = fdf::CurrentDispatcher.spawn_local(async move {
                *result_clone.borrow_mut() = Some(future.await);
            });
            TaskHandle { inner: Some(TaskHandleInner::Local(handle, result)), detached: false }
        }
    }

    impl fidl_next::RunsTransport<Transport> for LocalDriverExecutor {}

    impl fdf::GetAsyncDispatcher for Dispatcher {
        fn try_get_async_dispatcher(&self) -> Option<AsyncDispatcher> {
            fdf::CurrentDispatcher.try_get_async_dispatcher()
        }
    }
}

mod elf {
    #![cfg(not(feature = "dso"))]

    pub use super::*;

    pub type MonotonicInstant = fuchsia_async::MonotonicInstant;

    pub type Transport = zx::Channel;

    #[derive(Debug)]
    pub struct TaskHandle<T>(fuchsia_async::Task<T>);

    impl TaskHandle<()> {
        pub fn detach(self) {
            self.0.detach();
        }
    }

    #[cfg(test)]
    impl<T: 'static> From<fuchsia_async::Task<T>> for TaskHandle<T> {
        fn from(task: fuchsia_async::Task<T>) -> Self {
            Self(task)
        }
    }

    impl<T: 'static> Future for TaskHandle<T> {
        type Output = T;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            match self.0.poll_unpin(cx) {
                Poll::Ready(t) => Poll::Ready(t),
                Poll::Pending => Poll::Pending,
            }
        }
    }

    impl Dispatcher {
        #[must_use]
        pub fn spawn_local(future: impl Future<Output = ()> + 'static) -> TaskHandle<()>
        where
            Self: 'static,
        {
            TaskHandle(fuchsia_async::Task::local(future))
        }

        pub fn after_deadline(deadline: MonotonicInstant) -> impl Future<Output = ()> + 'static {
            fuchsia_async::Timer::new(deadline)
        }

        pub fn client_from_zx_channel<P>(
            client_end: fidl_next::ClientEnd<P, zx::Channel>,
        ) -> ClientEnd<P, Transport> {
            client_end
        }
        pub fn server_from_zx_channel<P>(
            server_end: ServerEnd<P, zx::Channel>,
        ) -> ServerEnd<P, Transport> {
            server_end
        }
    }

    #[derive(Clone, Copy, Default, Debug)]
    pub struct LocalDriverExecutor;

    impl fidl_next::Executor for LocalDriverExecutor {
        type JoinHandle<T: 'static> = TaskHandle<T>;

        fn spawn<F>(&self, future: F) -> Self::JoinHandle<F::Output>
        where
            F: Future + Send + 'static,
            F::Output: Send + 'static,
        {
            use fidl_next::LocalExecutor;
            self.spawn_local(future)
        }
    }

    impl fidl_next::LocalExecutor for LocalDriverExecutor {
        fn spawn_local<F>(&self, future: F) -> Self::JoinHandle<F::Output>
        where
            F: Future + 'static,
            F::Output: 'static,
        {
            TaskHandle(fuchsia_async::Task::local(future))
        }
    }

    impl fidl_next::RunsTransport<Transport> for LocalDriverExecutor {}
}
