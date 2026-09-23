// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use futures::stream::{FusedStream, FuturesUnordered, Stream};
use pin_project::pin_project;

/// A collection of multiple futures that optimizes for the single-future case.
///
/// Instances of `OneOrMany` can be created with `Default`, [`OneOrMany::new`],
/// or as the result of `.collect()`ing from an iterator.
#[pin_project]
pub struct OneOrMany<F>(#[pin] Impl<F>);

/// Maintains internal state for the [`Impl::One`] case, keeping track of when
/// `None` is already yielded to provide a correct `FusedFuture` implementation.
#[pin_project(project=OneInnerProj)]
enum OneInner<F> {
    Present(#[pin] F),
    Absent,
    AbsentNoneYielded,
}

impl<F> OneInner<F> {
    fn take(&mut self) -> Option<F> {
        let v = std::mem::replace(self, Self::Absent);
        match v {
            Self::Present(f) => Some(f),
            Self::Absent => None,
            Self::AbsentNoneYielded => {
                // Restore back the NoneYieldedState.
                *self = Self::AbsentNoneYielded;
                None
            }
        }
    }
}

#[pin_project(project=OneOrManyProj)]
enum Impl<F> {
    One(#[pin] OneInner<F>),
    Many(#[pin] FuturesUnordered<F>),
}

impl<F> Default for OneOrMany<F> {
    fn default() -> Self {
        Self(Impl::One(OneInner::Absent))
    }
}

impl<F> OneOrMany<F> {
    /// Constructs a `OneOrMany` with a single future.
    ///
    /// Constructs a new `OneOrMany` with exactly one future. If no additional
    /// futures are added via [`push`](OneOrMany::push), this is behaviorally
    /// identical to constructing a stream by providing the future to
    /// [`futures::stream::once`].
    pub fn new(f: F) -> Self {
        Self(Impl::One(OneInner::Present(f)))
    }

    /// Appends a new future to the set of pending futures.
    ///
    /// Like [`FuturesUnordered::push`], this doesn't call
    /// [`poll`](Future::poll) on the provided future. The caller must ensure
    /// that [`poll_next`](Stream::poll_next) is called in order to receive
    /// wake-up notifications for the provided future.
    pub fn push(&mut self, f: F) {
        let Self(this) = self;
        match this {
            Impl::One(o) => match o.take() {
                None => *o = OneInner::Present(f),
                Some(first) => *this = Impl::Many([first, f].into_iter().collect()),
            },
            Impl::Many(unordered) => {
                if unordered.is_empty() {
                    // Opportunistically switch back to `One`, but only if there
                    // are no more futures. This is expensive in the short term
                    // but more performant on average assuming most of the time
                    // this `OneOrMany` is holding zero or one futures.
                    // Otherwise the cost of allocating and deallocating a
                    // `FuturesUnordered` would outweigh the gains of less
                    // indirection.
                    *this = Impl::One(OneInner::Present(f))
                } else {
                    unordered.push(f);
                }
            }
        }
    }

    /// Returns true if and only if there are no futures held.
    pub fn is_empty(&self) -> bool {
        let Self(this) = self;
        match this {
            Impl::One(OneInner::Absent | OneInner::AbsentNoneYielded) => true,
            Impl::One(OneInner::Present(_)) => false,
            Impl::Many(many) => many.is_empty(),
        }
    }
}

impl<F: Future> Stream for OneOrMany<F> {
    type Item = F::Output;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        match this.0.project() {
            OneOrManyProj::One(mut p) => match p.as_mut().project() {
                OneInnerProj::Absent | OneInnerProj::AbsentNoneYielded => {
                    p.set(OneInner::AbsentNoneYielded);
                    Poll::Ready(None)
                }
                OneInnerProj::Present(f) => match f.poll(cx) {
                    Poll::Ready(t) => {
                        let output = Poll::Ready(Some(t));
                        p.set(OneInner::Absent);
                        output
                    }
                    Poll::Pending => Poll::Pending,
                },
            },
            OneOrManyProj::Many(unordered) => {
                // Instead of returning the value directly, we could check
                // whether `unordered` contains a single element and, if so,
                // return `this` to the `Impl::One` case. We avoid doing that
                // because it's unfriendly to an expected common pattern, where
                // the OneOrMany holds two futures and a new one is added every
                // time one of them completes (e.g. if the futures are
                // constructed from streams). Instead we implement a little bit
                // of hysteresis here and in the `Impl::Many` case in `push`. by
                // requiring `unordered` to be completely empty before reverting
                // to `Impl::One`.
                unordered.poll_next(cx)
            }
        }
    }
}

impl<F: Future> FusedStream for OneOrMany<F> {
    fn is_terminated(&self) -> bool {
        let Self(this) = self;
        match this {
            Impl::One(OneInner::Present(_) | OneInner::Absent) => false,
            Impl::One(OneInner::AbsentNoneYielded) => true,
            Impl::Many(unordered) => unordered.is_terminated(),
        }
    }
}

impl<F> FromIterator<F> for OneOrMany<F> {
    fn from_iter<T: IntoIterator<Item = F>>(iter: T) -> Self {
        let mut iter = iter.into_iter();

        Self(match iter.next() {
            None => Impl::One(OneInner::Absent),
            Some(first) => match iter.next() {
                None => Impl::One(OneInner::Present(first)),
                Some(second) => Impl::Many([first, second].into_iter().chain(iter).collect()),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::event::Event;
    use assert_matches::assert_matches;
    use futures::future::Ready;
    use futures::{StreamExt as _, pin_mut};
    use futures_test::task::new_count_waker;
    use std::task::Waker;

    use super::*;

    #[test]
    fn one_or_many_one() {
        let (waker, count) = new_count_waker();
        let mut context = Context::from_waker(&waker);

        let event = Event::new();
        let one_or_many = OneOrMany::new(event.wait());
        pin_mut!(one_or_many);

        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Pending);
        assert_eq!(event.signal(), true);
        assert_eq!(count, 1);

        assert_eq!(one_or_many.poll_next(&mut context), Poll::Ready(Some(())));
        assert_eq!(count, 1);
    }

    #[test]
    fn one_or_many_one_poll_exhausted() {
        let (waker, count) = new_count_waker();
        let mut context = Context::from_waker(&waker);

        let one_or_many = OneOrMany::new(futures::future::ready(()));
        pin_mut!(one_or_many);
        assert_eq!(one_or_many.is_terminated(), false);
        assert_eq!(one_or_many.is_empty(), false);

        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(())));
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(None));
        assert_eq!(count, 0);
        assert_eq!(one_or_many.is_terminated(), true);
        assert_eq!(one_or_many.is_empty(), true);
    }

    #[test]
    fn one_or_many_push_one() {
        let (waker, count) = new_count_waker();
        let mut context = Context::from_waker(&waker);

        let mut one_or_many = OneOrMany::new(futures::future::ready(()));
        one_or_many.push(futures::future::ready(()));
        pin_mut!(one_or_many);
        assert_eq!(one_or_many.is_terminated(), false);
        assert_eq!(one_or_many.is_empty(), false);

        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(())));
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(())));
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(None));
        assert_eq!(one_or_many.is_terminated(), true);
        assert_eq!(count, 0);
        assert_eq!(one_or_many.is_empty(), true);
    }

    #[test]
    fn one_or_many_push_one_after_poll() {
        let (waker, count) = new_count_waker();
        let mut context = Context::from_waker(&waker);

        let event = Event::new();
        let one_or_many = OneOrMany::new(event.wait());
        pin_mut!(one_or_many);
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Pending);
        assert_eq!(one_or_many.is_empty(), false);

        let other_event = Event::new();
        one_or_many.push(other_event.wait());

        assert_eq!(count, 0);
        assert_eq!(event.signal(), true);
        assert_eq!(count, 1);

        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(())));
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Pending);
        assert_eq!(one_or_many.is_empty(), false);
    }

    #[test]
    fn one_or_many_push_one_after_ready_before_poll() {
        let (waker, count) = new_count_waker();
        let mut context = Context::from_waker(&waker);

        let event = Event::new();
        let one_or_many = OneOrMany::new(event.wait());
        pin_mut!(one_or_many);
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Pending);

        assert_eq!(count, 0);
        assert_eq!(event.signal(), true);

        let other_event = Event::new();
        one_or_many.push(other_event.wait());
        assert_eq!(count, 1);

        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(())));
        assert_eq!(one_or_many.poll_next(&mut context), Poll::Pending);
    }

    #[test]
    fn one_or_many_one_exhausted_push() {
        let (waker, count) = new_count_waker();
        let mut context = Context::from_waker(&waker);

        let one_or_many = OneOrMany::new(futures::future::ready(1));
        pin_mut!(one_or_many);
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(1)));
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(None));
        assert_eq!(one_or_many.is_terminated(), true);

        one_or_many.push(futures::future::ready(2));
        assert_eq!(one_or_many.is_terminated(), false);
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(2)));
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(None));
        assert_eq!(one_or_many.is_terminated(), true);
        assert_eq!(count, 0);
    }

    #[test]
    fn one_or_many_many_exhausted_push() {
        let (waker, count) = new_count_waker();
        let mut context = Context::from_waker(&waker);

        let one_or_many: OneOrMany<_> = [1, 2].into_iter().map(futures::future::ready).collect();
        pin_mut!(one_or_many);

        let mut values = [(); 2].map(|()| {
            let poll = one_or_many.as_mut().poll_next(&mut context);
            assert_matches!(poll, Poll::Ready(Some(i)) => i)
        });
        values.sort();
        assert_eq!(values, [1, 2]);
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(None));
        assert_eq!(one_or_many.is_terminated(), true);

        one_or_many.push(futures::future::ready(3));
        assert_eq!(one_or_many.is_terminated(), false);

        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(3)));
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(None));
        assert_eq!(one_or_many.is_terminated(), true);
        assert_eq!(count, 0)
    }

    #[test]
    fn one_or_many_collect_none() {
        let waker = Waker::noop();
        let mut context = Context::from_waker(&waker);

        let one_or_many: OneOrMany<Ready<()>> = std::iter::empty().collect();
        pin_mut!(one_or_many);

        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(None));
        assert_eq!(one_or_many.is_empty(), true);
    }

    #[test]
    fn one_or_many_collect_one() {
        let waker = Waker::noop();
        let mut context = Context::from_waker(&waker);

        let one_or_many: OneOrMany<_> = std::iter::once(futures::future::ready(1)).collect();
        pin_mut!(one_or_many);

        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(1)));
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(None));
        assert_eq!(one_or_many.is_empty(), true);
    }

    #[test]
    fn one_or_many_collect_multiple() {
        let waker = Waker::noop();
        let mut context = Context::from_waker(&waker);

        let one_or_many: OneOrMany<_> =
            (1..=5).into_iter().map(|i| futures::future::ready(i)).collect();

        let fut = one_or_many.collect();
        pin_mut!(fut);
        let all: HashSet<_> = assert_matches!(fut.poll(&mut context), Poll::Ready(x) => x);
        assert_eq!(all, HashSet::from_iter(1..=5));
    }

    #[test]
    fn fused_stream() {
        let waker = futures_test::task::panic_waker();
        let mut context = Context::from_waker(&waker);

        let one_or_many = OneOrMany::<_>::default();
        pin_mut!(one_or_many);
        assert!(!one_or_many.is_terminated());
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(None));
        assert!(one_or_many.is_terminated());

        one_or_many.as_mut().push(futures::future::ready(()));
        assert!(!one_or_many.is_terminated());
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(())));
        assert!(!one_or_many.is_terminated());
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(None));
        assert!(one_or_many.is_terminated());

        // Do it again with two futures to test the FuturesUnordered passthrough
        // case.
        one_or_many.as_mut().push(futures::future::ready(()));
        assert!(!one_or_many.is_terminated());
        one_or_many.as_mut().push(futures::future::ready(()));
        assert!(!one_or_many.is_terminated());
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(())));
        assert!(!one_or_many.is_terminated());
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(Some(())));
        assert!(!one_or_many.is_terminated());
        assert_eq!(one_or_many.as_mut().poll_next(&mut context), Poll::Ready(None));
        assert!(one_or_many.is_terminated());
    }
}
