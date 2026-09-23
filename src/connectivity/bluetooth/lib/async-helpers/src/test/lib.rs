// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_utils::PollExt;
use fuchsia_async as fasync;
use futures::future::Either;
use futures::stream::{Stream, StreamExt};
use futures::task::Poll;
use futures::Future;
use std::pin::pin;

///! Utilities for tests

/// Run a background task while waiting for a future that should occur.
/// This is useful for running a task which you expect to produce side effects that
/// mean the task is operating correctly. i.e. reacting to a peer action by producing a
/// response on a client's hanging get.
/// `background_fut` is expected not to finish ans is returned to the caller, along with
/// the result of `result_fut`.  If `background_fut` finishes, this function will panic.
pub fn run_while<BackgroundFut, ResultFut, Out>(
    exec: &mut fasync::TestExecutor,
    background_fut: BackgroundFut,
    result_fut: ResultFut,
) -> (Out, BackgroundFut)
where
    BackgroundFut: Future + Unpin,
    ResultFut: Future<Output = Out>,
{
    let result_fut = pin!(result_fut);
    let mut select_fut = futures::future::select(background_fut, result_fut);
    loop {
        match exec.run_until_stalled(&mut select_fut) {
            Poll::Ready(Either::Right(r)) => return r,
            Poll::Ready(Either::Left(_)) => panic!("Background future finished"),
            Poll::Pending => {}
        }
    }
}

/// Polls the provided `stream` and expects a non-null item to be produced.
#[track_caller]
pub fn expect_stream_item<S: Stream + Unpin>(
    exec: &mut fasync::TestExecutor,
    stream: &mut S,
) -> S::Item {
    exec.run_until_stalled(&mut stream.next()).expect("stream item").expect("not terminated")
}

/// Polls the provided `stream` and expects that it is Pending (e.g no stream items).
#[track_caller]
pub fn expect_stream_pending<S: Stream + Unpin>(exec: &mut fasync::TestExecutor, stream: &mut S) {
    let _ = exec.run_until_stalled(&mut stream.next()).expect_pending("stream pending");
}

/// Polls the provided `stream` and expects that it is terminated (e.g. returns None).
#[track_caller]
pub fn expect_stream_terminated<S: Stream + Unpin>(
    exec: &mut fasync::TestExecutor,
    stream: &mut S,
) {
    let result = exec.run_until_stalled(&mut stream.next()).expect("stream item");
    assert!(result.is_none());
}
