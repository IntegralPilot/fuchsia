// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use inspect_validator::run_all_trials;

#[fuchsia::test]
async fn inspect_validation_tests() {
    let results = run_all_trials().await;
    results.print_pretty_text();
    assert!(!results.failed());
}
