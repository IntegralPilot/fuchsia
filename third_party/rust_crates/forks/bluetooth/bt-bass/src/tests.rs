// Copyright 2023 Google LLC
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::task::Poll;

use assert_matches::assert_matches;
use futures::channel::mpsc::unbounded;
use futures::stream::StreamExt;

use crate::client::error::Error;
use crate::client::event::*;
use crate::test_utils::*;
use bt_bap::types::BroadcastId;

#[test]
fn fake_bass_event_stream() {
    let (sender, receiver) = unbounded();
    let mut stream = FakeBassEventStream::new(receiver);

    let mut noop_cx = futures::task::Context::from_waker(futures::task::noop_waker_ref());
    assert!(stream.poll_next_unpin(&mut noop_cx).is_pending());

    // Add events.
    sender
        .unbounded_send(Ok(Event::SyncedToPa(BroadcastId::try_from(1).unwrap())))
        .expect("should succeed");
    sender
        .unbounded_send(Ok(Event::BroadcastCodeRequired(BroadcastId::try_from(1).unwrap())))
        .expect("should succeed");

    let polled = stream.poll_next_unpin(&mut noop_cx);
    let Poll::Ready(Some(Ok(event))) = polled else {
        panic!("Expected to receive event");
    };
    assert_matches!(event, Event::SyncedToPa(_));

    let polled = stream.poll_next_unpin(&mut noop_cx);
    let Poll::Ready(Some(Ok(event))) = polled else {
        panic!("Expected to receive event");
    };
    assert_matches!(event, Event::BroadcastCodeRequired(_));

    assert!(stream.poll_next_unpin(&mut noop_cx).is_pending());

    sender
        .unbounded_send(Err(Error::EventStream(Box::new(Error::Generic(format!("some error"))))))
        .expect("should succeed");

    let polled = stream.poll_next_unpin(&mut noop_cx);
    let Poll::Ready(Some(Err(_))) = polled else {
        panic!("Expected to receive error");
    };

    let polled = stream.poll_next_unpin(&mut noop_cx);
    let Poll::Ready(None) = polled else {
        panic!("Expected to have terminated");
    };
}
