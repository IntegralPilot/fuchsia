// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::main]
fn main() {
    log::info!("{}", echo_config_lib::Config::take_from_startup_handle().greeting);
}
