// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::test(
    logging_tags = ["error_logging_test"],
    logging_minimum_severity = "debug"
)]
async fn log_and_exit() {
    log::info!("my info message");
    log::warn!("my warn message");
    log::error!("my error message");
}
