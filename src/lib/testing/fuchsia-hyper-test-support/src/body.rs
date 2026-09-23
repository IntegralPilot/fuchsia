// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use futures::prelude::*;
use std::pin::Pin;
use std::task::{Context, Poll};

/// An HTTP body enum supporting empty, full bytes, and streamed frames for test support.
pub enum Body {
    /// An empty body.
    Empty(http_body_util::Empty<hyper::body::Bytes>),
    /// A body consisting of a single in-memory buffer of bytes.
    Full(http_body_util::Full<hyper::body::Bytes>),
    /// A streamed body produced by an asynchronous stream of frames.
    Stream(
        http_body_util::StreamBody<
            Pin<
                Box<
                    dyn Stream<
                            Item = Result<hyper::body::Frame<hyper::body::Bytes>, std::io::Error>,
                        > + Send
                        + Sync,
                >,
            >,
        >,
    ),
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Body::Empty(_) => f.debug_tuple("Empty").finish(),
            Body::Full(_) => f.debug_tuple("Full").finish(),
            Body::Stream(_) => f.debug_tuple("Stream").finish(),
        }
    }
}

impl Body {
    /// Create a new empty body.
    pub fn empty() -> Self {
        Body::Empty(http_body_util::Empty::new())
    }
    /// Create a streamed body from a stream of items convertible to bytes.
    pub fn wrap_stream<S, O, E>(stream: S) -> Self
    where
        S: Stream<Item = Result<O, E>> + Send + Sync + 'static,
        O: Into<hyper::body::Bytes>,
        E: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    {
        let stream = stream.map(|res| match res {
            Ok(data) => Ok(hyper::body::Frame::data(data.into())),
            Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e)),
        });
        Body::Stream(http_body_util::StreamBody::new(Box::pin(stream)))
    }
}

impl From<Vec<u8>> for Body {
    fn from(data: Vec<u8>) -> Self {
        Body::Full(http_body_util::Full::new(data.into()))
    }
}

impl hyper::body::Body for Body {
    type Data = hyper::body::Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        match self.get_mut() {
            Body::Empty(b) => Pin::new(b).poll_frame(cx).map_err(|e| match e {}),
            Body::Full(b) => Pin::new(b).poll_frame(cx).map_err(|e| match e {}),
            Body::Stream(b) => Pin::new(b).poll_frame(cx),
        }
    }
}
