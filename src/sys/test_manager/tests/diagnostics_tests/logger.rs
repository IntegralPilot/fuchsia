// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use log::info;

#[fuchsia::test]
fn log() {
    info!("I'm a info log from a test");
}
