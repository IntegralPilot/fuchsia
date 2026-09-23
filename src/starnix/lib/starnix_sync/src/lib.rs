// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Synchronization objects used by Starnix

mod atomic_time;
mod condvar;
mod interruptible_event;
mod lock_dep_mutex;
mod lock_ordering;
mod lock_traits;
mod locks;
mod port_event;
mod rw_seq_lock;
mod thread_affinity;

pub use atomic_time::*;
pub use condvar::*;
pub use interruptible_event::*;
pub use lock_dep_mutex::*;
pub use lock_ordering::*;
pub use lock_ordering_macro::*;
pub use lock_traits::*;
pub use locks::*;
pub use port_event::*;
pub use rw_seq_lock::*;
pub use thread_affinity::*;

// This allows lock_ordering! macro to use paths within this crate
// by referring to them by the external crate name.
extern crate self as starnix_sync;
