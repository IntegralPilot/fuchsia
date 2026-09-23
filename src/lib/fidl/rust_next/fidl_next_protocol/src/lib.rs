// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Protocol support for FIDL.
//!
//! This crate provides a number of types and traits related to FIDL protocols.
//! These types and traits are all "untyped" - that is, they do not know about
//! the specific types of the FIDL messages being sent and received. They only
//! deal with protocol-layer semantics: requests and responses, clients and
//! servers, and transports.
//!
//! ## Transports
//!
//! This crate uses "transport" to refer to the specific object which moves
//! bytes and handles from a sender to a receiver. For example, this crate may
//! refer to a channel, socket, file, or other byte source/sink as a
//! "transport". This differs from the `@transport(..)` annotation in the FIDL
//! language, which can be added to protocols.
//!
//! FIDL transports implement the [`Transport`] trait. This trait defines
//! several key properties:
//!
//! - The associated [`Shared`](Transport::Shared) and
//!   [`Exclusive`](Transport::Exclusive) types.
//! - The buffer types for sending and receiving data.
//! - The `async` methods for sending and receiving data with those buffers.
//!
//! All types in the protocol layer are generic over the transport, making it
//! easy to add support for new types of transports.
//!
//! By default, both sending and receiving data with a transport are
//! asynchronous operations. However, transports may support synchronous send
//! operations by implementing the [`NonBlockingTransport`] trait. This trait
//! allows users to replace `.await`-ing a send operation with
//! [`.send_immediately()`](NonBlockingTransport::send_immediately), which
//! synchronously completes the send future.
//!
//! This crate provides an implementation of `Transport` for Fuchsia channels.
//!
//! ## Clients and servers
//!
//! [`ClientDispatcher`]s and [`ServerDispatcher`]s are constructed from a
//! transport, and can be `run` with a corresponding [`ClientHandler`] or
//! [`ServerHandler`]. The dispatcher will then run its event loop to receive
//! data through the transport. Client dispatchers use their handlers to handle
//! incoming events, and coordinate with any [`Client`]s to route two-way method
//! responses to waiting futures. Server dispatchers use their handlers to
//! handle incoming one-way and two-way method requests, but do not generally
//! coordinate with their associated [`Server`]s.
//!
//! [`Client`]s and [`Server`]s implement `Clone`, and should be cloned to
//! interact with the connection from multiple locations.
//!
//! ## Message ordering
//!
//! Dispatchers are guaranteed to handle requests and events serially and in the
//! order they are received. However, because the responses to two-way messages
//! are awaited at the call site and not handled by the dispatcher, there is no
//! guarantee about the relative orderings of responses and subsequent events.
//! This means that if a malformed two-way response is received, the resulting
//! connection closure may not occur before subsequent events are processed.
//!
//! Two-way responses are guaranteed to preserve completion order relative to
//! each other: if response A is received before response B, and the future
//! awaiting response B completes, then the future awaiting response A will
//! complete on the next poll. However, the executor may schedule tasks ready to
//! make progress in any order it chooses. This means that the orders in which
//! tasks awaiting two-way responses were scheduled may not match the orders in
//! which those responses were received.

#![deny(
    future_incompatible,
    missing_docs,
    nonstandard_style,
    unused,
    warnings,
    clippy::all,
    clippy::alloc_instead_of_core,
    clippy::missing_safety_doc,
    clippy::std_instead_of_core,
    // TODO: re-enable this lint after justifying unsafe blocks
    // clippy::undocumented_unsafe_blocks,
    rustdoc::broken_intra_doc_links,
    rustdoc::missing_crate_level_docs
)]
#![forbid(unsafe_op_in_unsafe_fn)]

mod endpoints;
mod error;
mod flexibility;
mod flexible;
mod framework_error;
#[cfg(feature = "fuchsia")]
pub mod fuchsia;
mod message;
pub mod mpsc;
mod service;
mod strict;
#[cfg(test)]
mod testing;
mod transport;
pub mod wire;

pub use self::endpoints::*;
pub use self::error::*;
pub use self::flexibility::*;
pub use self::flexible::*;
pub use self::framework_error::*;
pub use self::message::*;
pub use self::service::*;
pub use self::strict::*;
pub use self::transport::*;
