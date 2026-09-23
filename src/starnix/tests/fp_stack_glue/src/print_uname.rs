// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

fn main() {
    // This will transitively call fuchsia.buildinfo/Provider.GetBuildInfo which our test mocks.
    let _uname = nix::sys::utsname::uname().unwrap();
}
