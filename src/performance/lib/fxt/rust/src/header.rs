// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[macro_export]
macro_rules! trace_header {
    ($name:ident (max_size_bit: $upper_size_bit:literal) ($size_ty:ty) $(($header_ty:expr))? {
        $($field_ty:ty, $getter:ident: $start_bit:literal, $end_bit:literal;)*
    }) => {
        trace_header!(
            $name (max_size_bit: $upper_size_bit) ($size_ty) $(($header_ty))?
            {
                $($field_ty, $getter: $start_bit, $end_bit;)*
            } => |_h| Ok(())
        );
    };
    ($name:ident $(($header_ty:expr))? {
        $($field_ty:ty, $getter:ident: $start_bit:literal, $end_bit:literal;)*
    }) => {
        trace_header!(
            $name $(($header_ty))?
            {
                $($field_ty, $getter: $start_bit, $end_bit;)*
            } => |_h| Ok(())
        );
    };
    (
        $name:ident $(($header_ty:expr))? {
            $($field_ty:ty, $getter:ident: $start_bit:literal, $end_bit:literal;)*
        } => |$header:ident $(: $header_arg_ty:ty)?| $verify:expr
    ) => {
        trace_header!(
            $name (max_size_bit: 15) (u16) $(($header_ty))?
            {
                $($field_ty, $getter: $start_bit, $end_bit;)*
            } => |$header $(: $header_arg_ty)?| $verify
        );
    };
    (
        $name:ident (max_size_bit: $upper_size_bit:literal) ($size_ty:ty) $(($header_ty:expr))? {
            $($field_ty:ty, $getter:ident: $start_bit:literal, $end_bit:literal;)*
        } => |$header:ident $(: $header_arg_ty:ty)?| $verify:expr
    ) => {
        // We invoke the bitfield macros ourselves here so we can use derives on the type.
        #[derive(Clone, Copy, Eq, PartialEq)]
        pub struct $name(u64);

        bitfield::bitfield_bitrange! { struct $name(u64) }

        // NB: bitfield macros flip the start and end bits compared to ours.
        impl std::fmt::Debug for $name {
            bitfield::bitfield_debug! {
                struct $name;
                u8, raw_type, _: 3, 0;
                $size_ty, size_words, _: $upper_size_bit, 4;
                $($field_ty, $getter, _: $end_bit, $start_bit;)*
            }
        }

        impl $name {
            paste::paste! { bitfield::bitfield_fields! {
                u64;
                pub u8, raw_type, set_raw_type: 3, 0;
                pub $size_ty, size_words, set_size_words: $upper_size_bit, 4;
                $(pub $field_ty, $getter, [<set_ $getter>]: $end_bit, $start_bit;)*
            }}

            #[allow(unused_mut)]
            pub fn empty() -> Self {
                let mut header = Self(0);
                $(header.set_raw_type($header_ty);)?
                header
            }

            fn new(bits: u64) -> Result<Self, crate::ParseError> {
                let header = Self(bits);

                $(if header.raw_type() != $header_ty {
                    return Err(crate::ParseError::WrongType {
                        context: stringify!($name),
                        expected: $header_ty,
                        observed: header.raw_type(),
                    });
                })?

                // Run invoker-defined verification and return if it's an error.
                let res: Result<(), crate::ParseError> = (|$header $(: $header_arg_ty)?| $verify)(&header);
                res?;

                Ok(header)
            }

            #[allow(unused)] // Some headers are converted from others and don't need to be parsed.
            pub(crate) fn parse(buf: &[u8]) -> crate::ParseResult<'_, Self> {
                use nom::Parser;
                nom::combinator::map_res(nom::number::streaming::le_u64, |h| Self::new(h)).parse(buf)
            }

            #[allow(unused)] // Not all headers come with payloads, some we use to probe for types.
            pub(crate) fn take_payload<'a>(&self, buf: &'a [u8]) -> crate::ParseResult<'a, &'a [u8]> {
                if self.size_words() == 0 {
                    return Err(nom::Err::Failure(crate::ParseError::InvalidSize));
                }
                let size_bytes_without_header = (self.size_words() as usize - 1) * 8;
                if size_bytes_without_header > buf.len() {
                    let needed = size_bytes_without_header - buf.len();
                    return Err(nom::Err::Incomplete(nom::Needed::Size(
                        std::num::NonZero::new(needed).unwrap())
                    ));
                }
                let (payload, rem) = buf.split_at(size_bytes_without_header);
                Ok((rem, payload))
            }
        }

        impl crate::header::TraceHeader for $name {
            fn set_size_words(&mut self, n: u16) {
                self.set_size_words(n.try_into().unwrap());
            }
            fn to_le_bytes(&self) -> [u8; 8] {
                self.0.to_le_bytes()
            }
        }
    };
}

pub trait TraceHeader {
    fn set_size_words(&mut self, n: u16);
    fn to_le_bytes(&self) -> [u8; 8];
}
