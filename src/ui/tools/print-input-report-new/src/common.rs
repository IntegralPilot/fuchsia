// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub const SERVICE_DIR: &str = "/svc/fuchsia.input.report.Service";

/// Maps an iterator of items to a `Vec<U>` using a mapping function `f`.
pub fn map_vec_by<T, U>(items: impl IntoIterator<Item = T>, f: impl FnMut(T) -> U) -> Vec<U> {
    items.into_iter().map(f).collect()
}

/// Converts an iterator of items into a `Vec<U>` using `Into`.
pub fn convert_vec<T: Into<U>, U>(items: impl IntoIterator<Item = T>) -> Vec<U> {
    map_vec_by(items, Into::into)
}

/// Converts an iterator of items into a `Vec<String>` using `Debug` formatting.
pub fn debug_vec<T: std::fmt::Debug>(items: impl IntoIterator<Item = T>) -> Vec<String> {
    map_vec_by(items, |x| format!("{x:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_vec_by() {
        let input = vec![1, 2, 3];
        let result = map_vec_by(input, |x| x * 2);
        assert_eq!(result, vec![2, 4, 6]);
    }

    #[test]
    fn test_convert_vec() {
        #[derive(Debug, PartialEq, Eq)]
        struct ComplexNumber {
            real: i32,
            imag: i32,
        }

        impl From<i32> for ComplexNumber {
            fn from(val: i32) -> Self {
                Self { real: val, imag: 0 }
            }
        }

        let input = vec![1, 2, 3];
        let result: Vec<ComplexNumber> = convert_vec(input);
        assert_eq!(
            result,
            vec![
                ComplexNumber { real: 1, imag: 0 },
                ComplexNumber { real: 2, imag: 0 },
                ComplexNumber { real: 3, imag: 0 },
            ]
        );
    }

    #[test]
    fn test_debug_vec() {
        #[derive(Debug)]
        enum Sample {
            Foo,
            Bar,
        }
        let input = vec![Sample::Foo, Sample::Bar];
        assert_eq!(debug_vec(input), vec!["Foo".to_string(), "Bar".to_string()]);
    }
}
