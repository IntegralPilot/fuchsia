// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const CONTAINER_SENTINEL_BIT: usize = 1;

/// Create a sentinel pointer from a raw pointer.
pub fn make_sentinel<T, U>(ptr: *mut U) -> *mut T {
    const {
        assert!(
            core::mem::align_of::<T>() > 1,
            "Type T must have alignment > 1 to be used with sentinel pointers"
        )
    };
    ptr.map_addr(|addr| addr | CONTAINER_SENTINEL_BIT).cast()
}

/// Create a sentinel pointer from null.
pub const fn make_sentinel_null<T>() -> *mut T {
    // In const fn, we can assert directly since it is evaluated in const context
    assert!(
        core::mem::align_of::<T>() > 1,
        "Type T must have alignment > 1 to be used with sentinel pointers"
    );
    core::ptr::without_provenance_mut(CONTAINER_SENTINEL_BIT)
}

/// Turn a sentinel pointer back into a normal pointer.
pub fn unmake_sentinel<T, U>(sentinel: *mut U) -> *mut T {
    const {
        assert!(
            core::mem::align_of::<T>() > 1,
            "Type T must have alignment > 1 to be used with sentinel pointers"
        )
    };
    sentinel.map_addr(|addr| addr & !CONTAINER_SENTINEL_BIT).cast()
}

/// Test to see if a pointer is a sentinel pointer.
pub fn is_sentinel_ptr<T>(ptr: *const T) -> bool {
    const {
        assert!(
            core::mem::align_of::<T>() > 1,
            "Type T must have alignment > 1 to be used with sentinel pointers"
        )
    };
    (ptr.addr() & CONTAINER_SENTINEL_BIT) != 0
}

/// Test to see if a pointer (which may be a sentinel) is valid.
/// Valid means it is not null and not a sentinel.
pub fn valid_sentinel_ptr<T>(ptr: *const T) -> bool {
    const {
        assert!(
            core::mem::align_of::<T>() > 1,
            "Type T must have alignment > 1 to be used with sentinel pointers"
        )
    };
    !ptr.is_null() && !is_sentinel_ptr(ptr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sentinel_machinery() {
        let mut value = 42;
        let ptr = &mut value as *mut i32;

        assert!(!is_sentinel_ptr(ptr));
        assert!(valid_sentinel_ptr(ptr));

        let sent = make_sentinel::<i32, i32>(ptr);
        assert!(is_sentinel_ptr(sent));
        assert!(!valid_sentinel_ptr(sent));

        let unmasked = unmake_sentinel::<i32, i32>(sent);
        assert_eq!(unmasked, ptr);
        assert!(!is_sentinel_ptr(unmasked));
        assert!(valid_sentinel_ptr(unmasked));

        let null_sent = make_sentinel_null::<i32>();
        assert!(is_sentinel_ptr(null_sent));
        assert!(!valid_sentinel_ptr(null_sent));
        assert_eq!(null_sent as usize, 1);

        let unmasked_null = unmake_sentinel::<i32, i32>(null_sent);
        assert!(unmasked_null.is_null());
    }
}
