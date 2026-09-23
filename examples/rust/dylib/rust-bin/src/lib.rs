// Copyright 2022 The Fuchsia Authors.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
mod test {
    #[fuchsia::test]
    fn call() {
        assert_eq!(rust_shared::rust_get_int(42), 42);
    }

    #[fuchsia::test(logging = false)]
    #[should_panic]
    fn call_and_panic() {
        assert_ne!(rust_shared::rust_get_int(42), 42);
    }
}
