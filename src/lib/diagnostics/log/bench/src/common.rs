// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::LazyLock;

pub static PLACEHOLDER_TEXT: LazyLock<String> = LazyLock::new(|| "x".repeat(32000));
pub static PROCESS_ID: LazyLock<zx::Koid> =
    LazyLock::new(|| fuchsia_runtime::process_self().koid().unwrap());
pub static THREAD_ID: LazyLock<zx::Koid> =
    LazyLock::new(|| fuchsia_runtime::with_thread_self(|thread| thread.koid().unwrap()));
