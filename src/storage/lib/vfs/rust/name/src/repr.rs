// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use static_assertions::const_assert;
use std::num::NonZeroU8;
use std::ptr::NonNull;

const MAX_INLINE_LEN: usize = 15;

// Since names are at most 255 bytes, `Repr` can store the length as a `u8`.
const_assert!(crate::MAX_NAME_LENGTH <= 255);

/// The underlying representation for a node name, implementing Small String Optimization (SSO).
///
/// `Repr` is designed to keep the outer [Name] struct at exactly 16 bytes with 8-byte alignment,
/// while supporting three storage strategies:
///
/// 1. **Inline (`Inline1` through `Inline15`)**: For strings of length 1 to 15 bytes, the
///    characters are stored directly inside the struct. The enum discriminant doubles as the length
///    of the string, avoiding any heap allocation.
///
/// 2. **Static Borrow (`StaticBorrow`)**: For static string literals (`&'static str`) longer than
///    15 bytes, this variant holds a direct pointer and length borrowing the static memory. This
///    avoids heap allocations for constants defined in the codebase.
///
/// 3. **Allocated (`Allocated`)**: For dynamically created strings longer than 15 bytes, this
///    variant holds a pointer to a heap-allocated, boxed byte slice (`Box<[u8]>`).
///
/// ## Memory Layout & Niche Optimization
///
/// * The size of `Repr` is exactly 16 bytes, and its alignment is 8 bytes.
/// * The types in the variants have been carefully chosen to enable niche optimizations making both
///   `Option<Name>`, and `Result<Name, ParseNameError>` also exactly 16 bytes.
pub enum Repr {
    Inlined1([u8; 1]),
    Inlined2([u8; 2]),
    Inlined3([u8; 3]),
    Inlined4([u8; 4]),
    Inlined5([u8; 5]),
    Inlined6([u8; 6]),
    Inlined7([u8; 7]),
    Inlined8([u8; 8]),
    Inlined9([u8; 9]),
    Inlined10([u8; 10]),
    Inlined11([u8; 11]),
    Inlined12([u8; 12]),
    Inlined13([u8; 13]),
    Inlined14([u8; 14]),
    Inlined15([u8; 15]),
    StaticBorrowed { ptr: NonNull<u8>, len: NonZeroU8 },
    HeapAllocated { ptr: NonNull<u8>, len: NonZeroU8 },
}

// Safety: Repr has value semantics and no shared mutability.
unsafe impl Send for Repr {}
unsafe impl Sync for Repr {}

impl Repr {
    /// Constructs a `Repr` from a static string slice.
    ///
    /// If the string is <= 15 bytes, it is stored inline. Otherwise, it borrows the static memory
    /// directly without allocation.
    ///
    /// # Panics
    ///
    /// Panics if the length of `name` is 0 or greater than 255. These requirements should be
    /// checked by [`crate::validate_name`].
    pub fn from_static_str(name: &'static str) -> Self {
        if name.len() <= MAX_INLINE_LEN {
            Self::new_inlined(name)
        } else {
            Self::new_static_borrowed(name)
        }
    }

    /// Constructs a `Repr` by copying a string slice.
    ///
    /// If the string is <= 15 bytes, it is stored inline. Otherwise, it copies the bytes into a
    /// heap-allocated buffer.
    ///
    /// # Panics
    ///
    /// Panics if the length of `name` is 0 or greater than 255. These requirements should be
    /// checked by [`crate::validate_name`].
    pub fn from_str(name: &str) -> Self {
        if name.len() <= MAX_INLINE_LEN {
            Self::new_inlined(name)
        } else {
            Self::new_heap_allocated(Box::from(name.as_bytes()))
        }
    }

    /// Constructs a `Repr` from an owned `String`.
    ///
    /// If the string is <= 15 bytes, it is stored inline (releasing the original allocation).
    /// Otherwise, it reuses the owned string's allocation directly without copying.
    ///
    /// # Panics
    ///
    /// Panics if the length of `name` is 0 or greater than 255. These requirements should be
    /// checked by [`crate::validate_name`].
    pub fn from_string(name: String) -> Self {
        if name.len() <= MAX_INLINE_LEN {
            Self::new_inlined(&*name)
        } else {
            Self::new_heap_allocated(name.into_bytes().into_boxed_slice())
        }
    }

    /// Constructs a `StaticBorrowed` variant wrapping a reference.
    fn new_static_borrowed(name: &'static str) -> Self {
        let len = NonZeroU8::try_from(u8::try_from(name.len()).unwrap()).unwrap();
        Self::StaticBorrowed { ptr: NonNull::from_ref(name.as_bytes()).cast(), len }
    }

    /// Constructs a `HeapAllocated` variant from a boxed slice.
    fn new_heap_allocated(name: Box<[u8]>) -> Self {
        let len = NonZeroU8::try_from(u8::try_from(name.len()).unwrap()).unwrap();
        // SAFETY: `Box::into_raw` is guaranteed to return a non-null pointer.
        let ptr = unsafe { NonNull::new_unchecked(Box::into_raw(name) as *mut u8) };
        Self::HeapAllocated { ptr, len }
    }

    /// Constructs the appropriate `InlinedN` variant based on string length.
    ///
    /// # Panics
    ///
    /// Panics if the string length is not in the range `1..=15`.
    fn new_inlined(name: &str) -> Self {
        let bytes = name.as_bytes();
        match name.len() {
            1 => Self::Inlined1(bytes.try_into().unwrap()),
            2 => Self::Inlined2(bytes.try_into().unwrap()),
            3 => Self::Inlined3(bytes.try_into().unwrap()),
            4 => Self::Inlined4(bytes.try_into().unwrap()),
            5 => Self::Inlined5(bytes.try_into().unwrap()),
            6 => Self::Inlined6(bytes.try_into().unwrap()),
            7 => Self::Inlined7(bytes.try_into().unwrap()),
            8 => Self::Inlined8(bytes.try_into().unwrap()),
            9 => Self::Inlined9(bytes.try_into().unwrap()),
            10 => Self::Inlined10(bytes.try_into().unwrap()),
            11 => Self::Inlined11(bytes.try_into().unwrap()),
            12 => Self::Inlined12(bytes.try_into().unwrap()),
            13 => Self::Inlined13(bytes.try_into().unwrap()),
            14 => Self::Inlined14(bytes.try_into().unwrap()),
            15 => Self::Inlined15(bytes.try_into().unwrap()),
            _ => {
                panic!("Invalid inline size");
            }
        }
    }

    /// Helper to safely retrieve a byte slice from any of the `InlinedN` variants.
    ///
    /// # Panics
    ///
    /// Panics if the variant is not one of the `InlinedN` variants.
    fn inline_bytes(&self) -> &[u8] {
        match self {
            Self::Inlined1(x) => x,
            Self::Inlined2(x) => x,
            Self::Inlined3(x) => x,
            Self::Inlined4(x) => x,
            Self::Inlined5(x) => x,
            Self::Inlined6(x) => x,
            Self::Inlined7(x) => x,
            Self::Inlined8(x) => x,
            Self::Inlined9(x) => x,
            Self::Inlined10(x) => x,
            Self::Inlined11(x) => x,
            Self::Inlined12(x) => x,
            Self::Inlined13(x) => x,
            Self::Inlined14(x) => x,
            Self::Inlined15(x) => x,
            _ => panic!("Not an inline variant"),
        }
    }

    /// Returns a shared reference to the underlying string slice.
    pub fn as_str(&self) -> &str {
        match self {
            Self::HeapAllocated { ptr, len } | Self::StaticBorrowed { ptr, len } => {
                // SAFETY: The pointer and length represent a valid UTF-8 string slice,
                // guaranteed by the constructors which only accept valid Rust `str`s.
                unsafe {
                    let slice = std::slice::from_raw_parts(ptr.as_ptr(), len.get().into());
                    std::str::from_utf8_unchecked(slice)
                }
            }
            _ => {
                let bytes = self.inline_bytes();
                // SAFETY: Inline variants are only constructed from valid Rust `str`s, which are
                // guaranteed to be valid UTF-8.
                unsafe { std::str::from_utf8_unchecked(bytes) }
            }
        }
    }
}

impl From<Repr> for String {
    /// For the `HeapAllocated` variant, this operation is **zero-copy and O(1)** as it directly
    /// reuses the heap buffer. For other variants, it allocates a new `String` and copies the
    /// bytes.
    fn from(repr: Repr) -> Self {
        match repr {
            Repr::HeapAllocated { ptr, len } => {
                let _this = std::mem::ManuallyDrop::new(repr);
                let slice = std::ptr::slice_from_raw_parts_mut(ptr.as_ptr(), len.get().into());
                // SAFETY:
                // 1. `ptr` was obtained from `Box::into_raw` of a `Box<[u8]>` in
                //    `new_heap_allocated`, and `len` is the original length, so reconstructing the
                //    Box is safe.
                // 2. The original bytes were validated as UTF-8 on creation, so they remain valid
                //    UTF-8.
                unsafe {
                    let boxed = Box::from_raw(slice);
                    String::from_utf8_unchecked(boxed.into_vec())
                }
            }
            _ => repr.as_str().to_string(),
        }
    }
}

impl Drop for Repr {
    fn drop(&mut self) {
        match self {
            Self::HeapAllocated { ptr, len } => {
                let slice = std::ptr::slice_from_raw_parts_mut(ptr.as_ptr(), len.get().into());
                // SAFETY: `ptr` and `len` were obtained from `Box::into_raw` of a `Box<[u8]>` in
                // `new_heap_allocated`, so reconstructing and dropping the Box is safe and prevents
                // leaks.
                unsafe {
                    let _ = Box::from_raw(slice);
                }
            }
            _ => {}
        }
    }
}

impl Clone for Repr {
    fn clone(&self) -> Self {
        match self {
            Self::HeapAllocated { ptr, len } => {
                // SAFETY: `ptr` and `len` represent a valid allocated slice, guaranteed by the
                // constructor.
                let slice = unsafe { std::slice::from_raw_parts(ptr.as_ptr(), len.get().into()) };
                let boxed = Box::from(slice);
                Self::new_heap_allocated(boxed)
            }
            Self::StaticBorrowed { ptr, len } => Self::StaticBorrowed { ptr: *ptr, len: *len },
            Self::Inlined1(x) => Self::Inlined1(*x),
            Self::Inlined2(x) => Self::Inlined2(*x),
            Self::Inlined3(x) => Self::Inlined3(*x),
            Self::Inlined4(x) => Self::Inlined4(*x),
            Self::Inlined5(x) => Self::Inlined5(*x),
            Self::Inlined6(x) => Self::Inlined6(*x),
            Self::Inlined7(x) => Self::Inlined7(*x),
            Self::Inlined8(x) => Self::Inlined8(*x),
            Self::Inlined9(x) => Self::Inlined9(*x),
            Self::Inlined10(x) => Self::Inlined10(*x),
            Self::Inlined11(x) => Self::Inlined11(*x),
            Self::Inlined12(x) => Self::Inlined12(*x),
            Self::Inlined13(x) => Self::Inlined13(*x),
            Self::Inlined14(x) => Self::Inlined14(*x),
            Self::Inlined15(x) => Self::Inlined15(*x),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_static_str() {
        // Inline variants 1 to 15
        assert!(matches!(Repr::from_static_str("1"), Repr::Inlined1(_)));
        assert!(matches!(Repr::from_static_str("123456789012345"), Repr::Inlined15(_)));

        // StaticBorrowed for > 15
        let repr = Repr::from_static_str("1234567890123456");
        assert!(matches!(repr, Repr::StaticBorrowed { .. }));
        assert_eq!(repr.as_str(), "1234567890123456");

        // Max length (255)
        let max_str = std::str::from_utf8(&[b'a'; 255]).unwrap();
        let repr_max = Repr::from_static_str(max_str);
        assert!(matches!(repr_max, Repr::StaticBorrowed { .. }));
        assert_eq!(repr_max.as_str(), max_str);
    }

    #[test]
    fn test_from_str() {
        // Inline variants 1 to 15
        assert!(matches!(Repr::from_str("1"), Repr::Inlined1(_)));
        assert!(matches!(Repr::from_str("123456789012345"), Repr::Inlined15(_)));

        // HeapAllocated for > 15
        let repr = Repr::from_str("1234567890123456");
        assert!(matches!(repr, Repr::HeapAllocated { .. }));
        assert_eq!(repr.as_str(), "1234567890123456");

        // Max length (255)
        let max_str = "a".repeat(255);
        let repr_max = Repr::from_str(&max_str);
        assert!(matches!(repr_max, Repr::HeapAllocated { .. }));
        assert_eq!(repr_max.as_str(), max_str);
    }

    #[test]
    fn test_from_string() {
        // Inline variants 1 to 15
        assert!(matches!(Repr::from_string("1".to_string()), Repr::Inlined1(_)));
        assert!(matches!(Repr::from_string("123456789012345".to_string()), Repr::Inlined15(_)));

        // HeapAllocated for > 15
        let repr = Repr::from_string("1234567890123456".to_string());
        assert!(matches!(repr, Repr::HeapAllocated { .. }));
        assert_eq!(repr.as_str(), "1234567890123456");

        // Max length (255)
        let max_str = "a".repeat(255);
        let repr_max = Repr::from_string(max_str.clone());
        assert!(matches!(repr_max, Repr::HeapAllocated { .. }));
        assert_eq!(repr_max.as_str(), max_str);
    }

    #[test]
    fn test_as_str() {
        let repr = Repr::from_str("inline");
        assert_eq!(repr.as_str(), "inline");

        let repr = Repr::from_static_str("static_borrow_large");
        assert_eq!(repr.as_str(), "static_borrow_large");

        let repr = Repr::from_str("heap_allocated_large_string");
        assert_eq!(repr.as_str(), "heap_allocated_large_string");
    }

    #[test]
    fn test_from_repr_for_string() {
        // Inline
        let repr = Repr::from_str("inline");
        let ptr_before = repr.as_str().as_ptr();
        let string = String::from(repr);
        assert_eq!(string, "inline");
        assert_ne!(ptr_before, string.as_ptr());

        // StaticBorrow
        let repr = Repr::from_static_str("static_borrow_large");
        let ptr_before = repr.as_str().as_ptr();
        let string = String::from(repr);
        assert_eq!(string, "static_borrow_large");
        assert_ne!(ptr_before, string.as_ptr());

        // Allocated (should reuse buffer)
        let repr = Repr::from_string("heap_allocated_large_string".to_string());
        let ptr_before = repr.as_str().as_ptr();
        let string = String::from(repr);
        assert_eq!(string, "heap_allocated_large_string");
        assert_eq!(ptr_before, string.as_ptr());
    }

    #[test]
    fn test_clone() {
        // Inline: contents are equal, but pointers must be different because the bytes are stored
        // inline in distinct stack instances.
        let repr = Repr::from_str("inline");
        let cloned = repr.clone();
        assert!(matches!(cloned, Repr::Inlined6(_)));
        assert_eq!(repr.as_str(), cloned.as_str());
        assert_ne!(repr.as_str().as_ptr(), cloned.as_str().as_ptr());

        // StaticBorrowed: pointers must be identical because they borrow the same static memory
        // without allocation.
        let repr = Repr::from_static_str("static_borrow_large");
        let cloned = repr.clone();
        assert!(matches!(cloned, Repr::StaticBorrowed { .. }));
        assert_eq!(repr.as_str(), cloned.as_str());
        assert_eq!(repr.as_str().as_ptr(), cloned.as_str().as_ptr());

        // HeapAllocated: contents are equal, but pointers must be different because cloning
        // allocates a new heap buffer.
        let repr = Repr::from_str("heap_allocated_large_string");
        let cloned = repr.clone();
        assert!(matches!(cloned, Repr::HeapAllocated { .. }));
        assert_eq!(repr.as_str(), cloned.as_str());
        assert_ne!(repr.as_str().as_ptr(), cloned.as_str().as_ptr());
    }

    #[test]
    fn test_allocated_drop_and_clone_independence() {
        let repr = Repr::from_string("heap_allocated_large_string_for_drop_test".to_string());
        assert!(matches!(repr, Repr::HeapAllocated { .. }));

        let cloned = repr.clone();

        // Drop the original. This must deallocate the original buffer. The cloned buffer must
        // remain valid (deep copy).
        drop(repr);

        assert_eq!(cloned.as_str(), "heap_allocated_large_string_for_drop_test");

        // Drop the clone. This must deallocate the cloned buffer.
        drop(cloned);
    }

    #[test]
    #[should_panic]
    fn test_from_static_str_empty_panics() {
        Repr::from_static_str("");
    }

    #[test]
    #[should_panic]
    fn test_from_static_str_too_long_panics() {
        let leaked_str = std::str::from_utf8(&[b'a'; 256]).unwrap();
        Repr::from_static_str(leaked_str);
    }

    #[test]
    #[should_panic]
    fn test_from_str_empty_panics() {
        Repr::from_str("");
    }

    #[test]
    #[should_panic]
    fn test_from_str_too_long_panics() {
        let large_str = "a".repeat(256);
        Repr::from_str(&large_str);
    }

    #[test]
    #[should_panic]
    fn test_from_string_empty_panics() {
        Repr::from_string(String::new());
    }

    #[test]
    #[should_panic]
    fn test_from_string_too_long_panics() {
        let large_str = "a".repeat(256);
        Repr::from_string(large_str);
    }
}
