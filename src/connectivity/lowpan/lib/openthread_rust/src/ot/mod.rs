// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! # OpenThread API Module #
//!
//! This module contains (mostly) type-safe versions of the OpenThread API,
//! excluding the platform API.
//!
//! ## Type Safety ##
//!
//! Full type safety for an API which wasn't created for type safety is hard. There are some
//! operations which feel like they should be fully safe but are ultimately marked as `unsafe`
//! because it might be possible to abuse in some way that causes undefined behavior.
//!
//! ## Types ##
//!
//! Each enum, struct, or object in the OpenThread C API is associated with a safe Rust
//! equivalent. For example:
//!
//! * [`otInstance`](crate::otsys::otInstance): [`ot::Instance`](Instance).
//! * [`otMessage`](crate::otsys::otMessage): [`ot::Message<'_>`](Message).
//! * [`otError`](crate::otsys::otError): [`ot::Error`](Error) or [`Result`].
//!
//! ## Ownership ##
//!
//! Some OpenThread API objects, like [`otsys::otInstance`](crate::otsys::otInstance) and
//! [`otsys::otMessage`](crate::otsys::otMessage) have hidden implementations and explicit ownership
//! transfer. That means we must have a notation which is capable of both "owned" instances
//! and "borrowed" references.
//!
//! The rust equivalent of passing around a pointer to one of thse
//! objects would be to pass around a reference: `otInstance*` becomes a `&ot::Instance`.
//! Owned instances are "boxed" into a [`ot::Box`](Box), so an owned `otInstance*` would become
//! a `ot::Box<ot::Instance>`, or a [`OtInstanceBox`](crate::OtInstanceBox) for short. When the box
//! goes out of scope, the appropriate OpenThread C finalization API is called.
//!
//! ## Singleton/Multiple OpenThread Instances ##
//!
//! OpenThread can be compiled to either only support a singleton instance or to support
//! multiple independent OpenThread instances.
//!
//! Currently, this crate only supports a singleton OpenThread instance. Attempting to create
//! more than one OpenThread instance at a time will result in a runtime panic.
//!
//! ## Traits ##
//!
//! The OpenThread API is broken down into "modules" of functionality containing types and
//! related methods. Similarly, this Rust interface breaks down functionality into traits.
//! This allows parts of the OpenThread API to be substituted with mocked versions for unit
//! testing.
//!
//! ## Callbacks ##
//!
//! In most cases, you register a callback by passing a closure to the appropriate callback
//! registration API.
//!
//! ## Platform Implementations ##
//!
//! This crate doesn't directly provide the platform implementations for OpenThread—that
//! needs to come from either a separate library or the program which is using this crate.
//!

use openthread_sys::*;
use std::ffi::CStr;

mod cli;
pub use cli::*;

mod error;
pub use error::*;

mod srp;
pub use srp::*;

mod singleton;
pub use singleton::*;

pub(crate) mod tasklets;
pub use tasklets::*;

mod reset;
pub use reset::*;

mod link;
pub use link::*;

mod link_metrics;
pub use link_metrics::*;

pub(crate) mod types;
pub use types::*;

mod dnssd;
pub use dnssd::*;

mod thread;
pub use thread::*;

mod ip6;
pub use ip6::*;

mod state;
pub use state::*;

mod radio;
pub use radio::*;

mod uptime;
pub use uptime::*;

mod udp;
pub use udp::*;

mod border_agent;
pub use border_agent::*;

mod infra_if;
pub use infra_if::*;

pub mod message;
pub use message::{Message, MessageBuffer};

mod otbox;
pub use otbox::*;

mod dataset;
pub use dataset::*;

mod backbone_router;
pub use backbone_router::*;

mod border_router;
pub use border_router::*;

mod platform;
pub use platform::*;

mod joiner;
pub use joiner::*;

mod trel;
pub use trel::*;

mod net_data;
pub use net_data::*;

mod nat64;
pub use nat64::*;

mod dns_upstream;
pub use dns_upstream::*;

mod multi_radio_neighbor_info;
pub use multi_radio_neighbor_info::*;

mod history_tracker;
pub use history_tracker::*;

/// Trait implemented by all OpenThread instances.
pub trait InstanceInterface:
    Ip6
    + Cli
    + Reset
    + Dataset
    + Link
    + Dnssd
    + State
    + Tasklets
    + Thread
    + BackboneRouter
    + BorderRouter
    + SrpServer
    + MessageBuffer
    + Radio
    + Joiner
    + Uptime
    + Udp
    + Trel
    + BorderAgent
    + NetData
    + Nat64
    + DnsUpstream
    + LinkMetrics
    + MultiRadioLink
    + HistoryTracker
{
}

/// Returns the OpenThread version string. This is the safe equivalent of
/// [`otsys::otGetVersionString()`](crate::otsys::otGetVersionString()).
pub fn get_version_string() -> &'static str {
    unsafe {
        // SAFETY: `otGetVersionString` guarantees to return a C-String that will not change.
        CStr::from_ptr(otGetVersionString())
            .to_str()
            .expect("OpenThread version string was bad UTF8")
    }
}

/// Changes the logging level.
pub fn set_logging_level(level: LogLevel) {
    unsafe {
        otLoggingSetLevel(level.into());
    }
}

/// Get the logging level.
pub fn get_logging_level() -> LogLevel {
    unsafe { otLoggingGetLevel().into() }
}

/// Represents the thread version
#[derive(
    Debug,
    Copy,
    Clone,
    Eq,
    Ord,
    PartialOrd,
    PartialEq,
    num_derive::FromPrimitive,
    num_derive::ToPrimitive,
)]
#[repr(u16)]
pub enum ThreadVersion {
    /// Thread specification version 1.1.0.
    ///
    /// This is the functional equivalent of `OT_THREAD_VERSION_1_1`, which is not currently
    /// exported as a part of the OpenThread C API.
    V1_1 = 2,

    /// Thread specification version 1.2.0.
    ///
    /// This is the functional equivalent of `OT_THREAD_VERSION_1_2`, which is not currently
    /// exported as a part of the OpenThread C API.
    V1_2 = 3,

    /// Thread specification version 1.3.0.
    ///
    /// This is the functional equivalent of `OT_THREAD_VERSION_1_3`, which is not currently
    /// exported as a part of the OpenThread C API.
    V1_3 = 4,
}

/// Returns the version of the Thread specification that OpenThread
/// is configured to use. This is the safe equivalent of
/// [`otsys::otThreadGetVersion()`](crate::otsys::otThreadGetVersion()).
pub fn get_thread_version() -> ThreadVersion {
    use num::FromPrimitive;

    // SAFETY: otThreadGetVersion() is guaranteed to be safe to call in any context.
    let ver = unsafe { otThreadGetVersion() };

    ThreadVersion::from_u16(ver).unwrap_or_else(|| panic!("Unknown Thread specification: {ver}"))
}

/// Returns a `'static`-scoped string slice describing the version of the
/// Thread specification that is currently in use.
///
/// Format is suitable for use with MeshCop.
pub fn get_thread_version_str() -> &'static str {
    match get_thread_version() {
        ThreadVersion::V1_1 => "1.1.0",
        ThreadVersion::V1_2 => "1.2.0",
        ThreadVersion::V1_3 => "1.3.0",
    }
}

/// Converts a byte string into an ASCII [`String`], properly escaped.
pub(crate) fn ascii_dump(data: &[u8]) -> String {
    let vec = data.iter().copied().flat_map(std::ascii::escape_default).collect::<Vec<_>>();
    std::str::from_utf8(&vec).unwrap().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_version() {
        let vstr = get_version_string();
        println!("OpenThread Version: {vstr:?}");
        assert!(!vstr.is_empty());
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn otPlatAlarmMilliGetNow() -> u32 {
        // SAFETY: Must only be called from unit test.
        0
    }
}
