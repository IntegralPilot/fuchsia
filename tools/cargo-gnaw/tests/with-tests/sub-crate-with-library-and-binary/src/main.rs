// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

fn main() {}

#[cfg(test)]
mod tests {
    #[test]
    fn test_lib_and_bin_binary() {
        test_helpers::helper();
    }
}
