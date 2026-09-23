// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::stream::FusedStream;
use futures::task::Poll;
use futures::{ready, Stream, TryStream};
use pin_project::pin_project;
use std::pin::Pin;
use std::task::Context;

/// Short circuits a [`TryStream`] stream when the first error occurs.
///
/// The [`futures::TryStreamExt`] extension crate provides combinators that
/// pass through errors. However, sometimes we want the stream to short circurt
/// when an error occurs.
#[pin_project]
pub struct ShortCircuit<St> {
    #[pin]
    stream: Option<St>,
}

impl<St: TryStream> Stream for ShortCircuit<St> {
    type Item = Result<St::Ok, St::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        let Some(stream) = this.stream.as_mut().as_pin_mut() else { return Poll::Ready(None) };
        Poll::Ready(match ready!(stream.try_poll_next(cx)) {
            Some(Ok(val)) => Some(Ok(val)),
            Some(Err(err)) => {
                this.stream.set(None);
                Some(Err(err))
            }
            None => None,
        })
    }
}

impl<St: FusedStream + TryStream> FusedStream for ShortCircuit<St> {
    fn is_terminated(&self) -> bool {
        self.stream.as_ref().map(|st| st.is_terminated()).unwrap_or(true)
    }
}

impl<St> ShortCircuit<St> {
    /// Creates a new [`TryStream`] that will terminate after the first error
    /// is observed.
    pub fn new(stream: St) -> Self {
        Self { stream: Some(stream) }
    }
}

#[cfg(test)]
mod tests {
    use futures::{FutureExt, StreamExt as _};
    use test_case::test_case;

    use super::*;

    #[test_case(vec![Ok(1), Err(()), Ok(2)] => vec![Ok(1), Err(())])]
    #[test_case(vec![Ok(1), Ok(2), Ok(3)] => vec![Ok(1), Ok(2), Ok(3)])]
    fn short_circuit(input: Vec<Result<usize, ()>>) -> Vec<Result<usize, ()>> {
        let stream = futures::stream::iter(input).fuse();
        let mut short_circuited = ShortCircuit::new(stream);
        assert!(!short_circuited.is_terminated());
        let output = (&mut short_circuited)
            .collect::<Vec<_>>()
            .now_or_never()
            .expect("all items should be ready");
        assert!(short_circuited.is_terminated());
        output
    }
}
