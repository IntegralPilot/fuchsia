// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#![allow(unused)]

use futures::Stream;
use futures::stream::FusedStream;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// A struct for matching pairs of values that may become available in any
/// order.  The two values are called left and right. Clients may enqueue lefts
/// and rights in any order, and then poll the Stream implementation for this
/// struct.  The stream implementation checks to see if both a left and a right
/// exist in their respective queues, and if so, takes the first of each and
/// calls a combining function on them to produce a value to yield.
///
/// This is intended to match a sequence of events with responders for hanging
/// get FIDL methods to guarantee that callers of the hanging get method see all
/// events.  However, it can be used for other things as well.
///
/// - L is the type of "left" values.
/// - R is the type of "right" values.
/// - O is the type of the output.
pub struct OneToOneMatcher<L, R, O> {
    lefts: VecDeque<L>,
    rights: VecDeque<R>,
    match_fn: Box<dyn Fn(L, R) -> O + 'static>,
    waker: Option<Waker>,
}

impl<L, R, O> OneToOneMatcher<L, R, O> {
    pub fn new<F>(match_fn: F) -> Self
    where
        F: Fn(L, R) -> O + 'static,
    {
        Self {
            lefts: VecDeque::new(),
            rights: VecDeque::new(),
            match_fn: Box::new(match_fn),
            waker: None,
        }
    }

    pub fn enqueue_left(&mut self, left: L) {
        self.lefts.push_back(left);

        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    pub fn enqueue_right(&mut self, right: R) {
        self.rights.push_back(right);

        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }

    /// Clear all enqueued "left" items.
    pub fn clear_left_queue(&mut self) {
        self.lefts.clear();
    }
}

impl<L, R, O> Unpin for OneToOneMatcher<L, R, O> {}

impl<L, R, O> Stream for OneToOneMatcher<L, R, O> {
    type Item = O;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if !self.lefts.is_empty() && !self.rights.is_empty() {
            let left = self.lefts.pop_front().expect("Lefts should not be empty.");
            let right = self.rights.pop_front().expect("Rights should not be empty.");
            let output = (self.match_fn)(left, right);
            Poll::Ready(Some(output))
        } else {
            self.waker = Some(context.waker().clone());
            Poll::Pending
        }
    }
}

impl<L, R, O> FusedStream for OneToOneMatcher<L, R, O> {
    fn is_terminated(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use async_utils::PollExt;
    use fuchsia_async as fasync;
    use futures::StreamExt;

    #[fuchsia::test]
    fn match_one_pair() {
        let mut exec = fasync::TestExecutor::new();

        let add = |x, y| x + y;
        let mut matcher = OneToOneMatcher::new(add);

        matcher.enqueue_left(1);
        matcher.enqueue_right(2);

        let output = exec.run_singlethreaded(matcher.next());

        assert_eq!(output, Some(3));
    }

    #[fuchsia::test]
    fn match_multiple_pairs() {
        let mut exec = fasync::TestExecutor::new();

        let add = |x, y| x + y;
        let mut matcher = OneToOneMatcher::new(add);

        matcher.enqueue_left(1);
        matcher.enqueue_right(2);

        matcher.enqueue_left(101);
        matcher.enqueue_right(102);

        let output = exec.run_singlethreaded(matcher.next());
        assert_eq!(output, Some(3));

        let output = exec.run_singlethreaded(matcher.next());
        assert_eq!(output, Some(203));
    }

    #[fuchsia::test]
    fn no_match() {
        let mut exec = fasync::TestExecutor::new();

        let add = |x: i64, y: i64| x + y;
        let mut matcher = OneToOneMatcher::new(add);

        matcher.enqueue_left(1);

        exec.run_until_stalled(&mut matcher.next()).expect_pending("Unexpected match");
    }

    #[fuchsia::test]
    fn match_and_then_no_match() {
        let mut exec = fasync::TestExecutor::new();

        let add = |x, y| x + y;
        let mut matcher = OneToOneMatcher::new(add);

        matcher.enqueue_left(1);
        matcher.enqueue_right(2);
        matcher.enqueue_left(101);

        let output = exec.run_singlethreaded(matcher.next());
        assert_eq!(output, Some(3));

        exec.run_until_stalled(&mut matcher.next()).expect_pending("Unexpected match");
    }

    #[fuchsia::test]
    fn no_match_and_then_match() {
        let mut exec = fasync::TestExecutor::new();

        let add = |x, y| x + y;
        let mut matcher = OneToOneMatcher::new(add);

        matcher.enqueue_left(1);
        exec.run_until_stalled(&mut matcher.next()).expect_pending("Unexpected match");

        matcher.enqueue_right(2);
        let output = exec.run_singlethreaded(matcher.next());
        assert_eq!(output, Some(3));
    }
}
