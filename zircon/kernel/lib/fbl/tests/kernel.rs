// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// Test suite for Rust fbl::Name implementation.
#[cfg(ktest)]
#[unittest::suite(name = "name_rust")]
mod tests {
    use fbl::Name;
    use pin_init::stack_pin_init;
    use unittest::{assert_eq, assert_false, assert_true};

    const FILL: u8 = 0x7f;
    const NAME_SIZE: usize = 32;

    fn buffer_invariants_hold(buf: &[u8]) -> bool {
        let mut idx = 0;
        while idx < buf.len() && buf[idx] != 0 {
            idx += 1;
        }
        if idx == buf.len() {
            return false;
        }
        idx += 1;
        while idx < buf.len() {
            if buf[idx] != 0 {
                return false;
            }
            idx += 1;
        }
        true
    }

    /// Test in-place pin initialization, emptiness, getting the name, and embedded null.
    #[test]
    fn test_basic() {
        stack_pin_init!(let empty = Name::<NAME_SIZE>::init());
        let empty = &*empty;
        assert_true!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let mut out = [FILL; NAME_SIZE];
        empty.get(&mut out);
        assert_eq!(out[0], 0);
        assert_true!(buffer_invariants_hold(&out));

        stack_pin_init!(let name = Name::<NAME_SIZE>::init());
        let name = &*name;
        name.set(b"hello");
        assert_false!(name.is_empty());
        assert_eq!(name.len(), 5);

        name.get(&mut out);
        assert_true!(&out[..5] == b"hello");
        assert_eq!(out[5], 0);
        assert_true!(buffer_invariants_hold(&out));

        // Test that an embedded null byte terminates the name.
        name.set(b"hello\0world");
        assert_eq!(name.len(), 5);
        name.get(&mut out);
        assert_true!(&out[..5] == b"hello");
        assert_eq!(out[5], 0);
        assert_true!(buffer_invariants_hold(&out));
    }

    /// Test input truncation beyond SIZE - 1 and output buffer truncation.
    #[test]
    fn test_truncation() {
        stack_pin_init!(let name = Name::<NAME_SIZE>::init());
        let name = &*name;

        // Input truncation to SIZE - 1 bytes.
        let long_input = [b'x'; NAME_SIZE + 10];
        name.set(&long_input);
        assert_eq!(name.len(), NAME_SIZE - 1);

        let mut out = [FILL; NAME_SIZE * 2];
        name.get(&mut out);
        assert_true!(out[..NAME_SIZE - 1] == [b'x'; NAME_SIZE - 1]);
        assert_eq!(out[NAME_SIZE - 1], 0);
        assert_true!(buffer_invariants_hold(&out));

        // Output buffer truncation to smaller buffer size (8 bytes -> 7 chars + null).
        let mut small_out = [FILL; 8];
        name.get(&mut small_out);
        assert_true!(small_out[..7] == [b'x'; 7]);
        assert_eq!(small_out[7], 0);
        assert_true!(buffer_invariants_hold(&small_out));

        // Zero-sized output buffer should not be modified or panic.
        let mut zero_out = [FILL; 2];
        name.get(&mut zero_out[..0]);
        assert_eq!(zero_out[0], FILL);
        assert_eq!(zero_out[1], FILL);
    }

    /// Test set, set_str, copy_name, and copy_from.
    #[test]
    fn test_set_and_copy() {
        stack_pin_init!(let name = Name::<NAME_SIZE>::init());
        let name = &*name;
        name.set_str("test_name");
        assert_eq!(name.len(), 9);

        let copied = name.copy_name();
        assert_true!(&copied[..9] == b"test_name");
        assert_eq!(copied[9], 0);

        stack_pin_init!(let dst = Name::<NAME_SIZE>::init());
        let dst = &*dst;
        dst.copy_from(name);
        assert_true!(dst.copy_name() == name.copy_name());

        // Self-copy is a no-op and should preserve content.
        dst.copy_from(dst);
        assert_true!(dst.copy_name() == name.copy_name());
    }
}
