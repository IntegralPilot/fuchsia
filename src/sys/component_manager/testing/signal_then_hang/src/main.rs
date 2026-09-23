// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fuchsia_runtime::{HandleInfo, HandleType};

use zx::Peered;

/// Signals USER0 on the peer of the USER0 handle, then sleep forever.
///
/// This little utility program is useful for testing runners. We can monitor
/// signals to verify that the program was successfully run, and also test
/// stopping a program.
fn main() {
    let user0 =
        fuchsia_runtime::take_startup_handle(HandleInfo::new(HandleType::User0, 0)).unwrap();
    let user0: zx::Channel = user0.into();
    user0.signal_peer(zx::Signals::empty(), zx::Signals::USER_0).unwrap();
    loop {
        std::thread::park();
    }
}
