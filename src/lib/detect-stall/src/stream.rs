// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Support for running FIDL request streams until stalled.

use fidl::endpoints::RequestStream;
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::channel::oneshot::{self, Receiver};
use futures::{Stream, StreamExt, ready};
use pin_project_lite::pin_project;
use std::future::Future as _;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use zx::MonotonicDuration;

/// [`until_stalled`] wraps a FIDL request stream of type [`RS`] into another
/// stream yielding the same requests, but could complete prematurely if it
/// has stalled, meaning:
///
/// - The underlying `request_stream` has no new messages.
/// - There are no pending FIDL replies.
/// - This condition has lasted for at least `debounce_interval`.
///
/// When that happens, the request stream will complete, and the returned future
/// will complete with the server endpoint which has been unbound from the
/// stream. The returned future will also complete if the request stream ended
/// on its own without stalling.
pub fn until_stalled<RS: RequestStream>(
    request_stream: RS,
    debounce_interval: MonotonicDuration,
) -> (impl StreamAndControlHandle<RS, <RS as Stream>::Item>, Receiver<Option<zx::Channel>>) {
    let (sender, receiver) = oneshot::channel();
    let stream = StallableRequestStream::new(request_stream, debounce_interval, move |channel| {
        let _ = sender.send(channel);
    });
    (stream, receiver)
}

/// Types that implement [`StreamAndControlHandle`] can stream out FIDL request
/// messages and vend out weak control handles which lets you manage the connection
/// and send events.
pub trait StreamAndControlHandle<RS, Item>: Stream<Item = Item> {
    /// Obtain a weak control handle. Different from [`RequestStream::control_handle`],
    /// the weak control handle will not prevent unbinding. You may hold on to the
    /// weak control handle and the request stream can still complete when stalled.
    fn control_handle(&self) -> WeakControlHandle<RS>;
}

pin_project! {
    /// The stream returned from [`until_stalled`].
    pub struct StallableRequestStream<RS, F> {
        stream: Arc<Mutex<Option<RS>>>,
        debounce_interval: MonotonicDuration,
        unbind_callback: Option<F>,
        #[pin]
        timer: Option<fasync::Timer>,
    }
}

impl<RS, F> StallableRequestStream<RS, F> {
    /// Creates a new stallable request stream that will send the channel via `unbind_callback` when
    /// stream is stalled.
    pub fn new(stream: RS, mut debounce_interval: MonotonicDuration, unbind_callback: F) -> Self {
        if debounce_interval < MonotonicDuration::from_millis(1) {
            debounce_interval = MonotonicDuration::from_millis(1);
        }
        Self {
            stream: Arc::new(Mutex::new(Some(stream))),
            debounce_interval,
            unbind_callback: Some(unbind_callback),
            timer: None,
        }
    }
}

impl<RS: RequestStream + Unpin, F: FnOnce(Option<zx::Channel>) + Unpin>
    StreamAndControlHandle<RS, RS::Item> for StallableRequestStream<RS, F>
{
    fn control_handle(&self) -> WeakControlHandle<RS> {
        WeakControlHandle { stream: Arc::downgrade(&self.stream) }
    }
}

pub struct WeakControlHandle<RS> {
    stream: Weak<Mutex<Option<RS>>>,
}

impl<RS> WeakControlHandle<RS>
where
    RS: RequestStream,
{
    /// If the server endpoint is not unbound, calls `user` function with the
    /// control handle and propagates the return value. Otherwise, returns `None`.
    ///
    /// Typically you can use it to send an event within the closure:
    ///
    /// ```
    /// let control_handle = stream.control_handle();
    /// let result = control_handle.use_control_handle(
    ///     |control_handle| control_handle.send_my_event());
    /// ```
    ///
    pub fn use_control_handle<User, R>(&self, user: User) -> Option<R>
    where
        User: FnOnce(RS::ControlHandle) -> R,
    {
        self.stream
            .upgrade()
            .as_ref()
            .map(|stream| stream.lock().as_ref().map(|stream| user(stream.control_handle())))
            .flatten()
    }
}

impl<RS: RequestStream + Unpin, F: FnOnce(Option<zx::Channel>) + Unpin> Stream
    for StallableRequestStream<RS, F>
{
    type Item = <RS as Stream>::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let poll_result = self
            .stream
            .as_ref()
            .lock()
            .as_mut()
            .expect("Stream already resolved")
            .poll_next_unpin(cx);
        let mut this = self.project();
        match poll_result {
            Poll::Ready(message) => {
                if message.is_none() {
                    this.unbind_callback.take().unwrap()(None);
                } else {
                    this.timer.set(Some(fasync::Timer::new(*this.debounce_interval)));
                }
                Poll::Ready(message)
            }
            Poll::Pending => {
                if this.timer.is_none() {
                    this.timer.set(Some(fasync::Timer::new(*this.debounce_interval)));
                }
                ready!(this.timer.as_mut().as_pin_mut().unwrap().poll(cx));
                this.timer.set(None);

                // Try and unbind, which will fail if there are outstanding responders or
                // control handles.
                let (inner, is_terminated) = this.stream.lock().take().unwrap().into_inner();
                match Arc::try_unwrap(inner) {
                    Ok(inner) => {
                        this.unbind_callback.take().unwrap()(Some(
                            inner.into_channel().into_zx_channel(),
                        ));
                        Poll::Ready(None)
                    }
                    Err(inner) => {
                        // We can't unbind because there are outstanding responders or control
                        // handles, so we'll try again after another debounce interval.
                        *this.stream.lock() = Some(RS::from_inner(inner, is_terminated));
                        this.timer.set(Some(fasync::Timer::new(*this.debounce_interval)));
                        if this.timer.as_mut().as_pin_mut().unwrap().poll(cx).is_ready()
                        {
                            cx.waker().wake_by_ref();
                        }
                        Poll::Pending
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fasync::TestExecutor;
    use fidl::endpoints::Proxy;
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use futures::{FutureExt, TryStreamExt};
    use std::pin::pin;

    #[fuchsia::test(allow_stalls = false)]
    async fn no_message() {
        let initial = fasync::MonotonicInstant::from_nanos(0);
        TestExecutor::advance_to(initial).await;
        const DURATION_NANOS: i64 = 1_000_000;
        let idle_duration = MonotonicDuration::from_nanos(DURATION_NANOS);

        let (_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fio::DirectoryMarker>();
        let (stream, stalled) = until_stalled(stream, idle_duration);
        let mut stream = pin!(stream);

        assert_matches!(
            futures::join!(
                stream.next(),
                TestExecutor::advance_to(initial + idle_duration).then(|()| stalled)
            ),
            (None, Ok(Some(_)))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn strong_control_handle_blocks_stalling() {
        let initial = fasync::MonotonicInstant::from_nanos(0);
        TestExecutor::advance_to(initial).await;
        const DURATION_NANOS: i64 = 1_000_000;
        let idle_duration = MonotonicDuration::from_nanos(DURATION_NANOS);

        let (_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fio::DirectoryMarker>();
        let (stream, mut stalled) = until_stalled(stream, idle_duration);

        let strong_control_handle: fio::DirectoryControlHandle =
            stream.control_handle().use_control_handle(|x| x).unwrap();

        // The connection does not stall, because there is `strong_control_handle`.
        TestExecutor::advance_to(initial + idle_duration * 2).await;
        let mut stream = pin!(stream.fuse());
        futures::select! {
            _ = stream.next() => unreachable!(),
            _ = stalled => unreachable!(),
            default => {},
        }

        // Once we drop it then the connection can stall.
        drop(strong_control_handle);
        assert_matches!(
            futures::join!(
                stream.next(),
                TestExecutor::advance_to(initial + idle_duration * 4).then(|()| stalled)
            ),
            (None, Ok(Some(_)))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn weak_control_handle() {
        let initial = fasync::MonotonicInstant::from_nanos(0);
        TestExecutor::advance_to(initial).await;
        const DURATION_NANOS: i64 = 1_000_000;
        let idle_duration = MonotonicDuration::from_nanos(DURATION_NANOS);

        let (_proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fio::DirectoryMarker>();
        let (stream, stalled) = until_stalled(stream, idle_duration);

        // Just getting a weak control handle should not block the connection from stalling.
        let weak_control_handle = stream.control_handle();

        let mut stream = pin!(stream);
        assert_matches!(
            futures::join!(
                stream.next(),
                TestExecutor::advance_to(initial + idle_duration).then(|()| stalled)
            ),
            (None, Ok(Some(_)))
        );

        weak_control_handle.use_control_handle(|_| unreachable!());
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn one_message() {
        let initial = fasync::MonotonicInstant::from_nanos(0);
        TestExecutor::advance_to(initial).await;
        const DURATION_NANOS: i64 = 1_000_000;
        let debounce_interval = MonotonicDuration::from_nanos(DURATION_NANOS);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fio::DirectoryMarker>();
        let (stream, stalled) = until_stalled(stream, debounce_interval);
        let mut stream = pin!(stream);
        let mut stalled = pin!(stalled);

        // The stream should have no messages.
        let mut message = pin!(stream.next());
        assert_matches!(TestExecutor::poll_until_stalled(&mut message).await, Poll::Pending);
        assert_matches!(TestExecutor::poll_until_stalled(&mut stalled).await, Poll::Pending);

        // Send a message to the stream and process it. The debounce timer resets.
        TestExecutor::advance_to(initial + debounce_interval / 2).await;
        let _ = proxy.get_flags();
        assert_matches!(
            TestExecutor::poll_until_stalled(&mut message).await,
            Poll::Ready(Some(Ok(fio::DirectoryRequest::GetFlags { responder }))) => {
                // Reply to the request so that the stream doesn't have any pending replies.
                responder.send(Ok(fio::Flags::empty())).unwrap();
            }
        );
        assert_matches!(TestExecutor::poll_until_stalled(&mut stalled).await, Poll::Pending);

        // The stream should remain open after the old debounce interval but before the new debounce
        // interval.
        TestExecutor::advance_to(initial + debounce_interval).await;
        let mut message = pin!(stream.next());
        assert_matches!(TestExecutor::poll_until_stalled(&mut message).await, Poll::Pending);
        assert_matches!(TestExecutor::poll_until_stalled(&mut stalled).await, Poll::Pending);

        // The stream should close after the new debounce interval.
        TestExecutor::advance_to(initial + debounce_interval / 2 + debounce_interval).await;
        assert_matches!(message.await, None);
        assert_matches!(stalled.await, Ok(Some(_)));
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn pending_reply_blocks_stalling() {
        let initial = fasync::MonotonicInstant::from_nanos(0);
        TestExecutor::advance_to(initial).await;
        const DURATION_NANOS: i64 = 1_000_000;
        let idle_duration = MonotonicDuration::from_nanos(DURATION_NANOS);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fio::DirectoryMarker>();
        let (stream, mut stalled) = until_stalled(stream, idle_duration);
        let mut stream = pin!(stream.fuse());

        let _ = proxy.get_flags();

        // Do not reply to the request, but hold on to the responder, so that there is a
        // pending reply in the connection.
        let message_with_pending_reply = stream.next().await.unwrap();
        let Ok(fio::DirectoryRequest::GetFlags { responder, .. }) = message_with_pending_reply
        else {
            panic!("Unexpected {message_with_pending_reply:?}");
        };

        // The connection does not stall, because there is a pending reply.
        TestExecutor::advance_to(initial + idle_duration * 2).await;
        futures::select! {
            _ = stream.next() => unreachable!(),
            _ = stalled => unreachable!(),
            default => {},
        }

        // Now we resolve the pending reply.
        responder.send(Ok(fio::Flags::empty())).unwrap();

        // The connection should stall.
        assert_matches!(
            futures::join!(
                stream.next(),
                TestExecutor::advance_to(initial + idle_duration * 3).then(|()| stalled)
            ),
            (None, Ok(Some(_)))
        );
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn completed_stream() {
        let initial = fasync::MonotonicInstant::from_nanos(0);
        TestExecutor::advance_to(initial).await;
        const DURATION_NANOS: i64 = 1_000_000;
        let idle_duration = MonotonicDuration::from_nanos(DURATION_NANOS);

        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<fio::DirectoryMarker>();
        let (stream, stalled) = until_stalled(stream, idle_duration);

        let mut stalled = pin!(stalled);
        assert_matches!(TestExecutor::poll_until_stalled(&mut stalled).await, Poll::Pending);

        // Close the proxy such that the stream completes.
        drop(proxy);

        let mut stream = pin!(stream);

        {
            // Read the `None` from the stream.
            assert_matches!(stream.next().await, None);

            // In practice the async tasks reading from the stream will exit, thus
            // dropping the stream. We'll emulate that here.
            drop(stream);
        }

        // Now the future should finish with `None` because the connection has
        // terminated without stalling.
        assert_matches!(stalled.await, Ok(None));
    }

    /// Simulate what would happen when a component serves a FIDL stream that's been
    /// wrapped in `until_stalled`, and thus will complete and give the unbound channel
    /// back to the user, who can then pass it back to `component_manager` in practice.
    #[fuchsia::test(allow_stalls = false)]
    async fn end_to_end() {
        let initial = fasync::MonotonicInstant::from_nanos(0);
        TestExecutor::advance_to(initial).await;
        use fidl_fuchsia_component_client_test::{ProtocolAMarker, ProtocolARequest};

        const DURATION_NANOS: i64 = 40_000_000;
        let idle_duration = MonotonicDuration::from_nanos(DURATION_NANOS);
        let (proxy, stream) = fidl::endpoints::create_proxy_and_stream::<ProtocolAMarker>();
        let (stream, stalled) = until_stalled(stream, idle_duration);

        // Launch a task that serves the stream.
        let task = fasync::Task::spawn(async move {
            let mut stream = pin!(stream);
            while let Some(request) = stream.try_next().await.unwrap() {
                match request {
                    ProtocolARequest::Foo { responder } => responder.send().unwrap(),
                }
            }
        });

        // Launch another task to await the `stalled` future and also let us
        // check for it synchronously.
        let stalled = fasync::Task::spawn(stalled).map(Arc::new).shared();

        // Make some requests at intervals half the idle duration. Stall should not happen.
        let request_duration = MonotonicDuration::from_nanos(DURATION_NANOS / 2);
        const NUM_REQUESTS: usize = 5;
        let mut deadline = initial;
        for _ in 0..NUM_REQUESTS {
            proxy.foo().await.unwrap();
            deadline += request_duration;
            TestExecutor::advance_to(deadline).await;
            assert!(stalled.clone().now_or_never().is_none());
        }

        // Wait for stalling.
        deadline += idle_duration;
        TestExecutor::advance_to(deadline).await;
        let server_end = stalled.await;

        // Ensure the server task can stop (by observing the completed stream).
        task.await;

        // Check that this channel was the original server endpoint.
        let client = proxy.into_channel().unwrap().into_zx_channel();
        assert_eq!(
            client.basic_info().unwrap().koid,
            (*server_end).as_ref().unwrap().as_ref().unwrap().basic_info().unwrap().related_koid
        );
    }
}
