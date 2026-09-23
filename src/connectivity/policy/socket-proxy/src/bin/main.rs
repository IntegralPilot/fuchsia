// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[fuchsia::main(logging_tags = ["network_socket_proxy"])]
pub async fn main() {
    socket_proxy::run().await.expect("socket_proxy exited with an error")
}
