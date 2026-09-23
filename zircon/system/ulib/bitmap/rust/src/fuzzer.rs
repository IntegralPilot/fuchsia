// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ::bitmap::*;
use arbitrary::Arbitrary;
use fuzz::fuzz;

#[derive(Arbitrary, Debug)]
enum BitmapOp {
    Set { index: usize, next: usize },
    ClearAll,
    Scan { off: usize, max: usize, set: bool },
    Find { set: bool, off: usize, max: usize, run_len: usize },
    Get { bit: usize, last_bit: usize },
    Reset { memory: usize },
}

#[fuzz]
fn raw_bitmap_fuzzer(ops: Vec<BitmapOp>) {
    let mut bitmap = RawBitmapGeneric::new(DefaultStorage::new());
    for op in ops {
        match op {
            BitmapOp::Set { index, next } => {
                let _ = bitmap.set(index, next);
            }
            BitmapOp::ClearAll => {
                bitmap.clear_all();
            }
            BitmapOp::Scan { off, max, set } => {
                let _ = bitmap.scan(off, max, set);
            }
            BitmapOp::Find { set, off, max, run_len } => {
                let _ = bitmap.find(set, off, max, run_len);
            }
            BitmapOp::Get { bit, last_bit } => {
                let _ = bitmap.get(bit, last_bit);
            }
            BitmapOp::Reset { memory } => {
                // Cap memory to 10MB to avoid OOM
                let capped_memory = memory % (10 * 1024 * 1024);
                let _ = bitmap.reset(capped_memory);
            }
        }
    }
}
