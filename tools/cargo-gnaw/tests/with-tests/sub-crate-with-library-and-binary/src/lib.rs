// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub fn lib_fn() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lib_and_bin_lib() {
        lib_fn();
        test_helpers::helper();
    }
}
