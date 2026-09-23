// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Get the size of a field in a struct.
///
/// This is a compile-time equivalent of `std::mem::size_of_val(&value.field)`.
///
/// Usage:
///
/// ```
/// struct MyStruct {
///     field1: u8,
///     nested: Nested,
/// }
///
/// struct Nested {
///     field2: u32,
/// }
///
/// const FIELD1_SIZE: usize = size_of_field!(MyStruct, field1);
/// const FIELD2_SIZE: usize = size_of_field!(MyStruct, nested.field2);
/// ```
#[macro_export]
macro_rules! size_of_field {
    ($type_name:ty, $($field:ident).+) => {{
        const fn size_of_pointee<T>(_val: *const T) -> usize {
            std::mem::size_of::<T>()
        }
        const fn compute() -> usize {
            let p = ::std::mem::MaybeUninit::<$type_name>::uninit();
            // SAFETY
            // This operation is safe because the pointer is never dereferenced. This is making the
            // compiler compute the type of the field and then using function specialization to
            // compute the size of the type.
            // Moreover, this is only used in a const context, so the value will be compiled at
            // compilation time.
            size_of_pointee(unsafe { &raw const (*p.as_ptr()) . $($field).+ })
        }
        const size: usize = compute();
        size
    }};
}

/// Ensure 2 different types have the same layout.
///
/// Usage:
///
/// ```
/// check_same_layout! {
///     Type1 = Type2 {
///         type1_field1 => type2_field1,
///         type1_field2 => type2_field2,
///         ...
///     }
/// }
/// ```
#[macro_export]
macro_rules! check_same_layout {
    {} => {};
    {
        $type_name1:ty = $type_name2:ty
        {
            $(
                $($field1:ident).+ => $($field2:ident).+
            ),*
            $(,)?
        }
        $($token:tt)*
    } => {
        $crate::__static_assertions::assert_eq_size!($type_name1, $type_name2);
        $(
            $crate::__static_assertions::const_assert_eq!(
                std::mem::offset_of!($type_name1, $($field1).+),
                std::mem::offset_of!($type_name2, $($field2).+)
            );
        )*
        $crate::check_same_layout! { $($token)* }
    };
}

/// Ensure a uapi type has the same layout in 32 and 64 bits.
///
/// Usage:
///
/// ```
/// check_arch_independent_layout! {
///     UapiType {
///         field1,
///         field2,
///         ...
///     }
/// }
/// ```
#[macro_export]
macro_rules! check_arch_independent_layout {
    {} => {};
    {
        $type_name:ident {
            $( $($field:ident).+ ),*
            $(,)?
        }
        $($token:tt)*
    }=> {
        $crate::check_same_layout! {
            $crate::$type_name = $crate::arch32::$type_name {
                $(
                    $($field).+ => $($field).+,
                )*
            }
        }
        $crate::check_arch_independent_layout! { $($token)* }
    };
}

/// Ensure a custom type has the same layout as an ABI independant uapi type
///
/// Usage:
///
/// ```
/// check_arch_independent_same_layout! {
///     Type = UapiType {
///         type_field1 => uapi_type_field1,
///         type_field2 => uapi_type_field2,
///         ...
///     }
/// }
/// ```
#[macro_export]
macro_rules! check_arch_independent_same_layout {
    {} => {};
    {
        $type_name1:ty = $type_name2:ident
        {
            $(
                $($field1:ident).+ => $($field2:ident).+
            ),*
            $(,)?
        }
        $($token:tt)*
    } => {
        $crate::check_same_layout! {
            $type_name1 = $crate::$type_name2
            {
                $($($field1).+ => $($field2).+,)*
            }
        }
        $crate::check_same_layout! {
            $type_name1 = $crate::arch32::$type_name2
            {
                $($($field1).+ => $($field2).+,)*
            }
        }
        $crate::check_arch_independent_same_layout! { $($token)* }
    };
}

/// Implement From/TryFrom between 2 structs.
///
/// This defined 4 constructs:
/// - From / TryFrom implements the equivalent trait
/// - BidiFrom / BidiTryFrom implements the trait in both direction
#[macro_export]
macro_rules! translate_data {
    {} => {};
    {
        $(#[$meta:meta])*
        BidiFrom<$type_name1:ty, $type_name2:ty> {
            $(
                $field1:ident = $field2:ident;
            )*
            $(..$($d1:expr)?, $($d2:expr)?)?
        }
        $($token:tt)*
    } => {
        $crate::translate_data! {
            $(#[$meta])*
            From<$type_name1> for $type_name2 {
                $( $field2 = $field1; )*
                $($(..$d2)?)?
            }
            $(#[$meta])*
            From<$type_name2> for $type_name1 {
                $( $field1 = $field2; )*
                $($(..$d1)?)?
            }
        }
        $crate::translate_data! { $($token)* }
    };
    {
        $(#[$meta:meta])*
        BidiTryFrom<$type_name1:ty, $type_name2:ty> {
            $(
                $field1:ident = $field2:ident;
            )*
            $(..$($d1:expr)?, $($d2:expr)?)?
        }
        $($token:tt)*
    } => {
        $crate::translate_data! {
            $(#[$meta])*
            TryFrom<$type_name1> for $type_name2 {
                $( $field2 = $field1; )*
                $($(..$d2)?)?
            }
            $(#[$meta])*
            TryFrom<$type_name2> for $type_name1 {
                $( $field1 = $field2; )*
                $($(..$d1)?)?
            }
        }
        $crate::translate_data! { $($token)* }
    };
    {
        $(#[$meta:meta])*
        From<$type_name1:ty> for $type_name2:ty {
            $(
                $field2:ident = $field1:ident;
            )*
            $(..$d:expr)?
        }
        $($token:tt)*
    } => {
        $(#[$meta])*
        impl From<$type_name1> for $type_name2 {
            fn from(src: $type_name1) -> Self {
                Self {
                    $( $field2: src.$field1.into(), )*
                    $(..$d)?
                }
            }
        }
        $crate::translate_data! { $($token)* }
    };
    {
        $(#[$meta:meta])*
        TryFrom<$type_name1:ty> for $type_name2:ty {
            $(
                $field2:ident = $field1:ident $( ( $field1_default:expr ))?;
            )*
            $(..$d:expr)?
        }
        $($token:tt)*
    } => {
        $(#[$meta])*
        impl TryFrom<$type_name1> for $type_name2 {
            type Error = ();
            fn try_from(src: $type_name1) -> Result<Self, ()> {
                Ok(Self {
                    $( $field2: $crate::translate_data_expr!( src.$field1 $( ( $field1_default ) )? ), )*
                    $(..$d)?
                })
            }
        }
        $crate::translate_data! { $($token)* }
    };
}

#[macro_export]
macro_rules! translate_data_expr {
    ( $src:ident . $field:ident ) => {
        $src.$field.try_into().map_err(|_| ())?
    };
    ( $src:ident . $field:ident ( $field1_default:expr ) ) => {
        $src.$field.try_into().unwrap_or($field1_default)
    };
}

/// Implement From/TryFrom between 2 uapi struct of different ABI.
///
/// This defined 3 constructs:
/// - TryFrom64 transform a 64 bits uapi struct into a 32 bits one
/// - From32 transform a 32 bits uapi struct into a 64 bits one
/// - BidiFrom implements both of these operations
#[macro_export]
macro_rules! arch_translate_data {
    {} => {};
    {
        BidiFrom<$type_name:ident> {
            $( $field:ident ),+
            $(,)?
        }
        $($token:tt)*
    } => {
        $crate::arch_translate_data! {
            TryFrom64<$type_name> {
                $(
                    $field,
                )*
            }
            From32<$type_name> {
                $(
                    $field,
                )*
            }
        }
        $crate::arch_translate_data! { $($token)* }
    };
    {
        TryFrom64<$type_name:ident> {
            $( $field:ident ),+
            $(,)?
        }
        $($token:tt)*
    } => {
        $crate::translate_data! {
            #[cfg(target_arch = "aarch64")]
            TryFrom<$crate::$type_name> for $crate::arch32::$type_name {
                $(
                    $field = $field;
                )*
                ..Default::default()
            }
        }
        $crate::arch_translate_data! { $($token)* }
    };
    {
        From32<$type_name:ident> {
            $( $field:ident ),+
            $(,)?
        }
        $($token:tt)*
    } => {
        $crate::translate_data! {
            #[cfg(target_arch = "aarch64")]
            From<$crate::arch32::$type_name> for $crate::$type_name {
                $(
                    $field = $field;
                )*
                ..Default::default()
            }
        }
        $crate::arch_translate_data! { $($token)* }
    };
}

/// Implement TryFrom between 2 uapi struct of different ABI with a common type.
#[macro_export]
macro_rules! arch_map_data {
    {} => {};
    {
        BidiTryFrom<$type_name1:ty, $type_name2:ident> {
            $(
                $field1:ident = $field2:ident;
            )*
            $(..$d:expr)?
        }
        $($token:tt)*
    } => {
        $crate::translate_data! {
            BidiTryFrom<$type_name1, $crate::$type_name2> {
                $(
                    $field1 = $field2;
                )*
                ..$($d)?, Default::default()
            }
            #[cfg(target_arch = "aarch64")]
            BidiTryFrom<$type_name1, $crate::arch32::$type_name2> {
                $(
                    $field1 = $field2;
                )*
                ..$($d)?, Default::default()
            }
        }
        $crate::arch_map_data! { $($token)* }
    };
}
