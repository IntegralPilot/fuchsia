// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_async as fasync;
use futures::future::join;
use futures::{Future, FutureExt, StreamExt};
use mpmc::*;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

#[fasync::run_singlethreaded]
#[test]
async fn it_works() {
    let s = Sender::default();
    let mut r1 = s.new_receiver();
    let mut r2 = r1.clone();

    s.send(20).await;
    assert_eq!(r1.next().await, Some(20));
    assert_eq!(r2.next().await, Some(20));
}

#[fasync::run_singlethreaded]
#[test]
async fn dropping_sender_terminates_stream() {
    let s = Sender::default();
    let mut r1 = s.new_receiver();
    let mut r2 = r1.clone();

    s.send(20).await;
    drop(s);
    assert_eq!(r1.next().await, Some(20));
    assert_eq!(r2.next().await, Some(20));
    assert_eq!(r1.next().await, None);
    assert_eq!(r2.next().await, None);
}

#[fasync::run_singlethreaded]
#[test]
async fn receivers_cloned_after_termination_yield_none() {
    let s = Sender::default();
    let mut r1 = s.new_receiver();

    s.send(20).await;
    drop(s);
    let mut r2 = r1.clone();
    assert_eq!(r1.next().await, Some(20));
    assert_eq!(r1.next().await, None);
    assert_eq!(r2.next().await, None);
}

#[fasync::run_singlethreaded]
#[test]
async fn sender_side_initialization() {
    let s = Sender::default();
    let mut r1 = s.new_receiver();
    let mut r2 = s.new_receiver();

    s.send(20).await;
    assert_eq!(r1.next().await, Some(20));
    assert_eq!(r2.next().await, Some(20));
}

#[fasync::run_singlethreaded]
#[test]
async fn backpressure() {
    let s: Sender<usize> = Sender::with_buffer_size(1);
    let _r = s.new_receiver();

    s.send(1).await;
    s.send(1).await;

    let mut send_exceeding_buffer = s.send(1).boxed();
    let poll_result =
        Pin::new(&mut send_exceeding_buffer).poll(&mut Context::from_waker(Waker::noop()));
    assert_eq!(poll_result, Poll::Pending);
}

#[fasync::run_singlethreaded]
#[test]
async fn backpressure_across_senders() {
    let s1: Sender<usize> = Sender::with_buffer_size(1);
    let s2 = s1.clone();
    let mut r = s1.new_receiver();

    s1.send(1).await;
    s1.send(1).await;

    // Ensure a different sender is pressured.
    let mut send1_exceeding_buffer = s2.send(1).boxed();
    let poll1_result =
        Pin::new(&mut send1_exceeding_buffer).poll(&mut Context::from_waker(Waker::noop()));
    assert_eq!(poll1_result, Poll::Pending);

    // Ensure all senders are blocked.
    let mut send2_exceeding_buffer = s1.send(1).boxed();
    let poll2_result =
        Pin::new(&mut send2_exceeding_buffer).poll(&mut Context::from_waker(Waker::noop()));
    assert_eq!(poll2_result, Poll::Pending);

    // Unblock
    r.next().await;
    r.next().await;

    // Ensure all sends resolve.
    join(send1_exceeding_buffer, send2_exceeding_buffer).await;
}

// Tests that `send_or_disconnect` drops full receivers instead of applying backpressure.
//
// Standard `send()` applies backpressure when any receiver's buffer is full, blocking asynchronous
// send operations until space is available. In broadcast server loops, the entire service would be
// blocked if a single client watcher stalls or stops consuming `send()` messages.
//
// Instead, `send_or_disconnect()` uses non-blocking `try_send()` checks. If a receiver returns
// `Full`, we drop its corresponding sender from the active list, closing the channel immediately.
//
// This test constructs a sender with buffer size 0 and three receivers (`r1`, `r2`, and `r3`).
// It sends msg 10, consuming it on `r2` and `r3` but leaving `r1` full. Sending msg 20 then
// disconnects `r1` while buffering in `r2` and `r3`. Only `r3` consumes msg 20, so `r2` is now
// full. Sending msg 30 disconnects `r2` while keeping `r3` connected, verifying that receivers are
// evaluated independently on each send and can be sequentially disconnected as they lag behind.
#[fasync::run_singlethreaded]
#[test]
async fn send_or_disconnect_disconnects_full_receivers() {
    let s: Sender<usize> = Sender::with_buffer_size(0);
    let mut r1 = s.new_receiver();
    let mut r2 = s.new_receiver();
    let mut r3 = s.new_receiver();

    // -------------------------------------
    // Send msg 10 to all receivers.
    s.send_or_disconnect(10).await;
    // All buffers are now full (capacity = 1).

    // r1 does not consume msg 10 - it will remain full.

    // r2 and r3 consume msg 10 so their buffers have space.
    assert_eq!(r2.next().await, Some(10));
    assert_eq!(r3.next().await, Some(10));

    // -------------------------------------
    // Send msg 20.
    s.send_or_disconnect(20).await;
    // r1 is full and disconnects.
    // r2 and r3 receive msg 20 and become full again.

    // r1 was disconnected after it buffered msg 10.
    assert_eq!(r1.next().await, Some(10));
    assert_eq!(r1.next().await, None);

    // r2 does not consume msg 20 - it will remain full.

    // r3 consumes msg 20 so it has space again.
    assert_eq!(r3.next().await, Some(20));

    // -------------------------------------
    // Send msg 30.
    s.send_or_disconnect(30).await;
    // r2 is full and disconnects.
    // r3 receives msg 30.

    // r2 was disconnected after it buffered msg 20.
    assert_eq!(r2.next().await, Some(20));
    assert_eq!(r2.next().await, None);

    // r3 remains fully functional, receiving msg 30
    assert_eq!(r3.next().await, Some(30));

    // -------------------------------------
    // Send msg 40.
    s.send_or_disconnect(40).await;
    // After the sender disconnects two receivers, sender and r3 continue to function properly.

    // r3 receives msg 40.
    assert_eq!(r3.next().await, Some(40));
}
