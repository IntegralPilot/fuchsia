// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::sync::atomic::Ordering;

use fuchsia_loom::loom;
use fuchsia_loom::sync::Arc;
use fuchsia_loom::sync::atomic::AtomicUsize;

#[test]
fn basic_functionality() {
    let mut builder = loom::model::Builder::new();
    builder.preemption_bound = Some(3);
    builder.check(|| {
        let a = Arc::new(AtomicUsize::new(0));
        let b = a.clone();
        let c = a.clone();

        let a_thread = loom::thread::spawn(move || a.fetch_add(1, Ordering::Relaxed));
        let b_thread = loom::thread::spawn(move || b.fetch_sub(1, Ordering::Relaxed));

        a_thread.join().unwrap();
        b_thread.join().unwrap();

        assert_eq!(c.load(Ordering::Relaxed), 0);
    });
}
