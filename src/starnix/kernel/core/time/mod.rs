// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

///! The time module is responsible for managing the UTC clock of the kernel.
pub mod utc;

mod hr_timer_manager;
mod interval_timer;
mod timeline;
mod timers;

pub use hr_timer_manager::*;
pub use interval_timer::*;
pub use timeline::*;
pub use timers::*;
