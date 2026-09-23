// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::client::QueryResponseFut;
use fidl::encoding::ResourceDialect;
use futures::future::{FusedFuture as _, FutureExt as _, MaybeDone};
use futures::stream::{FusedStream, Stream};
use futures::task::{Context, Poll};
use pin_project::pin_project;
use std::pin::Pin;

/// HangingGetStream is a [`Stream`] that is oriented towards being a client to the
/// "Hanging Get" design pattern for flow control as in //docs/development/api/fidl.md#flow_control
#[must_use = "streams do nothing unless polled"]
#[pin_project]
pub struct HangingGetStream<
    P,
    O,
    D: ResourceDialect = fidl::encoding::DefaultFuchsiaResourceDialect,
    Q = fn(&P) -> QueryResponseFut<O, D>,
> {
    proxy: P,
    query: Q,
    response: QueryResponseFut<O, D>,
    eager: bool,
    _dialect: std::marker::PhantomData<D>,
}

impl<P, O, D, Q> Stream for HangingGetStream<P, O, D, Q>
where
    O: Unpin,
    Q: FnMut(&P) -> QueryResponseFut<O, D>,
    D: ResourceDialect,
{
    type Item = fidl::Result<O>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        if *this.eager {
            match this.response.poll_unpin(cx) {
                Poll::Ready(o) => {
                    let QueryResponseFut(prev_response) =
                        std::mem::replace(this.response, (&mut this.query)(&this.proxy));

                    if cfg!(debug_assertions) {
                        match prev_response {
                            MaybeDone::Gone => {}
                            MaybeDone::Future(_) => {
                                panic!("previous uncompleted future still exists")
                            }
                            MaybeDone::Done(_) => {
                                panic!("previous completed future's result not taken")
                            }
                        }
                    } else {
                        let _: MaybeDone<_> = prev_response;
                    }

                    Poll::Ready(Some(o))
                }
                Poll::Pending => Poll::Pending,
            }
        } else {
            if this.response.is_terminated() {
                *this.response = (&mut this.query)(&this.proxy);
            }
            this.response.poll_unpin(cx).map(Some)
        }
    }
}

impl<P, O, D, Q> FusedStream for HangingGetStream<P, O, D, Q>
where
    O: Unpin,
    Q: FnMut(&P) -> QueryResponseFut<O, D>,
    D: ResourceDialect,
{
    fn is_terminated(&self) -> bool {
        false
    }
}

impl<P, O, D, Q> HangingGetStream<P, O, D, Q>
where
    Q: FnMut(&P) -> QueryResponseFut<O, D>,
    O: Unpin,
    D: ResourceDialect,
{
    fn new_inner(proxy: P, mut query: Q, eager: bool) -> Self {
        let response = if eager { query(&proxy) } else { QueryResponseFut(MaybeDone::Gone) };

        Self { proxy, query, response, eager, _dialect: std::marker::PhantomData }
    }

    /// Creates a new lazily-polled hanging-get stream.
    pub fn new(proxy: P, query: Q) -> Self {
        Self::new_inner(proxy, query, false /* eager */)
    }

    /// Creates a new eagerly-polled hanging-get stream.
    pub fn new_eager(proxy: P, query: Q) -> Self {
        Self::new_inner(proxy, query, true /* eager */)
    }
}

impl<P, O, D> HangingGetStream<P, O, D, fn(&P) -> QueryResponseFut<O, D>>
where
    O: Unpin,
    D: ResourceDialect,
{
    /// Creates a new lazily-polled hanging-get stream with a function pointer.
    pub fn new_with_fn_ptr(proxy: P, query: fn(&P) -> QueryResponseFut<O, D>) -> Self {
        Self::new(proxy, query)
    }

    /// Creates a new eagerly-polled hanging-get stream with a function pointer.
    pub fn new_eager_with_fn_ptr(proxy: P, query: fn(&P) -> QueryResponseFut<O, D>) -> Self {
        Self::new_eager(proxy, query)
    }
}

#[cfg(test)]
mod tests {

    use super::*;
    use fuchsia_async as fasync;
    use futures::TryStreamExt as _;
    use std::cell::Cell;

    struct TestProxy {
        state: Cell<usize>,
    }

    impl TestProxy {
        fn watch(&self) -> QueryResponseFut<usize> {
            let cur = self.state.get();
            self.state.set(cur + 1);
            QueryResponseFut(MaybeDone::Done(Ok(cur)))
        }
    }

    #[fasync::run_singlethreaded(test)]
    async fn generates_items_lazily() {
        let proxy = TestProxy { state: Cell::new(0) };
        let mut watcher = HangingGetStream::new(proxy, TestProxy::watch);

        const ITERS: usize = 3;
        for i in 0..ITERS {
            assert!(watcher.response.is_terminated());
            assert_eq!(watcher.try_next().await.expect("failed to get next item"), Some(i));
        }
    }

    #[fasync::run_singlethreaded(test)]
    async fn generates_items_eagerly() {
        let proxy = TestProxy { state: Cell::new(0) };
        let mut watcher = HangingGetStream::new_eager_with_fn_ptr(proxy, TestProxy::watch);

        const ITERS: usize = 3;
        for i in 0..ITERS {
            assert!(
                !watcher.response.is_terminated(),
                "should keep the server hydrated with an in-flight request",
            );
            assert_eq!(watcher.try_next().await.expect("failed to get next item"), Some(i));
        }
    }
}
