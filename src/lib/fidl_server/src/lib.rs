// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A library for implementing FIDL servers.
//!
//! This crate allows you to implement a FIDL server by writing a single
//! function that handles an incoming request.
//!
//! # Example
//!
//! The easiest approach is to call `serve_detached` with a request stream and a
//! handler function. This will spawn a background task for each client and
//! automatically log errors:
//!
//! ```
//! fs.dir("svc").add_fidl_service(|stream: LogRequestStream| {
//!     serve_detached(stream, |request| {
//!         let LogRequest::Log { message } = request;
//!         info!(message);
//!         Ok(())
//!     });
//! });
//! ```
//!
//! You can also run a server in the current task with `serve`. In this case,
//! you need to deal with the returned error yourself:
//!
//! ```
//! if let Err(err) = serve(stream, handler).await {
//!     error!("{:?}", err);
//! }
//! ```
//!
//! Instead of passing a function, you can also pass any object that implements
//! the `RequestHandler` trait. For example:
//!
//! ```
//! struct LogHandler;
//!
//! impl RequestHandler<LogMarker> for LogHandler {
//!     fn handle_request(&self, request: LogRequest) -> Result<(), Error> {
//!         let LogRequest::Log { message } = request;
//!         info!(message);
//!         Ok(())
//!     }
//! }
//!
//! // Somewhere else...
//! fs.dir("svc").add_fidl_service(|stream: LogRequestStream| {
//!     serve_detached(stream, LogHandler);
//! });
//!
//! For asynchronous request handling you can alternatively use the following:
//!
//! * `AsyncRequestHandler`
//! * `serve_async`
//! * `serve_async_detached`.
//! ```

#![deny(missing_docs)]

mod server;
pub use server::*;
