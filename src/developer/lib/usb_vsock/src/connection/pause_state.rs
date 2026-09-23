// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_sync::Mutex;
use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Poll, Waker};

/// Maintains whether a connection is paused. A paused connection should not
/// send any more data to the peer.
pub struct PauseState(Mutex<PauseStateInner>);

/// Mutex-protected interior of [`PauseState`]
struct PauseStateInner {
    paused: bool,
    wakers: Vec<Waker>,
}

impl PauseState {
    /// Create a new [`PauseState`]. The initial state is un-paused.
    pub fn new() -> Arc<Self> {
        Arc::new(PauseState(Mutex::new(PauseStateInner { paused: false, wakers: Vec::new() })))
    }

    /// Polls the given future, but pauses polling when we are in the paused
    /// state.
    pub async fn while_unpaused<T>(&self, f: impl Future<Output = T>) -> T {
        let mut f = pin!(f);
        futures::future::poll_fn(move |ctx| {
            {
                let mut this = self.0.lock();

                if this.wakers.iter().all(|x| !x.will_wake(ctx.waker())) {
                    this.wakers.push(ctx.waker().clone());
                }

                if this.paused {
                    return Poll::Pending;
                }
            }

            f.as_mut().poll(ctx)
        })
        .await
    }

    /// Set the paused state.
    pub fn set_paused(&self, paused: bool) {
        let mut this = self.0.lock();

        this.paused = paused;
        this.wakers.drain(..).for_each(Waker::wake);
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::{Stream, StreamExt};
    use std::task::Context;

    #[fuchsia::test]
    async fn test_pause() {
        let pause_state = PauseState::new();
        let pause_state_clone = Arc::clone(&pause_state);
        let stream = futures::stream::iter(1..)
            .then(|x| pause_state_clone.while_unpaused(futures::future::ready(x)));
        let mut stream = pin!(stream);
        let mut ctx = Context::from_waker(Waker::noop());

        assert_eq!(Poll::Ready(Some(1)), stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Ready(Some(2)), stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Ready(Some(3)), stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Ready(Some(4)), stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Ready(Some(5)), stream.as_mut().poll_next(&mut ctx));

        pause_state.set_paused(true);

        assert_eq!(Poll::Pending, stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Pending, stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Pending, stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Pending, stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Pending, stream.as_mut().poll_next(&mut ctx));

        pause_state.set_paused(true);

        assert_eq!(Poll::Pending, stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Pending, stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Pending, stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Pending, stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Pending, stream.as_mut().poll_next(&mut ctx));

        pause_state.set_paused(false);

        assert_eq!(Poll::Ready(Some(6)), stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Ready(Some(7)), stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Ready(Some(8)), stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Ready(Some(9)), stream.as_mut().poll_next(&mut ctx));
        assert_eq!(Poll::Ready(Some(10)), stream.as_mut().poll_next(&mut ctx));
    }
}
