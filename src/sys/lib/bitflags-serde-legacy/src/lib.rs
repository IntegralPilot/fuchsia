// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

/// Helper macro that generates `serde` implementations for a `bitflags` 2.x types that are
/// compatible with the `bitflags` 1.x serialization format.
#[macro_export]
macro_rules! impl_traits {
    ($ty:ident) => {
        mod __private_bitflags_serde_legacy {
            use ::bitflags::Flags;
            use ::serde::{Deserialize, Serialize, de};
            use ::std::marker::PhantomData;

            #[derive(Serialize, Deserialize)]
            struct Helper {
                bits: <super::$ty as Flags>::Bits,
            }

            impl ::serde::Serialize for super::$ty {
                fn serialize<S: ::serde::Serializer>(
                    &self,
                    serializer: S,
                ) -> Result<S::Ok, S::Error> {
                    let helper = Helper { bits: self.bits() };
                    ::serde::Serialize::serialize(&helper, serializer)
                }
            }

            impl<'de> ::serde::Deserialize<'de> for super::$ty {
                fn deserialize<D: ::serde::Deserializer<'de>>(
                    deserializer: D,
                ) -> Result<Self, D::Error> {
                    if deserializer.is_human_readable() {
                        deserializer.deserialize_any(HumanReadableVisitor(PhantomData))
                    } else {
                        deserializer.deserialize_any(BinaryVisitor(PhantomData))
                    }
                }
            }

            struct HumanReadableVisitor(PhantomData<<super::$ty as Flags>::Bits>);

            impl<'de> de::Visitor<'de> for HumanReadableVisitor {
                type Value = super::$ty;

                fn expecting(
                    &self,
                    formatter: &mut ::std::fmt::Formatter<'_>,
                ) -> ::std::fmt::Result {
                    formatter.write_str("string or map")
                }

                fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                where
                    E: de::Error,
                {
                    ::bitflags::parser::from_str(value).map_err(|e| E::custom(e))
                }

                fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
                where
                    M: de::MapAccess<'de>,
                {
                    let helper = Helper::deserialize(de::value::MapAccessDeserializer::new(map))?;
                    Ok(Flags::from_bits_retain(helper.bits))
                }
            }

            struct BinaryVisitor(PhantomData<<super::$ty as Flags>::Bits>);

            macro_rules! delegate_binary_visitor {
                ($method:ident, $value_ty:ty, $deserializer:ident) => {
                    fn $method<E>(self, value: $value_ty) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        let bits: <super::$ty as Flags>::Bits =
                            Deserialize::deserialize(de::value::$deserializer::new(value))?;
                        Ok(Flags::from_bits_retain(bits))
                    }
                };
            }

            impl<'de> de::Visitor<'de> for BinaryVisitor {
                type Value = super::$ty;

                fn expecting(
                    &self,
                    formatter: &mut ::std::fmt::Formatter<'_>,
                ) -> ::std::fmt::Result {
                    formatter.write_str("string or map")
                }

                delegate_binary_visitor!(visit_i8, i8, I8Deserializer);
                delegate_binary_visitor!(visit_i16, i16, I16Deserializer);
                delegate_binary_visitor!(visit_i32, i32, I32Deserializer);
                delegate_binary_visitor!(visit_i64, i64, I64Deserializer);
                delegate_binary_visitor!(visit_i128, i128, I128Deserializer);
                delegate_binary_visitor!(visit_u8, u8, U8Deserializer);
                delegate_binary_visitor!(visit_u16, u16, U16Deserializer);
                delegate_binary_visitor!(visit_u32, u32, U32Deserializer);
                delegate_binary_visitor!(visit_u64, u64, U64Deserializer);
                delegate_binary_visitor!(visit_u128, u128, U128Deserializer);
                delegate_binary_visitor!(visit_char, char, CharDeserializer);
                delegate_binary_visitor!(visit_str, &str, StrDeserializer);
                delegate_binary_visitor!(visit_borrowed_str, &'de str, BorrowedStrDeserializer);
                delegate_binary_visitor!(visit_string, String, StringDeserializer);
                delegate_binary_visitor!(visit_bytes, &[u8], BytesDeserializer);
                delegate_binary_visitor!(
                    visit_borrowed_bytes,
                    &'de [u8],
                    BorrowedBytesDeserializer
                );

                fn visit_map<M>(self, map: M) -> Result<Self::Value, M::Error>
                where
                    M: de::MapAccess<'de>,
                {
                    let helper = Helper::deserialize(de::value::MapAccessDeserializer::new(map))?;
                    Ok(Flags::from_bits_retain(helper.bits))
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    bitflags::bitflags! {
        #[derive(Debug, PartialEq)]
        pub struct LegacyFlags: u32 {
            const A = 0b00000001;
            const B = 0b00000010;
        }
    }

    impl_traits!(LegacyFlags);

    bitflags::bitflags! {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        pub struct NewFlags: u32 {
            const A = 0b00000001;
            const B = 0b00000010;
        }
    }

    #[test]
    fn test_roundtrip() {
        let flags = LegacyFlags::A | LegacyFlags::B;

        let flags_json = serde_json::to_value(&flags).unwrap();
        assert_eq!(flags_json, serde_json::json!({ "bits": 3 }));

        let flags: LegacyFlags = serde_json::from_value(flags_json).unwrap();
        assert_eq!(flags, LegacyFlags::A | LegacyFlags::B);
    }

    #[test]
    fn test_parses_new_human_readable_format() {
        let new_flags = NewFlags::A | NewFlags::B;
        let new_flags_json = serde_json::to_value(&new_flags).unwrap();
        assert_eq!(new_flags_json, serde_json::json!("A | B"));

        let legacy_flags: LegacyFlags = serde_json::from_value(new_flags_json).unwrap();
        assert_eq!(legacy_flags, LegacyFlags::A | LegacyFlags::B);
    }

    #[test]
    fn test_parses_new_binary_format() {
        let new_flags = NewFlags::A | NewFlags::B;
        let mut new_flags_cbor: Vec<u8> = Vec::new();
        ciborium::into_writer(&new_flags, &mut new_flags_cbor).unwrap();
        let legacy_flags: LegacyFlags = ciborium::from_reader(new_flags_cbor.as_slice()).unwrap();
        assert_eq!(legacy_flags, LegacyFlags::A | LegacyFlags::B);
    }
}
