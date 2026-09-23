// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Various types representing what can be sent from a repository.

use futures::prelude::*;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Helper for sending [hyper::body::Bytes] over a channel.
pub struct BodySender {
    tx: futures::channel::mpsc::Sender<Result<hyper::body::Bytes, std::io::Error>>,
}

impl BodySender {
    /// Try to send [hyper::body::Bytes] in the channel.
    #[allow(clippy::result_unit_err)]
    pub fn try_send_data(&mut self, data: hyper::body::Bytes) -> Result<(), ()> {
        self.tx.try_send(Ok(data)).map_err(|_| ())
    }
}

/// [Body] represents the type of content that be served from a repository.
#[allow(clippy::type_complexity)]
pub enum Body {
    Empty(http_body_util::Empty<hyper::body::Bytes>),
    Full(http_body_util::Full<hyper::body::Bytes>),
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
    #[cfg(not(target_os = "fuchsia"))]
    Sse(http_sse::Body),
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Body::Empty(_) => f.debug_tuple("Empty").finish(),
            Body::Full(_) => f.debug_tuple("Full").finish(),
            Body::Stream(_) => f.debug_tuple("Stream").finish(),
            #[cfg(not(target_os = "fuchsia"))]
            Body::Sse(_) => f.debug_tuple("Sse").finish(),
        }
    }
}

impl Body {
    /// An empty body.
    pub fn empty() -> Self {
        Body::Empty(http_body_util::Empty::new())
    }

    /// Stream the bytes as the body.
    pub fn wrap_stream<S, O, E>(stream: S) -> Self
    where
        S: Stream<Item = Result<O, E>> + Send + Sync + 'static,
        O: Into<hyper::body::Bytes>,
        E: Into<Box<dyn std::error::Error + Send + Sync + 'static>>,
    {
        let stream = stream.map(|res| match res {
            Ok(data) => Ok(hyper::body::Frame::data(data.into())),
            Err(e) => Err(std::io::Error::other(e)),
        });
        Body::Stream(http_body_util::StreamBody::new(Box::pin(stream)))
    }

    /// Stream teh body over a channel.
    pub fn channel() -> (BodySender, Self) {
        let (tx, rx) = futures::channel::mpsc::channel(1);
        (BodySender { tx }, Self::wrap_stream(rx))
    }
}

impl From<Vec<u8>> for Body {
    fn from(data: Vec<u8>) -> Self {
        Body::Full(http_body_util::Full::new(data.into()))
    }
}

impl From<String> for Body {
    fn from(data: String) -> Self {
        Body::Full(http_body_util::Full::new(data.into()))
    }
}

impl From<&'static str> for Body {
    fn from(data: &'static str) -> Self {
        Body::Full(http_body_util::Full::new(data.into()))
    }
}

#[cfg(not(target_os = "fuchsia"))]
impl From<http_sse::Body> for Body {
    fn from(body: http_sse::Body) -> Self {
        Body::Sse(body)
    }
}

impl hyper::body::Body for Body {
    type Data = hyper::body::Bytes;
    type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        match self.get_mut() {
            Body::Empty(b) => Pin::new(b).poll_frame(cx).map_err(|e| match e {}),
            Body::Full(b) => Pin::new(b).poll_frame(cx).map_err(|e| match e {}),
            Body::Stream(b) => Pin::new(b).poll_frame(cx).map_err(|e| Box::new(e) as _),
            #[cfg(not(target_os = "fuchsia"))]
            Body::Sse(b) => {
                Pin::new(b).poll_frame(cx).map_err(|e| Box::new(std::io::Error::other(e)) as _)
            }
        }
    }
}
