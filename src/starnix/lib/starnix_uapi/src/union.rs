// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::user_address::ArchSpecific;

/// Container that contains a union type that differs depending on the 32 vs 64 bit architecture.
#[derive(Copy, Clone)]
pub enum ArchSpecificUnionContainer<T64, T32> {
    Arch64(T64),
    #[allow(dead_code)]
    Arch32(T32),
}

impl<T64, T32> ArchSpecific for ArchSpecificUnionContainer<T64, T32> {
    fn is_arch32(&self) -> bool {
        !matches!(self, Self::Arch64(_))
    }
}

/// Initializes the given fields of a struct or union and returns the bytes of the
/// resulting object as a byte array.
///
/// `struct_with_union_into_bytes` is invoked like so:
///
/// ```rust,ignore
/// union Foo {
///     a: u8,
///     b: u16,
/// }
///
/// struct Bar {
///     a: Foo,
///     b: u8,
///     c: u16,
/// }
///
/// struct_with_union_into_bytes!(Bar { a.b: 1, b: 2, c: 3 })
/// ```
///
/// Each named field is initialized with a value whose type must implement
/// `zerocopy::IntoBytes`. Any fields which are not explicitly initialized will be left as
/// all zeroes.
#[macro_export]
macro_rules! struct_with_union_into_bytes {
    ($ty:ty { $($($field:ident).*: $value:expr,)* }) => {{
        use std::mem::MaybeUninit;

        const BYTES: usize = std::mem::size_of::<$ty>();

        struct AlignedBytes {
            bytes: [u8; BYTES],
            _align: MaybeUninit<$ty>,
        }

        let mut bytes = AlignedBytes { bytes: [0; BYTES], _align: MaybeUninit::uninit() };

        $({
            // Evaluate `$value` once to make sure it has the same type
            // when passed to `type_check_as_bytes` as when assigned to
            // the field.
            let value = $value;
            if false {
                fn type_check_as_bytes<T: zerocopy::IntoBytes>(_: T) {
                    unreachable!()
                }
                type_check_as_bytes(value);
            } else {
                // SAFETY: We only treat these zeroed bytes as a `$ty` for the purposes of
                // overwriting the given field. Thus, it's OK if a sequence of zeroes is
                // not a valid instance of `$ty` or if the sub-sequence of zeroes is not a
                // valid instance of the type of the field being overwritten. Note that we
                // use `std::ptr::write`, not normal field assignment, as the latter would
                // treat the current field value (all zeroes) as an initialized instance of
                // the field's type (in order to drop it), which would be unsound.
                //
                // Since we know from the preceding `if` branch that the type of `value` is
                // `IntoBytes`, we know that no uninitialized bytes will be written to the
                // field. That, combined with the fact that the entire `bytes.bytes` is
                // initialized to zero, ensures that all bytes of `bytes.bytes` are
                // initialized, so we can safely return `bytes.bytes` as a byte array.
                unsafe {
                    std::ptr::write(std::ptr::addr_of_mut!((*(&mut bytes.bytes as *mut [u8; BYTES] as *mut $ty)).$($field).*), value);
                }
            }
        })*

        bytes.bytes
    }};
}

/// Initializes the given fields of a struct or union and returns the union as a
/// `ArchSpecificUnionContainer`.
///
/// `arch_struct_with_union` is invoked like `struct_with_union_into_bytes`, but the first
/// parameter of the macro must be an expression returning an `ArchSpecific` used to decide which
/// macro to instantiate.
#[macro_export]
macro_rules! arch_struct_with_union {
    ($arch:expr, $ty:ident { $($token:tt)* }) => {{
        if $arch.is_arch32() {
            let v32: starnix_uapi::arch32::$ty = zerocopy::transmute!(struct_with_union_into_bytes! {
                starnix_uapi::arch32::$ty {
                    $($token)*
                }
            });
            $crate::union::ArchSpecificUnionContainer::<starnix_uapi::$ty, starnix_uapi::arch32::$ty>::Arch32(v32)
        } else {
            let v64: starnix_uapi::$ty = zerocopy::transmute!(struct_with_union_into_bytes! {
                starnix_uapi::$ty {
                    $($token)*
                }
            });
            $crate::union::ArchSpecificUnionContainer::<starnix_uapi::$ty, starnix_uapi::arch32::$ty>::Arch64(v64)
        }
    }};
}

/// Build the wrapper type around a UABI specific union.
///
/// `arch_union_wrapper` is invoked like so:
///
/// ```rust,ignore
/// arch_union_wrapper! {
///     Wrapper(wrappee_name);
/// }
/// ```
///
/// where `Wrapper` is the name of the wrapper type being generated, and `wrappee_name` is the
/// identifier of the union type of wrap.
/// This will allow to read and write `Wrapper` object using the arch specific read/write methods
/// of the memory manager passing a `WrapperPtr` reference.
#[macro_export]
macro_rules! arch_union_wrapper {
    {} => {};
    {
        $vis:vis $wrapper:ident( $ty:ident );
        $($token:tt)*
    } => {
        paste::paste! {
            $vis type [<$wrapper Inner>] =
                $crate::union::ArchSpecificUnionContainer::<
                    starnix_uapi::$ty,
                    starnix_uapi::arch32::$ty>;

            $vis struct $wrapper([<$wrapper Inner>]);

            $vis type [<$wrapper Ptr>] =
                starnix_uapi::user_address::MappingMultiArchUserRef<
                    $wrapper,
                    [u8; std::mem::size_of::<starnix_uapi::$ty>()],
                    [u8; std::mem::size_of::<starnix_uapi::arch32::$ty>()]>;

            impl $wrapper {
                fn inner(&self) -> &[<$wrapper Inner>] {
                    &self.0
                }
            }

            impl From<[u8; std::mem::size_of::<starnix_uapi::$ty>()]> for $wrapper {
                fn from(bytes: [u8; std::mem::size_of::<starnix_uapi::$ty>()]) -> Self {
                    Self([<$wrapper Inner>]::Arch64(zerocopy::transmute!(bytes)))
                }
            }

            #[cfg(target_arch = "aarch64")]
            impl From<[u8; std::mem::size_of::<starnix_uapi::arch32::$ty>()]> for $wrapper {
                fn from(bytes: [u8; std::mem::size_of::<starnix_uapi::arch32::$ty>()]) -> Self {
                    Self([<$wrapper Inner>]::Arch32(zerocopy::transmute!(bytes)))
                }
            }

            impl TryFrom<$wrapper> for [u8; std::mem::size_of::<starnix_uapi::$ty>()] {
                type Error = ();
                fn try_from(v: $wrapper) -> Result<Self, ()> {
                    if let [<$wrapper Inner>]::Arch64(v) = v.0 {
                        // SAFETY: All union used by this wrapper are generated from bytes, so
                        // there is no uninitialized data.
                        Ok(unsafe { std::mem::transmute(v) })
                    } else {
                        Err(())
                    }
                }
            }

            #[cfg(target_arch = "aarch64")]
            impl TryFrom<$wrapper> for [u8; std::mem::size_of::<starnix_uapi::arch32::$ty>()] {
                type Error = ();
                fn try_from(v: $wrapper) -> Result<Self, ()> {
                    if let [<$wrapper Inner>]::Arch32(v) = v.0 {
                        // SAFETY: All union used by this wrapper are generated from bytes, so
                        // there is no uninitialized data.
                        Ok(unsafe { std::mem::transmute(v) })
                    } else {
                        Err(())
                    }
                }
            }

            impl $crate::user_address::ArchSpecific for $wrapper {
                fn is_arch32(&self) -> bool {
                    self.0.is_arch32()
                }
            }

            impl std::fmt::Debug for $wrapper {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                    let mut s = if self.is_arch32() {
                        f.debug_tuple(std::stringify!([<$wrapper 32>]))
                    } else {
                        f.debug_tuple(std::stringify!([<$wrapper 64>]))
                    };
                    // SAFETY: All union used by this wrapper are generated from bytes, so
                    // there is no uninitialized data.
                    match self.inner () {
                        [<$wrapper Inner>]::Arch64(v) => {
                            let bytes: &[u8; std::mem::size_of::<starnix_uapi::$ty>()] =
                                unsafe { std::mem::transmute(v) };
                            s.field(bytes);
                        }
                        [<$wrapper Inner>]::Arch32(v) => {
                            let bytes: &[u8; std::mem::size_of::<starnix_uapi::arch32::$ty>()] =
                                unsafe { std::mem::transmute(v) };
                            s.field(bytes);
                        }
                    }
                    s.finish()
                }
            }
        }

        $crate::arch_union_wrapper! { $($token)* }
    };
}

pub use arch_struct_with_union;
pub use arch_union_wrapper;
pub use struct_with_union_into_bytes;
