// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(not(target_endian = "little"))]
compile_error!("only little-endian targets are supported by FIDL");

// Standard library traits

macro_rules! impl_unop {
    ($trait:ident:: $fn:ident for $name:ident : $prim:ty) => {
        impl ::core::ops::$trait for $name {
            type Output = <$prim as ::core::ops::$trait>::Output;

            #[inline]
            fn $fn(self) -> Self::Output {
                self.0.$fn()
            }
        }
    };
}

macro_rules! impl_binop_one {
    ($trait:ident:: $fn:ident($self:ty, $other:ty) -> $output:ty) => {
        impl ::core::ops::$trait<$other> for $self {
            type Output = $output;

            #[inline]
            fn $fn(self, other: $other) -> Self::Output {
                self.0.$fn(other.0)
            }
        }
    };
}

macro_rules! impl_binop_both {
    ($trait:ident:: $fn:ident($self:ty, $other:ty) -> $output:ty) => {
        impl ::core::ops::$trait<$other> for $self {
            type Output = $output;

            #[inline]
            fn $fn(self, other: $other) -> Self::Output {
                self.0.$fn(other)
            }
        }

        impl ::core::ops::$trait<$self> for $other {
            type Output = $output;

            #[inline]
            fn $fn(self, other: $self) -> Self::Output {
                self.$fn(other.0)
            }
        }
    };
}

macro_rules! impl_binop {
    ($trait:ident::$fn:ident for $name:ident: $prim:ty) => {
        impl_binop_both!($trait::$fn ($name, $prim) -> $prim);
        impl_binop_both!($trait::$fn (&'_ $name, $prim) -> $prim);
        impl_binop_both!($trait::$fn ($name, &'_ $prim) -> $prim);
        impl_binop_both!($trait::$fn (&'_ $name, &'_ $prim) -> $prim);

        impl_binop_one!($trait::$fn ($name, $name) -> $prim);
        impl_binop_one!($trait::$fn (&'_ $name, $name) -> $prim);
        impl_binop_one!($trait::$fn ($name, &'_ $name) -> $prim);
        impl_binop_one!($trait::$fn (&'_ $name, &'_ $name) -> $prim);
    };
}

macro_rules! impl_binassign {
    ($trait:ident:: $fn:ident for $name:ident : $prim:ty) => {
        impl ::core::ops::$trait<$prim> for $name {
            #[inline]
            fn $fn(&mut self, other: $prim) {
                let mut value = self.0;
                value.$fn(other);
                *self = Self(value);
            }
        }

        impl ::core::ops::$trait<$name> for $name {
            #[inline]
            fn $fn(&mut self, other: $name) {
                let mut value = self.0;
                value.$fn(other.0);
                *self = Self(value);
            }
        }

        impl ::core::ops::$trait<&'_ $prim> for $name {
            #[inline]
            fn $fn(&mut self, other: &'_ $prim) {
                let mut value = self.0;
                value.$fn(other);
                *self = Self(value);
            }
        }

        impl ::core::ops::$trait<&'_ $name> for $name {
            #[inline]
            fn $fn(&mut self, other: &'_ $name) {
                let mut value = self.0;
                value.$fn(other.0);
                *self = Self(value);
            }
        }
    };
}

macro_rules! impl_clone_and_copy {
    (for $name:ident) => {
        impl Clone for $name {
            #[inline]
            fn clone(&self) -> Self {
                *self
            }
        }

        impl Copy for $name {}
    };
}

macro_rules! impl_fmt {
    ($trait:ident for $name:ident) => {
        impl ::core::fmt::$trait for $name {
            #[inline]
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::$trait::fmt(&self.0, f)
            }
        }
    };
}

macro_rules! impl_default {
    (for $name:ident : $prim:ty) => {
        impl Default for $name {
            #[inline]
            fn default() -> Self {
                Self(<$prim>::default())
            }
        }
    };
}

macro_rules! impl_from {
    (for $name:ident : $prim:ty) => {
        impl From<$prim> for $name {
            fn from(value: $prim) -> Self {
                Self(value)
            }
        }

        impl<'a> From<&'a $prim> for $name {
            fn from(value: &'a $prim) -> Self {
                Self(*value)
            }
        }

        impl From<$name> for $prim {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl<'a> From<&'a $name> for $prim {
            fn from(value: &'a $name) -> Self {
                value.0
            }
        }
    };
}

macro_rules! impl_try_from_ptr_size {
    ($size:ident for $name:ident: $prim:ident) => {
        impl TryFrom<$size> for $name {
            type Error = <$prim as TryFrom<$size>>::Error;

            #[inline]
            fn try_from(value: $size) -> Result<Self, Self::Error> {
                Ok(Self(<$prim>::try_from(value)?))
            }
        }

        impl_try_into_ptr_size!($size for $name: $prim);
    };
}

macro_rules! impl_try_into_ptr_size {
    (isize for $name:ident: i16) => {
        impl_into_ptr_size!(isize for $name);
    };

    (usize for $name:ident: u16) => {
        impl_into_ptr_size!(usize for $name);
    };

    ($size:ident for $name:ident: $prim:ident) => {
        impl TryFrom<$name> for $size {
            type Error = <$size as TryFrom<$prim>>::Error;

            #[inline]
            fn try_from(value: $name) -> Result<Self, Self::Error> {
                <$size>::try_from(value.0)
            }
        }
    };
}

macro_rules! impl_into_ptr_size {
    ($size:ident for $name:ident) => {
        impl From<$name> for $size {
            #[inline]
            fn from(value: $name) -> Self {
                <$size>::from(value.0)
            }
        }
    };
}

macro_rules! impl_hash {
    (for $name:ident) => {
        impl core::hash::Hash for $name {
            fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
                self.0.hash(state);
            }
        }
    };
}

macro_rules! impl_partial_ord_and_ord {
    (for $name:ident : $prim:ty) => {
        impl PartialOrd for $name {
            #[inline]
            fn partial_cmp(&self, other: &Self) -> Option<::core::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        impl PartialOrd<$prim> for $name {
            #[inline]
            fn partial_cmp(&self, other: &$prim) -> Option<::core::cmp::Ordering> {
                self.0.partial_cmp(other)
            }
        }

        impl Ord for $name {
            #[inline]
            fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
                self.0.cmp(&other.0)
            }
        }
    };
}

macro_rules! impl_partial_eq_and_eq {
    (for $name:ident : $prim:ty) => {
        impl PartialEq for $name {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                let lhs = self.0;
                let rhs = other.0;
                lhs.eq(&rhs)
            }
        }

        impl PartialEq<$prim> for $name {
            #[inline]
            fn eq(&self, other: &$prim) -> bool {
                self.0.eq(other)
            }
        }

        impl Eq for $name {}
    };
}

macro_rules! impl_partial_ord {
    (for $name:ident : $prim:ty) => {
        impl PartialOrd for $name {
            #[inline]
            fn partial_cmp(&self, other: &Self) -> Option<::core::cmp::Ordering> {
                self.0.partial_cmp(&other.0)
            }
        }

        impl PartialOrd<$prim> for $name {
            #[inline]
            fn partial_cmp(&self, other: &$prim) -> Option<::core::cmp::Ordering> {
                self.0.partial_cmp(other)
            }
        }
    };
}

macro_rules! impl_product_and_sum {
    (for $name:ident) => {
        impl ::core::iter::Product for $name {
            #[inline]
            fn product<I: Iterator<Item = Self>>(iter: I) -> Self {
                Self(iter.map(|x| x.0).product())
            }
        }

        impl ::core::iter::Sum for $name {
            #[inline]
            fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
                Self(iter.map(|x| x.0).sum())
            }
        }
    };
}

macro_rules! impl_fidl_convert {
    (for $name:ty : $prim:ty) => {
        impl $crate::FromWire<$name> for $prim {
            const COPY_OPTIMIZATION: $crate::CopyOptimization<$name, $prim> =
                $crate::CopyOptimization::<$name, $prim>::PRIMITIVE;

            #[inline]
            fn from_wire(wire: $name) -> Self {
                wire.into()
            }
        }

        impl $crate::FromWireRef<$name> for $prim {
            #[inline]
            fn from_wire_ref(wire: &$name) -> Self {
                (*wire).into()
            }
        }

        impl $crate::IntoNatural for $name {
            type Natural = $prim;
        }
    };
}

macro_rules! impl_fidl_constrained {
    (for $name:ty) => {
        impl $crate::Constrained for $name {
            type Constraint = ();

            fn validate(
                _: $crate::Slot<'_, Self>,
                _: Self::Constraint,
            ) -> Result<(), $crate::ValidationError> {
                Ok(())
            }
        }
    };
}

macro_rules! impl_fidl_copy_optimize {
    (for $name:ty) => {
        impl $crate::CopyOptimization<$name, $name> {
            /// Whether copy optimization between the two primitive types is
            /// enabled.
            pub const PRIMITIVE: Self = Self::identity();
        }
    };

    (for $name:ty : $prim:ty) => {
        impl_fidl_copy_optimize!(for $name);

        impl $crate::CopyOptimization<$prim, $name> {
            /// Whether copy optimization between the two primitive types is
            /// enabled.
            pub const PRIMITIVE: Self =
                // SAFETY: Copy optimization for primitives is enabled if their
                // size <= 1 or the target is little-endian.
                unsafe {
                    $crate::CopyOptimization::enable_if(
                        size_of::<Self>() <= 1 || cfg!(target_endian = "little"),
                    )
                };
        }

        impl $crate::CopyOptimization<$name, $prim> {
            /// Whether copy optimization between the two primitive types is
            /// enabled.
            pub const PRIMITIVE: Self =
                // SAFETY: Copy optimization between these two primitives is
                // commutative.
                unsafe {
                    $crate::CopyOptimization::enable_if(
                        $crate::CopyOptimization::<$prim, $name>::PRIMITIVE.is_enabled(),
                    )
                };
        }
    }
}

macro_rules! impl_fidl_decode {
    (for $name:ty) => {
        // SAFETY: Primitives have no validation constraints and their wire representation
        // is identical to their Rust representation, so decoding is a no-op.
        unsafe impl<D: ?Sized> $crate::Decode<D> for $name {
            #[inline]
            fn decode(
                _: $crate::Slot<'_, Self>,
                _: &mut D,
                _: (),
            ) -> Result<(), $crate::DecodeError> {
                Ok(())
            }
        }
    };
}

macro_rules! impl_fidl_encode {
    (for $name:ty : $prim:ty) => {
        // SAFETY: Encoding a primitive writes its value directly to the output slot.
        unsafe impl<E: ?Sized> $crate::Encode<$name, E> for $prim {
            const COPY_OPTIMIZATION: $crate::CopyOptimization<$prim, $name> =
                $crate::CopyOptimization::<$prim, $name>::PRIMITIVE;

            #[inline]
            fn encode(
                self,
                encoder: &mut E,
                out: &mut ::core::mem::MaybeUninit<$name>,
                constraint: <$name as $crate::Constrained>::Constraint,
            ) -> Result<(), $crate::EncodeError> {
                $crate::Encode::encode(&self, encoder, out, constraint)
            }
        }

        // SAFETY: Encoding a primitive reference writes its value directly to the output slot.
        unsafe impl<E: ?Sized> $crate::Encode<$name, E> for &$prim {
            #[inline]
            fn encode(
                self,
                _: &mut E,
                out: &mut ::core::mem::MaybeUninit<$name>,
                _: <$name as $crate::Constrained>::Constraint,
            ) -> Result<(), $crate::EncodeError> {
                out.write(<$name>::from(*self));
                Ok(())
            }
        }

        // SAFETY: Encoding an optional primitive delegates to `Box::encode_present` or
        // `Box::encode_absent` which are safe.
        unsafe impl<E> $crate::EncodeOption<$crate::wire::Box<'static, $name>, E> for $prim
        where
            E: $crate::Encoder + ?Sized,
        {
            #[inline]
            fn encode_option(
                this: Option<Self>,
                encoder: &mut E,
                out: &mut ::core::mem::MaybeUninit<$crate::wire::Box<'static, $name>>,
                constraint: <$name as $crate::Constrained>::Constraint,
            ) -> Result<(), $crate::EncodeError> {
                if let Some(value) = this {
                    $crate::EncoderExt::encode_next_with_constraint(encoder, value, constraint)?;
                    $crate::wire::Box::encode_present(out);
                } else {
                    $crate::wire::Box::encode_absent(out);
                }

                Ok(())
            }
        }

        // SAFETY: Encoding an optional primitive reference delegates to the value implementation.
        unsafe impl<E> $crate::EncodeOption<$crate::wire::Box<'static, $name>, E> for &$prim
        where
            E: $crate::Encoder + ?Sized,
        {
            #[inline]
            fn encode_option(
                this: Option<Self>,
                encoder: &mut E,
                out: &mut ::core::mem::MaybeUninit<$crate::wire::Box<'static, $name>>,
                constraint: <$name as $crate::Constrained>::Constraint,
            ) -> Result<(), $crate::EncodeError> {
                <$prim>::encode_option(this.cloned(), encoder, out, constraint)
            }
        }
    };
}

macro_rules! impl_fidl_wire {
    (for $name:ty) => {
        // SAFETY: Primitives have stable layout and no padding.
        unsafe impl $crate::Wire for $name {
            type Narrowed<'de> = Self;

            #[inline]
            fn zero_padding(_: &mut ::core::mem::MaybeUninit<Self>) {}
        }
    };
}

// Builtins

macro_rules! impl_builtin_primitive {
    (for $name:ty) => {
        impl_fidl_convert!(for $name : $name);
        impl_fidl_constrained!(for $name);
        impl_fidl_copy_optimize!(for $name);
        impl_fidl_decode!(for $name);
        impl_fidl_encode!(for $name : $name);
        impl_fidl_wire!(for $name);
    };
}

impl_builtin_primitive!(for u8);
impl_builtin_primitive!(for i8);

// Bool

impl_fidl_convert!(for bool: bool);
impl_fidl_constrained!(for bool);
impl_fidl_copy_optimize!(for bool);

// SAFETY: `bool` is decoded by reading a byte and validating it is 0 or 1.
unsafe impl<D: ?Sized> crate::Decode<D> for bool {
    #[inline]
    fn decode(slot: crate::Slot<'_, Self>, _: &mut D, _: ()) -> Result<(), crate::DecodeError> {
        // SAFETY: `slot` is guaranteed to contain a valid `bool` (1 byte).
        let value = unsafe { slot.as_ptr().cast::<u8>().read() };
        match value {
            0 | 1 => Ok(()),
            invalid => Err(crate::DecodeError::InvalidBool(invalid)),
        }
    }
}

impl_fidl_encode!(for bool: bool);
impl_fidl_wire!(for bool);

// Integers

macro_rules! impl_signed_integer_traits {
    ($name:ident: $prim:ident) => {
        impl_binop!(Add::add for $name: $prim);
        impl_binassign!(AddAssign::add_assign for $name: $prim);
        impl_clone_and_copy!(for $name);
        impl_fmt!(Binary for $name);
        impl_binop!(BitAnd::bitand for $name: $prim);
        impl_binassign!(BitAndAssign::bitand_assign for $name: $prim);
        impl_binop!(BitOr::bitor for $name: $prim);
        impl_binassign!(BitOrAssign::bitor_assign for $name: $prim);
        impl_binop!(BitXor::bitxor for $name: $prim);
        impl_binassign!(BitXorAssign::bitxor_assign for $name: $prim);
        impl_fmt!(Debug for $name);
        impl_default!(for $name: $prim);
        impl_fmt!(Display for $name);
        impl_binop!(Div::div for $name: $prim);
        impl_binassign!(DivAssign::div_assign for $name: $prim);
        impl_from!(for $name: $prim);
        impl_try_from_ptr_size!(isize for $name: $prim);
        impl_hash!(for $name);
        impl_fmt!(LowerExp for $name);
        impl_fmt!(LowerHex for $name);
        impl_binop!(Mul::mul for $name: $prim);
        impl_binassign!(MulAssign::mul_assign for $name: $prim);
        impl_unop!(Neg::neg for $name: $prim);
        impl_unop!(Not::not for $name: $prim);
        impl_fmt!(Octal for $name);
        impl_partial_eq_and_eq!(for $name: $prim);
        impl_partial_ord_and_ord!(for $name: $prim);
        impl_product_and_sum!(for $name);
        impl_binop!(Rem::rem for $name: $prim);
        impl_binassign!(RemAssign::rem_assign for $name: $prim);
        impl_binop!(Shl::shl for $name: $prim);
        impl_binassign!(ShlAssign::shl_assign for $name: $prim);
        impl_binop!(Shr::shr for $name: $prim);
        impl_binassign!(ShrAssign::shr_assign for $name: $prim);
        impl_binop!(Sub::sub for $name: $prim);
        impl_binassign!(SubAssign::sub_assign for $name: $prim);
        impl_fmt!(UpperExp for $name);
        impl_fmt!(UpperHex for $name);

        impl_fidl_convert!(for $name: $prim);
        impl_fidl_constrained!(for $name);
        impl_fidl_copy_optimize!(for $name: $prim);
        impl_fidl_decode!(for $name);
        impl_fidl_encode!(for $name: $prim);
        impl_fidl_encode!(for $name: $name);
        impl_fidl_wire!(for $name);
    };
}

macro_rules! impl_unsigned_integer_traits {
    ($name:ident: $prim:ident) => {
        impl_binop!(Add::add for $name: $prim);
        impl_binassign!(AddAssign::add_assign for $name: $prim);
        impl_clone_and_copy!(for $name);
        impl_fmt!(Binary for $name);
        impl_binop!(BitAnd::bitand for $name: $prim);
        impl_binassign!(BitAndAssign::bitand_assign for $name: $prim);
        impl_binop!(BitOr::bitor for $name: $prim);
        impl_binassign!(BitOrAssign::bitor_assign for $name: $prim);
        impl_binop!(BitXor::bitxor for $name: $prim);
        impl_binassign!(BitXorAssign::bitxor_assign for $name: $prim);
        impl_fmt!(Debug for $name);
        impl_default!(for $name: $prim);
        impl_fmt!(Display for $name);
        impl_binop!(Div::div for $name: $prim);
        impl_binassign!(DivAssign::div_assign for $name: $prim);
        impl_from!(for $name: $prim);
        impl_try_from_ptr_size!(usize for $name: $prim);
        impl_hash!(for $name);
        impl_fmt!(LowerExp for $name);
        impl_fmt!(LowerHex for $name);
        impl_binop!(Mul::mul for $name: $prim);
        impl_binassign!(MulAssign::mul_assign for $name: $prim);
        impl_unop!(Not::not for $name: $prim);
        impl_fmt!(Octal for $name);
        impl_partial_eq_and_eq!(for $name: $prim);
        impl_partial_ord_and_ord!(for $name: $prim);
        impl_product_and_sum!(for $name);
        impl_binop!(Rem::rem for $name: $prim);
        impl_binassign!(RemAssign::rem_assign for $name: $prim);
        impl_binop!(Shl::shl for $name: $prim);
        impl_binassign!(ShlAssign::shl_assign for $name: $prim);
        impl_binop!(Shr::shr for $name: $prim);
        impl_binassign!(ShrAssign::shr_assign for $name: $prim);
        impl_binop!(Sub::sub for $name: $prim);
        impl_binassign!(SubAssign::sub_assign for $name: $prim);
        impl_fmt!(UpperExp for $name);
        impl_fmt!(UpperHex for $name);

        impl_fidl_convert!(for $name: $prim);
        impl_fidl_constrained!(for $name);
        impl_fidl_copy_optimize!(for $name: $prim);
        impl_fidl_decode!(for $name);
        impl_fidl_encode!(for $name: $prim);
        impl_fidl_encode!(for $name: $name);
        impl_fidl_wire!(for $name);
    };
}

macro_rules! impl_float_traits {
    ($name:ident: $prim:ty) => {
        impl_binop!(Add::add for $name: $prim);
        impl_binassign!(AddAssign::add_assign for $name: $prim);
        impl_clone_and_copy!(for $name);
        impl_fmt!(Debug for $name);
        impl_default!(for $name: $prim);
        impl_fmt!(Display for $name);
        impl_binop!(Div::div for $name: $prim);
        impl_binassign!(DivAssign::div_assign for $name: $prim);
        impl_from!(for $name: $prim);
        impl_fmt!(LowerExp for $name);
        impl_binop!(Mul::mul for $name: $prim);
        impl_binassign!(MulAssign::mul_assign for $name: $prim);
        impl_unop!(Neg::neg for $name: $prim);
        impl_partial_eq_and_eq!(for $name: $prim);
        impl_partial_ord!(for $name: $prim);
        impl_product_and_sum!(for $name);
        impl_binop!(Rem::rem for $name: $prim);
        impl_binassign!(RemAssign::rem_assign for $name: $prim);
        impl_binop!(Sub::sub for $name: $prim);
        impl_binassign!(SubAssign::sub_assign for $name: $prim);
        impl_fmt!(UpperExp for $name);

        impl_fidl_convert!(for $name: $prim);
        impl_fidl_constrained!(for $name);
        impl_fidl_copy_optimize!(for $name: $prim);
        impl_fidl_decode!(for $name);
        impl_fidl_encode!(for $name: $prim);
        impl_fidl_encode!(for $name: $name);
        impl_fidl_wire!(for $name);
    };
}

macro_rules! define_newtype {
    ($name:ident: $prim:ty, $align:expr) => {
        #[doc = concat!("A wire-encoded `", stringify!($prim), "`")]
        #[repr(C, align($align))]
        #[derive(zerocopy::FromBytes, zerocopy::IntoBytes)]
        pub struct $name(pub $prim);

        impl ::core::ops::Deref for $name {
            type Target = $prim;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl ::core::ops::DerefMut for $name {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
    };
}

macro_rules! define_signed_integer {
    ($name:ident: $prim:ident, $align:expr) => {
        define_newtype!($name: $prim, $align);
        impl_signed_integer_traits!($name: $prim);
    }
}

define_signed_integer!(Int16: i16, 2);
define_signed_integer!(Int32: i32, 4);
define_signed_integer!(Int64: i64, 8);

macro_rules! define_unsigned_integer {
    ($name:ident: $prim:ident, $align:expr) => {
        define_newtype!($name: $prim, $align);
        impl_unsigned_integer_traits!($name: $prim);
    }
}

define_unsigned_integer!(Uint16: u16, 2);
define_unsigned_integer!(Uint32: u32, 4);
define_unsigned_integer!(Uint64: u64, 8);

macro_rules! define_float {
    ($name:ident: $prim:ident, $align:expr) => {
        define_newtype!($name: $prim, $align);
        impl_float_traits!($name: $prim);
    }
}

define_float!(Float32: f32, 4);
define_float!(Float64: f64, 8);

#[cfg(test)]
mod tests {
    use crate::{DecoderExt as _, EncoderExt as _, chunks, wire};

    #[test]
    fn decode_bool() {
        #![allow(clippy::bool_assert_comparison)]

        assert_eq!(
            chunks![0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
                .as_mut_slice()
                .decode::<bool>()
                .unwrap(),
            true,
        );
        assert_eq!(
            chunks![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
                .as_mut_slice()
                .decode::<bool>()
                .unwrap(),
            false,
        );
    }

    #[test]
    fn encode_bool() {
        assert_eq!(
            Vec::encode(true).unwrap(),
            chunks![0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            Vec::encode(false).unwrap(),
            chunks![0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn decode_ints() {
        assert_eq!(
            chunks![0xa3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
                .as_mut_slice()
                .decode::<u8>()
                .unwrap(),
            0xa3u8,
        );
        assert_eq!(
            chunks![0xbb, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
                .as_mut_slice()
                .decode::<i8>()
                .unwrap(),
            -0x45i8,
        );

        assert_eq!(
            chunks![0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
                .as_mut_slice()
                .decode::<wire::Uint16>()
                .unwrap(),
            0x1234u16,
        );
        assert_eq!(
            chunks![0xcc, 0xed, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
                .as_mut_slice()
                .decode::<wire::Int16>()
                .unwrap(),
            -0x1234i16,
        );

        assert_eq!(
            chunks![0x78, 0x56, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00]
                .as_mut_slice()
                .decode::<wire::Uint32>()
                .unwrap(),
            0x12345678u32,
        );
        assert_eq!(
            chunks![0x88, 0xa9, 0xcb, 0xed, 0x00, 0x00, 0x00, 0x00]
                .as_mut_slice()
                .decode::<wire::Int32>()
                .unwrap(),
            -0x12345678i32,
        );

        assert_eq!(
            chunks![0xf0, 0xde, 0xbc, 0x9a, 0x78, 0x56, 0x34, 0x12]
                .as_mut_slice()
                .decode::<wire::Uint64>()
                .unwrap(),
            0x123456789abcdef0u64,
        );
        assert_eq!(
            chunks![0x10, 0x21, 0x43, 0x65, 0x87, 0xa9, 0xcb, 0xed]
                .as_mut_slice()
                .decode::<wire::Int64>()
                .unwrap(),
            -0x123456789abcdef0i64,
        );
    }

    #[test]
    fn encode_ints() {
        assert_eq!(
            Vec::encode(0xa3u8).unwrap(),
            chunks![0xa3, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            Vec::encode(-0x45i8).unwrap(),
            chunks![0xbb, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );

        assert_eq!(
            Vec::encode(0x1234u16).unwrap(),
            chunks![0x34, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            Vec::encode(-0x1234i16).unwrap(),
            chunks![0xcc, 0xed, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
        );

        assert_eq!(
            Vec::encode(0x12345678u32).unwrap(),
            chunks![0x78, 0x56, 0x34, 0x12, 0x00, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            Vec::encode(-0x12345678i32).unwrap(),
            chunks![0x88, 0xa9, 0xcb, 0xed, 0x00, 0x00, 0x00, 0x00]
        );

        assert_eq!(
            Vec::encode(0x123456789abcdef0u64).unwrap(),
            chunks![0xf0, 0xde, 0xbc, 0x9a, 0x78, 0x56, 0x34, 0x12],
        );
        assert_eq!(
            Vec::encode(-0x123456789abcdef0i64).unwrap(),
            chunks![0x10, 0x21, 0x43, 0x65, 0x87, 0xa9, 0xcb, 0xed],
        );
    }

    #[test]
    fn decode_floats() {
        assert_eq!(
            chunks![0xdb, 0x0f, 0x49, 0x40, 0x00, 0x00, 0x00, 0x00]
                .as_mut_slice()
                .decode::<wire::Float32>()
                .unwrap(),
            ::core::f32::consts::PI,
        );
        assert_eq!(
            chunks![0x18, 0x2d, 0x44, 0x54, 0xfb, 0x21, 0x09, 0x40]
                .as_mut_slice()
                .decode::<wire::Float64>()
                .unwrap(),
            ::core::f64::consts::PI,
        );
    }

    #[test]
    fn encode_floats() {
        assert_eq!(
            Vec::encode(::core::f32::consts::PI).unwrap(),
            chunks![0xdb, 0x0f, 0x49, 0x40, 0x00, 0x00, 0x00, 0x00],
        );
        assert_eq!(
            Vec::encode(::core::f64::consts::PI).unwrap(),
            chunks![0x18, 0x2d, 0x44, 0x54, 0xfb, 0x21, 0x09, 0x40],
        );
    }
}
