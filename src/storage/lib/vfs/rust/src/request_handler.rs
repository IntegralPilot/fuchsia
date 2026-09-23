// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::Stream;
use futures::future::BoxFuture;
use pin_project::pin_project;
use std::future::{Future, ready};
use std::ops::ControlFlow;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Trait for handling requests in a [`RequestListener`].
pub trait RequestHandler: Sized {
    /// The type of requests that this request handler handles.
    type Request;

    /// Called for each request in the stream or until [`ControlFlow::Break`] is
    /// returned.
    fn handle_request(
        self: Pin<&mut Self>,
        request: Self::Request,
    ) -> impl Future<Output = ControlFlow<()>> + Send;

    /// Called when the stream ends. Will not be called if [`RequestHandler::handle_request`]
    /// returns [`ControlFlow::Break`].
    fn stream_closed(self: Pin<&mut Self>) -> impl Future<Output = ()> + Send {
        ready(())
    }
}

// LINT.IfChange
enum RequestListenerState {
    /// Indicates that [`RequestListener::stream`] should be polled for more requests.
    PollStream,

    /// Holds the future returned from [`RequestHandler::handle_request`]. The 'static lifetime is a
    /// lie. This future holds a reference back to [`RequestListener::handler`] making
    /// [`RequestListener`] self-referential.
    RequestFuture(BoxFuture<'static, ControlFlow<()>>),

    /// Holds the future returned from [`RequestHandler::stream_closed`]. The 'static lifetime is a
    /// lie. This future holds a reference back to [`RequestListener::handler`] making
    /// [`RequestListener`] self-referential.
    CloseFuture(BoxFuture<'static, ()>),

    /// Indicates that there's nothing left to be done and [`RequestListener`] should return
    /// [`Poll::Ready`] from its future.
    Done,
}

/// A `Future` for handling requests in a `Stream`. Optimized to reduce memory usage while the
/// stream is idle.
#[pin_project(!Unpin)]
pub struct RequestListener<RS, RH> {
    #[pin]
    stream: RS,

    // `state` could hold a future that references `handler` so `state` must come before `handler`
    // so it gets dropped before `handler`.
    state: RequestListenerState,

    #[pin]
    handler: RH,
}
// LINT.ThenChange(//src/developer/debug/zxdb/console/commands/verb_async_backtrace.cc)

impl<RS, RH> RequestListener<RS, RH> {
    pub fn new(stream: RS, handler: RH) -> Self {
        Self { stream, state: RequestListenerState::PollStream, handler }
    }
}

impl<RS, RH, R> Future for RequestListener<RS, RH>
where
    RS: Stream<Item = R>,
    RH: RequestHandler<Request = R>,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        loop {
            let this = self.as_mut().project();
            *this.state = match this.state {
                RequestListenerState::PollStream => match this.stream.poll_next(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(None) => {
                        let close_future = Box::pin(this.handler.stream_closed());
                        // SAFETY: The future holds a reference to the handler making this a
                        // self-referential struct. This struct doesn't implement Unpin and the
                        // future is always dropped before the handler so transmuting the future's
                        // lifetime to static is safe.
                        let close_future = unsafe {
                            std::mem::transmute::<BoxFuture<'_, ()>, BoxFuture<'static, ()>>(
                                close_future,
                            )
                        };
                        RequestListenerState::CloseFuture(close_future)
                    }
                    Poll::Ready(Some(request)) => {
                        let request_future = Box::pin(this.handler.handle_request(request));
                        // SAFETY: The future holds a reference to the handler making this a
                        // self-referential struct. This struct doesn't implement Unpin and the
                        // future is always dropped before the handler so transmuting the future's
                        // lifetime to static is safe.
                        let request_future = unsafe {
                            std::mem::transmute::<
                                BoxFuture<'_, ControlFlow<()>>,
                                BoxFuture<'static, ControlFlow<()>>,
                            >(request_future)
                        };
                        RequestListenerState::RequestFuture(request_future)
                    }
                },
                RequestListenerState::RequestFuture(fut) => match fut.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(ControlFlow::Break(())) => RequestListenerState::Done,
                    Poll::Ready(ControlFlow::Continue(())) => RequestListenerState::PollStream,
                },
                RequestListenerState::CloseFuture(fut) => match fut.as_mut().poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(()) => RequestListenerState::Done,
                },
                RequestListenerState::Done => return Poll::Ready(()),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use futures::poll;
    use std::future::{pending, poll_fn};
    use std::pin::pin;

    enum Action {
        Continue,
        Stop,
        Pending,
    }

    struct Handler<'a> {
        request_responses: &'a [Action],
        close_responses: &'a [Poll<()>],
    }

    impl<'a> Handler<'a> {
        fn new(request_responses: &'a [Action], close_responses: &'a [Poll<()>]) -> Self {
            Self { request_responses, close_responses }
        }
    }

    impl RequestHandler for Handler<'_> {
        type Request = u8;
        fn handle_request(
            mut self: Pin<&mut Self>,
            _request: Self::Request,
        ) -> impl Future<Output = ControlFlow<()>> + Send {
            poll_fn(move |_| {
                let this = self.as_mut().get_mut();
                let (action, remaining) =
                    this.request_responses.split_first().expect("polled more times than expected");
                this.request_responses = remaining;
                match action {
                    Action::Continue => Poll::Ready(ControlFlow::Continue(())),
                    Action::Stop => Poll::Ready(ControlFlow::Break(())),
                    Action::Pending => Poll::Pending,
                }
            })
        }

        fn stream_closed(mut self: Pin<&mut Self>) -> impl Future<Output = ()> + Send {
            poll_fn(move |_| {
                let this = self.as_mut().get_mut();
                let (response, remaining) =
                    this.close_responses.split_first().expect("polled more times than expected");
                this.close_responses = remaining;
                *response
            })
        }
    }

    struct Requests<'a> {
        requests: &'a [Poll<u8>],
    }

    impl<'a> Requests<'a> {
        fn new(requests: &'a [Poll<u8>]) -> Self {
            Self { requests }
        }
    }

    impl Stream for Requests<'_> {
        type Item = u8;

        fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let this = self.get_mut();
            match this.requests.split_first() {
                None => Poll::Ready(None),
                Some((action, remaining)) => {
                    this.requests = remaining;
                    match action {
                        Poll::Pending => Poll::Pending,
                        Poll::Ready(request) => Poll::Ready(Some(*request)),
                    }
                }
            }
        }
    }

    #[fuchsia::test]
    async fn test_empty_stream() {
        let handler = Handler::new(&[], &[Poll::Ready(())]);
        let stream = Requests::new(&[]);
        let mut listener = pin!(RequestListener::new(stream, handler));

        listener.as_mut().await;

        assert!(listener.handler.close_responses.is_empty());
    }

    #[fuchsia::test]
    async fn test_stream_pending() {
        let handler = Handler::new(&[Action::Continue, Action::Continue], &[Poll::Ready(())]);
        let stream = Requests::new(&[Poll::Ready(1), Poll::Pending, Poll::Ready(3)]);
        let mut listener = pin!(RequestListener::new(stream, handler));

        assert_matches!(poll!(&mut listener), Poll::Pending);
        assert!(matches!(listener.state, RequestListenerState::PollStream));
        assert_matches!(poll!(&mut listener), Poll::Ready(()));

        assert!(listener.handler.request_responses.is_empty());
        assert!(listener.handler.close_responses.is_empty());
        assert!(listener.stream.requests.is_empty());
    }

    #[fuchsia::test]
    async fn test_handle_request_pending() {
        let handler = Handler::new(
            &[Action::Continue, Action::Pending, Action::Continue],
            &[Poll::Ready(())],
        );
        let stream = Requests::new(&[Poll::Ready(1), Poll::Ready(3)]);
        let mut listener = pin!(RequestListener::new(stream, handler));

        assert_matches!(poll!(&mut listener), Poll::Pending);
        assert!(matches!(listener.state, RequestListenerState::RequestFuture(_)));
        assert_matches!(poll!(&mut listener), Poll::Ready(()));

        assert!(listener.handler.request_responses.is_empty());
        assert!(listener.handler.close_responses.is_empty());
        assert!(listener.stream.requests.is_empty());
    }

    #[fuchsia::test]
    async fn test_stream_closed_pending() {
        let handler =
            Handler::new(&[Action::Continue, Action::Continue], &[Poll::Pending, Poll::Ready(())]);
        let stream = Requests::new(&[Poll::Ready(1), Poll::Ready(3)]);
        let mut listener = pin!(RequestListener::new(stream, handler));

        assert_matches!(poll!(&mut listener), Poll::Pending);
        assert!(matches!(listener.state, RequestListenerState::CloseFuture(_)));
        assert_matches!(poll!(&mut listener), Poll::Ready(()));

        assert!(listener.handler.request_responses.is_empty());
        assert!(listener.handler.close_responses.is_empty());
        assert!(listener.stream.requests.is_empty());
    }

    #[fuchsia::test]
    async fn test_stream_closed_called_when_stream_ends() {
        let handler = Handler::new(&[Action::Continue, Action::Continue], &[Poll::Ready(())]);
        let stream = Requests::new(&[Poll::Ready(1), Poll::Ready(2)]);
        let mut listener = pin!(RequestListener::new(stream, handler));

        (&mut listener).await;

        assert!(listener.handler.request_responses.is_empty());
        assert!(listener.handler.close_responses.is_empty());
        assert!(listener.stream.requests.is_empty());
        assert!(matches!(listener.state, RequestListenerState::Done));

        (&mut listener).await;
    }

    #[fuchsia::test]
    async fn test_stream_closed_not_called_when_request_handler_stops() {
        // No stream_closed calls are expected so the handler will panic if it's called.
        let handler = Handler::new(&[Action::Continue, Action::Stop], &[]);
        let stream = Requests::new(&[Poll::Ready(1), Poll::Ready(2), Poll::Ready(3)]);
        let mut listener = pin!(RequestListener::new(stream, handler));

        (&mut listener).await;

        assert!(listener.handler.request_responses.is_empty());
        assert_eq!(listener.stream.requests, &[Poll::Ready(3u8)]);
        assert!(matches!(listener.state, RequestListenerState::Done));
    }

    #[fuchsia::test]
    async fn test_poll_after_done() {
        let handler = Handler::new(&[Action::Continue], &[Poll::Ready(())]);
        let stream = Requests::new(&[Poll::Ready(1)]);
        let mut listener = pin!(RequestListener::new(stream, handler));

        (&mut listener).await;
        assert!(matches!(listener.state, RequestListenerState::Done));
        (&mut listener).await;
    }

    struct HandlerWithDropInFuture {
        future_dropped: bool,
    }
    impl Drop for HandlerWithDropInFuture {
        fn drop(&mut self) {
            assert!(self.future_dropped, "Handler dropped before future");
        }
    }

    struct UpdateHandlerOnDrop<'a> {
        handler: &'a mut HandlerWithDropInFuture,
    }

    impl Drop for UpdateHandlerOnDrop<'_> {
        fn drop(&mut self) {
            assert!(!self.handler.future_dropped);
            self.handler.future_dropped = true;
        }
    }

    impl RequestHandler for HandlerWithDropInFuture {
        type Request = u8;

        async fn handle_request(self: Pin<&mut Self>, _request: Self::Request) -> ControlFlow<()> {
            let _defer_drop = UpdateHandlerOnDrop { handler: self.get_mut() };
            pending().await
        }

        async fn stream_closed(self: Pin<&mut Self>) -> () {
            let _defer_drop = UpdateHandlerOnDrop { handler: self.get_mut() };
            pending().await
        }
    }

    #[fuchsia::test]
    async fn test_request_future_dropped_before_handler() {
        let handler = HandlerWithDropInFuture { future_dropped: false };
        let stream = Requests::new(&[Poll::Ready(1)]);
        let mut listener = pin!(RequestListener::new(stream, handler));

        assert_matches!(poll!(&mut listener), Poll::Pending);
        assert!(matches!(listener.state, RequestListenerState::RequestFuture(_)));
        assert!(!listener.handler.future_dropped);

        // The handler's drop impl will panic if it's dropped before the future.
    }

    #[fuchsia::test]
    async fn test_close_future_dropped_before_handler() {
        let handler = HandlerWithDropInFuture { future_dropped: false };
        let stream = Requests::new(&[]);
        let mut listener = pin!(RequestListener::new(stream, handler));

        assert_matches!(poll!(&mut listener), Poll::Pending);
        assert!(matches!(listener.state, RequestListenerState::CloseFuture(_)));
        assert!(!listener.handler.future_dropped);

        // The handler's drop impl will panic if it's dropped before the future.
    }
}
