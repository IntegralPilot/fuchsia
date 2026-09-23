// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[test]
fn a_passing_test() {
    println!("this is a passing test")
}

#[fuchsia::test]
async fn a_passing_test_with_err_logs() {
    log::error!("this is an error");
    println!("this is a passing test");
}

#[test]
fn a_failing_test() {
    panic!("this is a failing test")
}

#[fuchsia::test]
async fn a_failing_test_with_err_logs() {
    log::error!("this is an error");
    panic!("this is a failing test")
}

#[test]
fn a_skipped_test() {
    unreachable!("this is a skipped test")
}
