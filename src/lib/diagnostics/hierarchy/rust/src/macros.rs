// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Macro utilities for building a `DiagnosticsHierarchy`.

use crate::{ArrayContent, Property};

/// This macro simplifies creating diagnostics hierarchies, to remove the need of writing multiple
/// nested hierarchies and manually writing all properties.
///
/// Example:
///
/// ```
/// let hierarchy = hierarchy!{
///     root: {
///         some_int_property: 2,
///         some_nested_child: {
///             some_string_property: "foo",
///         }
///     }
/// };
/// ```
///
/// Using names like this will create a `DiagnosticsHierarchy` where `Key` is of type `String`.
///
/// It's possible to use expressions that resolve to some other type. In the following example, we
/// use `Field` variants as the key. The function `bar()` returns a `Field::Bar`.
///
/// ```
/// enum Field {
///     Foo,
///     Bar,
/// }
///
/// let hierarchy = hierarchy!{
///     root: {
///         Field::Foo => "bar",
///         bar() => 2,
///     }
/// };
/// ```
///
/// The only requirement (as in any `DiagnosticsHierarchy`'s `Key`) it must implement `AsRef<str>`.
///
/// It's also possible to use existing variables as keys. For example:
///
/// ```
/// let my_var = "my_key".to_string();
/// let hierarchy = hierarchy!{
///     root: {
///         var my_var: "foo",
///     }
/// };
/// ```
#[macro_export]
macro_rules! hierarchy {
    (@build $hierarchy:expr,) => {};

    // Handle adding a nodes to the hierarchy.
    (@build $hierarchy:expr, var $key:ident: { $($sub:tt)* }) => {{
        #[allow(unused_mut)]
        let mut child = $crate::DiagnosticsHierarchy::new($key, vec![], vec![]);
        $crate::hierarchy!(@build child, $($sub)*);
        $hierarchy.add_child(child);
    }};
    (@build $hierarchy:expr, var $key:ident: { $($sub:tt)* }, $($rest:tt)*) => {{
        $crate::hierarchy!(@build $hierarchy, var $key: { $($sub)* });
        $crate::hierarchy!(@build $hierarchy, $($rest)*);
    }};

    // Handle adding properties to the hierarchy.
    (@build $hierarchy:expr, var $key:ident: $value:expr) => {{
        use $crate::macros::IntoPropertyWithKey;
        let property = $value.into_property_with_key($key);
        $hierarchy.add_property(property);
    }};
    (@build $hierarchy:expr, var $key:ident: $value:expr, $($rest:tt)*) => {{
        $crate::hierarchy!(@build $hierarchy, var $key: $value);
        $crate::hierarchy!(@build $hierarchy, $($rest)*);
    }};

    // Allow a literal as key. It'll be treated as a String key.
    (@build $hierarchy:expr, $key:ident: $($rest:tt)+) => {{
        let key = stringify!($key).to_string();
        $crate::hierarchy!(@build $hierarchy, var key: $($rest)+);
    }};
    // Allow a string literal as key.
    (@build $hierarchy:expr, $key:tt: $($rest:tt)+) => {{
        let key: &'static str = $key;
        let key = $key.to_string();
        $crate::hierarchy!(@build $hierarchy, var key: $($rest)+);
    }};
    // Allow a expression as key, only for properties.
    (@build $hierarchy:expr, $key:expr => $value:expr) => {{
        let key = $key;
        $crate::hierarchy!(@build $hierarchy, var key: $value);
    }};
    (@build $hierarchy:expr, $key:expr => $value:expr, $($rest:tt)*) => {{
        let key = $key;
        $crate::hierarchy!(@build $hierarchy, var key: $value, $($rest)*);
    }};

    // Entry points
    (var $key:ident: { $($rest:tt)* }) => {{
        #[allow(unused_mut)]
        let mut hierarchy = $crate::DiagnosticsHierarchy::new($key, vec![], vec![]);
        $crate::hierarchy!(@build hierarchy, $($rest)*);
        hierarchy
    }};
    ($key:ident: $($rest:tt)+) => {{
        let key = stringify!($key).to_string();
        $crate::hierarchy!(var key: $($rest)+)
    }};
    ($key:tt: $($rest:tt)+) => {{
        let key : &'static str = $key;
        let key = key.to_string();
        $crate::hierarchy!(var key: $($rest)+)
    }};
}

/// Trait implemented by all types that can be converted to a `Property`.
pub trait IntoPropertyWithKey<Key = String> {
    fn into_property_with_key(self, key: Key) -> Property<Key>;
}

macro_rules! impl_into_property_with_key {
    // Implementation for all `$type`s.
    ($property_name:ident, [$($type:ty),*]) => {
        $(
        impl<Key> IntoPropertyWithKey<Key> for $type {
            fn into_property_with_key(self, key: Key) -> Property<Key> {
                Property::$property_name(key, self.into())
            }
        }
        )*
    };

    // Implementation for all `Vec<$type>`s calling Into for each of them.
    ($property_name:ident, map:[$($type:ty),*]) => {
        $(
        impl<Key> IntoPropertyWithKey<Key> for $type {
            fn into_property_with_key(self, key: Key) -> Property<Key> {
                let value = self.into_iter().map(|value| value.into()).collect::<Vec<_>>();
                Property::$property_name(key, ArrayContent::Values(value))
            }
        }
        )*
    };
}

// TODO(https://fxbug.dev/42155846): support missing data types -> bytes, histograms
impl_into_property_with_key!(String, [String, &str]);
impl_into_property_with_key!(Int, [i64, i32, i16, i8]);
impl_into_property_with_key!(Uint, [u64, u32, u16, u8]);
impl_into_property_with_key!(Double, [f64, f32]);
impl_into_property_with_key!(Bool, [bool]);
impl_into_property_with_key!(DoubleArray, map:[Vec<f64>, Vec<f32>]);
impl_into_property_with_key!(IntArray, map:[Vec<i64>, Vec<i32>, Vec<i16>, Vec<i8>]);
impl_into_property_with_key!(UintArray, map:[Vec<u64>, Vec<u32>, Vec<u16>]);
impl_into_property_with_key!(StringList, [Vec<String>]);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ArrayFormat, DiagnosticsHierarchy};

    #[fuchsia::test]
    fn test_empty_hierarchy() {
        let expected = DiagnosticsHierarchy::new_root();
        let h: DiagnosticsHierarchy = hierarchy! { root: {} };
        assert_eq!(expected, h);
    }

    #[fuchsia::test]
    fn test_all_types() {
        let string_list = vec!["foo".to_string(), "bar".to_string()];

        let mut expected = DiagnosticsHierarchy::new_root();
        expected.add_property(Property::String("string".to_string(), "some string".to_string()));
        expected.add_property(Property::String("strref".to_string(), "some str ref".to_string()));
        expected.add_property(Property::Int("int8".to_string(), 1i64));
        expected.add_property(Property::Int("int16".to_string(), 2i64));
        expected.add_property(Property::Int("int32".to_string(), 3i64));
        expected.add_property(Property::Int("int64".to_string(), 4i64));
        expected.add_property(Property::Uint("uint16".to_string(), 5u64));
        expected.add_property(Property::Uint("uint32".to_string(), 6u64));
        expected.add_property(Property::Uint("uint64".to_string(), 7u64));
        expected.add_property(Property::Double("float32".to_string(), 8.25f64));
        expected.add_property(Property::Double("float64".to_string(), 9f64));
        expected.add_property(Property::Bool("boolean".to_string(), true));
        expected.add_property(Property::DoubleArray(
            "array_float32".to_string(),
            ArrayContent::new(vec![1f64, 2.5], ArrayFormat::Default).unwrap(),
        ));
        expected.add_property(Property::DoubleArray(
            "array_float64".to_string(),
            ArrayContent::new(vec![3f64, 4.25], ArrayFormat::Default).unwrap(),
        ));
        expected.add_property(Property::IntArray(
            "array_int8".to_string(),
            ArrayContent::new(vec![1i64, 2, 3, 4], ArrayFormat::Default).unwrap(),
        ));
        expected.add_property(Property::IntArray(
            "array_int16".to_string(),
            ArrayContent::new(vec![5i64, 6, 7, 8], ArrayFormat::Default).unwrap(),
        ));
        expected.add_property(Property::IntArray(
            "array_int32".to_string(),
            ArrayContent::new(vec![2i64, 4], ArrayFormat::Default).unwrap(),
        ));
        expected.add_property(Property::IntArray(
            "array_int64".to_string(),
            ArrayContent::new(vec![6i64, 8], ArrayFormat::Default).unwrap(),
        ));
        expected.add_property(Property::UintArray(
            "array_uint16".to_string(),
            ArrayContent::new(vec![0u64, 9], ArrayFormat::Default).unwrap(),
        ));
        expected.add_property(Property::UintArray(
            "array_uint32".to_string(),
            ArrayContent::new(vec![1u64, 3, 5], ArrayFormat::Default).unwrap(),
        ));
        expected.add_property(Property::UintArray(
            "array_uint64".to_string(),
            ArrayContent::new(vec![7u64, 9], ArrayFormat::Default).unwrap(),
        ));
        expected.add_property(Property::StringList("string_list".to_string(), string_list.clone()));

        let result = hierarchy! {
            root: {
                string: "some string".to_string(),
                strref: "some str ref",
                int8:  1i8,
                int16: 2i16,
                int32: 3i32,
                int64: 4i64,
                uint16: 5u16,
                uint32: 6u32,
                uint64: 7u64,
                float32: 8.25f32,
                float64: 9f64,
                boolean: true,
                array_float32: vec![1f32, 2.5],
                array_float64: vec![3f64, 4.25],
                array_int8: vec![1i8,2,3,4],
                array_int16: vec![5i16,6,7,8],
                array_int32: vec![2i32,4],
                array_int64: vec![6i64,8],
                array_uint16: vec![0u16,9],
                array_uint32: vec![1u32,3, 5],
                array_uint64: vec![7u64, 9],
                string_list: string_list.clone(),
            }
        };

        assert_eq!(expected, result);
    }

    #[fuchsia::test]
    fn test_nested_hierarchy() {
        let mut expected = DiagnosticsHierarchy::new_root();
        expected.add_property_at_path(
            &["root", "sub1", "sub11", "sub111"],
            Property::Int("value".to_string(), 1i64),
        );
        expected.add_property_at_path(
            &["root", "sub1", "sub11", "sub111"],
            Property::String("other_value".to_string(), "foo".to_string()),
        );
        expected.add_property_at_path(
            &["root", "sub2", "sub21"],
            Property::Uint("value".to_string(), 2u64),
        );
        let _ = expected.get_or_add_node(&["root", "sub2", "sub22"]);
        let result = hierarchy! {
            root: {
                sub1: {
                    sub11: {
                        sub111: {
                            value: 1i64,
                            other_value: "foo",
                        }
                    }
                },
                sub2: {
                    sub21: {
                        value: 2u64,
                    },
                    sub22: {},
                }
            }
        };
        assert_eq!(expected, result);
    }

    #[fuchsia::test]
    fn test_var_key_syntax() {
        let mut expected = DiagnosticsHierarchy::new("foo", vec![], vec![]);
        expected
            .add_property_at_path(&["foo"], Property::String("bar".to_string(), "baz".to_string()));
        let some_key = "foo".to_string();
        let another_key = "bar".to_string();
        let result = hierarchy! {
            var some_key: {
                var another_key: "baz",
            }
        };
        assert_eq!(expected, result);
    }

    #[derive(Debug, PartialEq)]
    enum Field {
        Foo,
        Bar,
    }

    impl AsRef<str> for Field {
        fn as_ref(&self) -> &str {
            match self {
                Field::Foo => "foo",
                Field::Bar => "bar",
            }
        }
    }

    #[fuchsia::test]
    fn test_custom_key_type_for_properties() {
        let result = hierarchy! {
            root: {
                Field::Bar => 2u64,
                baz: {
                    Field::Foo => "baz",
                }
            }
        };
        assert_eq!(
            result,
            DiagnosticsHierarchy::new(
                "root",
                vec![Property::Uint(Field::Bar, 2u64)],
                vec![DiagnosticsHierarchy::new(
                    "baz",
                    vec![Property::String(Field::Foo, "baz".to_string())],
                    vec![]
                )],
            )
        );
    }

    #[fuchsia::test]
    fn test_string_literal_root_key() {
        let result = hierarchy! {
            "root": {
                foo: 1u64,
            }
        };
        assert_eq!(
            result,
            DiagnosticsHierarchy::new(
                "root",
                vec![Property::Uint("foo".to_string(), 1u64)],
                vec![]
            )
        );
    }
}
