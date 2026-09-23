// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
mod tests {

    #[test]
    fn hash_map_example() {
        // [START rcu_hash_map_example]
        use starnix_rcu::{RcuHashMap, RcuReadScope};

        // Create a new RcuHashMap.
        let map = RcuHashMap::default();

        // Single write operation.
        // This internally acquires the write lock for the duration of the insert.
        map.insert("key", "value".to_string());

        // Batched write operations.
        // Explicitly locking allows multiple updates to occur atomically.
        {
            let mut guard = map.lock();
            guard.insert("key2", "value2".to_string());
            guard.remove(&"key");
        } // The write lock is released here.

        // Read operation.
        // An RcuReadScope is required to protect the data from reclamation.
        {
            // Enter an RCU read-side critical section.
            let scope = RcuReadScope::new();

            if let Some(value) = map.get(&scope, &"key2") {
                println!("Found value: {}", value);
            } else {
                println!("Key not found");
            }
        } // The `scope` is dropped, ending the read-side critical section.
        // [END rcu_hash_map_example]
    }
}
