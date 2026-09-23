// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(not(feature = "fdomain"))]
extern crate fuchsia_fuzzctl as fuchsia_fuzzctl;
#[cfg(feature = "fdomain")]
extern crate fuchsia_fuzzctl_fdomain as fuchsia_fuzzctl;

mod controller;
mod diagnostics;
mod input;
mod manager;
mod options;
mod test;
mod util;
mod writer;

pub use self::controller::{FakeController, serve_controller};
pub use self::diagnostics::send_log_entry;
pub use self::input::verify_saved;
pub use self::manager::serve_manager;
pub use self::options::add_defaults;
pub use self::test::{TEST_URL, Test};
pub use self::util::create_task;
pub use self::writer::BufferSink;
