// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This crate contains a type-safe interface to the OpenThread API.
//!
//! This crate assumes that the OpenThread platform interface have been
//! provided externally, perhaps by a separate crate.

#![warn(missing_docs)]
#![warn(missing_debug_implementations)]
#![warn(rust_2018_idioms)]

pub mod ot;
pub use openthread_sys as otsys;

mod bindings;

#[cfg(target_os = "fuchsia")]
mod otfuchsia;

/// Shorthand for `ot::Box<T>`
pub type OtBox<T> = ot::Box<T>;

/// Shorthand for `ot::Box<ot::Instance>`.
pub type OtInstanceBox = ot::Box<ot::Instance>;

/// Shorthand for `ot::Box<ot::Message<'a>>`.
pub type OtMessageBox<'a> = ot::Box<ot::Message<'a>>;

/// Prelude namespace for improving the ergonomics of using this crate.
#[macro_use]
pub mod prelude {
    #![allow(unused_imports)]

    pub use crate::bindings::*;
    pub use crate::{ot, otsys, OtBox, OtInstanceBox, OtMessageBox};
    pub use ot::{
        BackboneRouter as _, BorderRouter as _, Boxable as _, Dnssd as _, DnssdExt as _,
        IntoOtError as _, Ip6 as _, Link as _, MessageBuffer as _, OtCastable as _, Reset as _,
        SrpServer as _, State as _, Tasklets as _, Thread as _, Udp as _, Uptime as _,
    };

    pub use ot::TaskletsStreamExt as _;
    pub use std::convert::{TryFrom as _, TryInto as _};
}

// Internal prelude namespace for internal crate use only.
#[doc(hidden)]
#[macro_use]
pub(crate) mod prelude_internal {
    #![allow(unused_imports)]

    pub use crate::impl_ot_castable;
    pub use crate::otsys::*;
    pub use crate::prelude::*;
    pub use core::convert::{TryFrom, TryInto};
    pub use futures::prelude::*;
    pub use log::{debug, error, info, trace, warn};
    pub use num::FromPrimitive as _;
    pub(crate) use ot::ascii_dump;
    pub use ot::types::*;
    pub use ot::{
        BackboneRouter, BorderRouter, Boxable, Error, Instance, InstanceInterface, Ip6, Link,
        Message, MessageBuffer, Platform, Result, SrpServer, Tasklets, Thread,
    };
    pub use static_assertions as sa;
    pub use std::ffi::{CStr, CString};
    pub use std::marker::PhantomData;
    pub use std::os::raw::c_char;
    pub use std::ptr::null;
}
