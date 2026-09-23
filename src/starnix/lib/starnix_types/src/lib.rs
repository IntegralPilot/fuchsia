// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod arch;
pub mod atomic;
pub mod augmented;
pub mod convert;
pub mod futex_address;
pub mod math;
pub mod ownership;
pub mod stats;
pub mod string;
pub mod thread_start_info;
pub mod time;
pub mod user_buffer;
pub mod vfs;

mod errors;

use std::sync::LazyLock;

pub static PAGE_SIZE: LazyLock<u64> = LazyLock::new(|| zx::system_get_page_size() as u64);
