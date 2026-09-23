// Copyright 2019 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

use {futures::future::BoxFuture, futures::prelude::*};

pub use http::{Request, Response};
pub type Body = http_body_util::Full<bytes::Bytes>;

pub fn empty_body() -> Body {
    Body::default()
}

pub fn body_from<T: Into<bytes::Bytes>>(data: T) -> Body {
    Body::new(data.into())
}

pub trait IntoBody {
    type Body: http_body::Body;
    fn into_body(self) -> Self::Body;
}

impl<B: http_body::Body> IntoBody for http::Request<B> {
    type Body = B;
    fn into_body(self) -> B {
        self.into_body()
    }
}

impl<B: http_body::Body> IntoBody for http::Response<B> {
    type Body = B;
    fn into_body(self) -> B {
        self.into_body()
    }
}

impl<D: bytes::Buf> IntoBody for http_body_util::Full<D> {
    type Body = Self;
    fn into_body(self) -> Self {
        self
    }
}

impl<D: bytes::Buf> IntoBody for http_body_util::Empty<D> {
    type Body = Self;
    fn into_body(self) -> Self {
        self
    }
}

impl IntoBody for hyper::body::Incoming {
    type Body = Self;
    fn into_body(self) -> Self {
        self
    }
}

pub async fn to_bytes<I>(item: I) -> Result<bytes::Bytes, <I::Body as http_body::Body>::Error>
where
    I: IntoBody,
{
    use http_body_util::BodyExt as _;
    item.into_body().collect().await.map(|buf| buf.to_bytes())
}

pub mod mock;

/// A trait for providing HTTP capabilities to the StateMachine.
///
/// This trait is a wrapper around Hyper, to provide a simple request->response style of API for
/// the state machine to use.
///
/// In particular, it's meant to be easy to mock for tests.
pub trait HttpRequest {
    /// Make a request, and return an Response, as the header Parts and collect the entire collected
    /// Body as a Vec of bytes.
    fn request(&mut self, req: Request<Body>) -> BoxFuture<'_, Result<Response<Vec<u8>>, Error>>;
}

#[derive(Debug, thiserror::Error)]
// Parentheses are needed for .source, but will trigger unused_parens, so a tuple is used.
#[error("Http request failed: {}", match (.source, ()).0 {
    Some(source) => format!("{source}"),
    None => format!("kind: {:?}", .kind),
})]
pub struct Error {
    kind: ErrorKind,
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

#[derive(Debug, Eq, PartialEq)]
enum ErrorKind {
    User,
    Transport,
    Timeout,
}

impl Error {
    /// Create a timeout error
    ///
    /// This is valid for use in tests as well as production implementations of the trait, if
    /// application-layer timeouts are being implemented.
    pub fn new_timeout() -> Self {
        Self { kind: ErrorKind::Timeout, source: None }
    }

    /// Returns true if this error the result of the Hyper API being incorrectly used (a "user"
    /// error in Hyper)
    pub fn is_user(&self) -> bool {
        self.kind == ErrorKind::User
    }

    /// Returns true if this error is the result of a timeout when trying to fulfill the request.
    ///
    /// Note: Connect timeouts may be returned as transport or I/O errors, not timeouts, depending
    /// on where in the network / HTTP client stack the timeout occurs.
    pub fn is_timeout(&self) -> bool {
        self.kind == ErrorKind::Timeout
    }

    /// Create a transport error wrapping an underlying source error.
    pub fn new_transport(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self { kind: ErrorKind::Transport, source: Some(error.into()) }
    }
}

impl From<hyper::Error> for Error {
    fn from(error: hyper::Error) -> Self {
        let kind = if error.is_user() { ErrorKind::User } else { ErrorKind::Transport };
        Error { kind, source: Some(Box::new(error)) }
    }
}

pub mod mock_errors {
    use super::*;

    pub fn make_user_error() -> Error {
        Error { kind: ErrorKind::User, source: None }
    }

    pub fn make_transport_error() -> Error {
        Error { kind: ErrorKind::Transport, source: None }
    }
}

/// A stub HttpRequest that does nothing and returns an empty response immediately.
pub struct StubHttpRequest;

impl HttpRequest for StubHttpRequest {
    fn request(&mut self, _req: Request<Body>) -> BoxFuture<'_, Result<Response<Vec<u8>>, Error>> {
        future::ok(Response::default()).boxed()
    }
}
