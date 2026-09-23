// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

const HELLO_WORLD: &str = "Hello, world!";

fn main() {
    // SAFETY: basic FFI call
    unsafe {
        heapdump_bind_with_fdio();
    }

    // Generate a few kilobytes of duplicated allocations.
    let mut strings = vec![];
    for _ in 0..1000 {
        strings.push(HELLO_WORLD.to_string());
    }

    std::fs::write("/tmp/waiting", "waiting!").unwrap();
    std::thread::sleep(std::time::Duration::MAX);
}

unsafe extern "C" {
    fn heapdump_bind_with_fdio();
}
