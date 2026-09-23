// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::test(logging_tags = ["logging_test"])]
async fn log_and_exit() {
    log::debug!("my debug message");
    log::info!("my info message");
    log::warn!("my warn message");
}
