// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon kernel
//! [syscalls](https://fuchsia.dev/fuchsia-src/reference/syscalls).

// Put this first so subsequently declared modules have access.
#[macro_use]
mod macros;

mod bti;
mod channel;
mod clock;
mod clock_update;
mod counter;
mod cprng;
mod debuglog;
mod event;
mod eventpair;
mod exception;
mod fifo;
mod futex;
mod guest;
mod handle;
mod info;
mod interrupt;
mod iob;
mod iommu;
mod job;
mod koid;
mod msi;
mod name;
mod object_type;
mod pager;
mod pci_device;
mod pmt;
mod port;
mod process;
mod profile;
mod property;
mod resource;
mod rights;
mod signals;
mod socket;
mod stream;
mod suspend_token;
mod system;
mod task;
mod thread;
mod time;
mod vcpu;
mod version;
mod vmar;
mod vmo;
mod wait;

pub mod vdso_next {
    pub use super::iob::vdso_next::*;
}

pub use self::bti::*;
pub use self::channel::*;
pub use self::clock::*;
pub use self::clock_update::{ClockUpdate, ClockUpdateBuilder};
pub use self::counter::*;
pub use self::cprng::*;
pub use self::debuglog::*;
pub use self::event::*;
pub use self::eventpair::*;
pub use self::exception::*;
pub use self::fifo::*;
pub use self::futex::*;
pub use self::guest::*;
pub use self::handle::*;
pub use self::info::*;
pub use self::interrupt::*;
pub use self::iob::*;
pub use self::iommu::*;
pub use self::job::*;
pub use self::koid::*;
pub use self::msi::*;
pub use self::name::*;
pub use self::object_type::*;
pub use self::pager::*;
pub use self::pci_device::*;
pub use self::pmt::*;
pub use self::port::*;
pub use self::process::*;
pub use self::profile::*;
pub use self::property::*;
pub use self::resource::*;
pub use self::rights::*;
pub use self::signals::*;
pub use self::socket::*;
pub use self::stream::*;
pub use self::suspend_token::*;
pub use self::system::*;
pub use self::task::*;
pub use self::thread::*;
pub use self::time::*;
pub use self::vcpu::*;
pub use self::version::*;
pub use self::vmar::*;
pub use self::vmo::*;
pub use self::wait::*;
pub use zx_status::*;
pub use zx_status_ext::*;

/// Prelude containing common utility traits.
/// Designed for use like `use zx::prelude::*;`
pub mod prelude {
    pub use crate::{AsHandleRef, Peered};
}

pub mod sys {
    pub use zx_sys::*;
}
