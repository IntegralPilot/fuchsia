// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A futures-rs executor designed specifically for Fuchsia.
//!
//! # Example:
//! A simple, singlethreaded print server:
//!
//! ```no_run
//! #[fuchsia_async::run_singlethreaded]
//! async fn main() {
//!     let op = say_world();
//!
//!     // This println! will happen first
//!     println!("Hello...");
//!
//!     // Calling `.await` on `op` starts executing `say_world`.
//!     op.await;
//! }
//!
//!
//! async fn say_world() {
//!     println!("...world");
//! }
//! ```

#![warn(missing_docs)]
#![warn(clippy::all)]
#![deny(clippy::await_holding_lock)]
#![deny(clippy::await_holding_refcell_ref)]

mod runtime;
pub use self::runtime::*;

mod handle;
pub use self::handle::channel::{Channel, RecvMsg};
pub use self::handle::on_signals::OnSignalsRef;
pub use self::handle::socket::Socket;

/// Asynchronous networking abstractions.
pub mod net;

#[cfg(target_os = "fuchsia")]
pub use self::handle::{
    fifo::{Fifo, FifoEntry},
    on_interrupt::OnInterrupt,
    on_signals::OnSignals,
    rwhandle::{
        DefaultRWHandleSpec, RWHandle, RWHandleSpec, ReadableHandle, ReadableState, WritableHandle,
        WritableState,
    },
};

#[cfg(target_os = "fuchsia")]
pub use runtime::instrument;

/// An emulation library for Zircon handles on non-Fuchsia platforms.
#[cfg(not(target_os = "fuchsia"))]
pub mod emulated_handle {
    pub use super::handle::{
        AsHandleRef, Channel, EmulatedHandleRef, Event, EventPair, Handle, HandleDisposition,
        HandleInfo, HandleOp, HandleRef, Koid, MessageBuf, MessageBufEtc, ObjectType, Peered,
        Rights, Signals, Socket, SocketOpts, shut_down_handles,
    };

    /// Type of raw Zircon handles.
    #[allow(non_camel_case_types)]
    pub type zx_handle_t = u32;
}

pub use fuchsia_async_macro::{run, run_singlethreaded, run_until_stalled};

pub mod condition;
/// Testing support for repeated runs
pub mod test_support;
