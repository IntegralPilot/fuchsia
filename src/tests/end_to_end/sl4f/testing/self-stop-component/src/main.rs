// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::thread;
use std::time::Duration;

fn main() {
    // sleep 5 second to avoid this component exit too fast.
    thread::sleep(Duration::from_secs(5));
}
