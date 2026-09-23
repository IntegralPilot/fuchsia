// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::collections::{FlatMap, FlatSet};

#[test]
fn test_flat_map_behavior() {
    let mut map = FlatMap::new();
    assert!(map.is_empty());

    map.insert("key1".to_string(), "val1".to_string());
    assert!(!map.is_empty());
    assert!(map.contains_key("key1"));
    assert_eq!(map.get("key1"), Some(&"val1".to_string()));

    // Insert duplicate
    map.insert("key1".to_string(), "val1_new".to_string());
    assert_eq!(map.get("key1"), Some(&"val1_new".to_string()));

    // Insert another
    map.insert("key2".to_string(), "val2".to_string());
    assert_eq!(map.get("key2"), Some(&"val2".to_string()));

    // Mutate
    if let Some(val) = map.get_mut("key2") {
        *val = "val2_mut".to_string();
    }
    assert_eq!(map.get("key2"), Some(&"val2_mut".to_string()));

    // Remove
    assert_eq!(map.remove("key1"), Some("val1_new".to_string()));
    assert!(!map.contains_key("key1"));

    assert_eq!(map.remove("nonexistent"), None);

    map.clear();
    assert!(map.is_empty());
}

#[test]
fn test_flat_set_behavior() {
    let mut set = FlatSet::new();
    assert_eq!(set.len(), 0);

    set.insert("val1".to_string());
    assert_eq!(set.len(), 1);
    assert!(set.contains("val1"));

    // Insert duplicate
    set.insert("val1".to_string());
    assert_eq!(set.len(), 1);

    set.insert("val2".to_string());
    assert_eq!(set.len(), 2);

    // Remove
    assert!(set.remove("val1"));
    assert_eq!(set.len(), 1);
    assert!(!set.contains("val1"));

    assert!(!set.remove("nonexistent"));
}
