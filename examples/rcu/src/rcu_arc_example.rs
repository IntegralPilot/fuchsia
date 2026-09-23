// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(test)]
mod tests {

    #[test]
    fn rcu_arc_example() {
        // [START rcu_arc_example]
        use fuchsia_rcu::RcuDroppableArc;
        use std::sync::Arc;

        // Initialize an RcuDroppableArc with an initial value.
        let rcu_arc = RcuDroppableArc::new(Arc::new(42));

        // Access the current value.
        // The returned guard dereferences to the inner type T.
        {
            let val = rcu_arc.read();
            println!("Current value: {}", *val);
        }

        // Atomically replace the inner Arc.
        // Concurrent readers may still see the old value during this update.
        rcu_arc.update(Arc::new(100));

        // Subsequent readers will observe the new value.
        {
            let val = rcu_arc.read();
            println!("New value: {}", *val);
        }
        // [END rcu_arc_example]
    }
}
