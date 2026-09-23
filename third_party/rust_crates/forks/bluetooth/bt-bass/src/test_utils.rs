// Copyright 2023 Google LLC
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::task::Poll;

use futures::channel::mpsc::UnboundedReceiver;
use futures::stream::FusedStream;
use futures::{Stream, StreamExt};

use crate::client::error::Error;
use crate::client::event::*;

pub struct FakeBassEventStream {
    receiver: UnboundedReceiver<Result<Event, Error>>,
    terminated: bool,
}

impl FakeBassEventStream {
    pub fn new(receiver: UnboundedReceiver<Result<Event, Error>>) -> Self {
        Self { receiver, terminated: false }
    }
}

impl FusedStream for FakeBassEventStream {
    fn is_terminated(&self) -> bool {
        self.terminated
    }
}

impl Stream for FakeBassEventStream {
    type Item = Result<Event, Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        if self.terminated {
            return Poll::Ready(None);
        }
        let polled = self.receiver.poll_next_unpin(cx);
        if let Poll::Ready(Some(Err(_))) = &polled {
            self.terminated = true;
        }
        polled
    }
}
