// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use serde_yaml::Value;

/// Merge `b` into `a`, appending arrays and overwriting everything else.
pub fn merge(a: &mut Value, b: Value) {
    match (a, b) {
        (Value::Mapping(a), Value::Mapping(b)) => {
            for (k, v) in b {
                if !a.contains_key(&k) {
                    a.insert(k, v);
                } else {
                    merge(&mut a[&k], v);
                }
            }
        }
        (Value::Sequence(a), Value::Sequence(ref mut b)) => {
            a.append(b);
        }
        (a, b) => *a = b,
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_merge_mapping() {
        let a = "
            test_params:
                name: a
                who_called:
                    was_a: true
        ";
        let mut a: Value = serde_yaml::from_str(a).unwrap();
        let b = "
            test_params:
                name: b
                who_called:
                    was_b: true
        ";
        let b: Value = serde_yaml::from_str(b).unwrap();
        merge(&mut a, b);
        let want = "
            test_params:
                name: b
                who_called:
                    was_a: true
                    was_b: true
        ";
        let want: Value = serde_yaml::from_str(want).unwrap();
        assert_eq!(a, want);
    }

    #[test]
    fn test_merge_append_arrays() {
        let mut a: Value = serde_yaml::from_str(" - a").unwrap();
        let b: Value = serde_yaml::from_str(" - b").unwrap();
        merge(&mut a, b);
        let want = "
            - a
            - b
        ";
        let want: Value = serde_yaml::from_str(want).unwrap();
        assert_eq!(a, want);
    }

    #[test]
    fn test_merge_append_arrays_allow_duplicates() {
        let mut a: Value = serde_yaml::from_str(" - a").unwrap();
        let b: Value = serde_yaml::from_str(" - a").unwrap();
        merge(&mut a, b);
        let want = "
            - a
            - a
        ";
        let want: Value = serde_yaml::from_str(want).unwrap();
        assert_eq!(a, want);
    }

    #[test]
    fn test_merge_overwrite_from_null() {
        let mut a: Value = Value::Null;
        let b: Value = serde_yaml::from_str("true").unwrap();
        merge(&mut a, b.clone());
        assert_eq!(a, b);
    }

    #[test]
    fn test_merge_overwrite_with_null() {
        let mut a: Value = serde_yaml::from_str("true").unwrap();
        let b: Value = Value::Null;
        merge(&mut a, b.clone());
        assert_eq!(a, b);
    }
}
