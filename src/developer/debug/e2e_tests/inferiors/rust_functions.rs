// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::f32::consts::PI;

static mut SOME_GLOBAL: i32 = 0;

#[inline(never)]
fn leaf_no_args() {
    unsafe {
        SOME_GLOBAL = 5;
    }
}

#[inline(never)]
fn nested_no_args() {
    leaf_no_args();
}

#[inline(never)]
fn print_hello() {
    let num: i32;
    unsafe {
        num = SOME_GLOBAL;
    }
    println!("Hello! SOME_GLOBAL = {}", num);
}

#[inline(never)]
fn return_global_plus_one() -> i32 {
    unsafe { SOME_GLOBAL + 1 }
}

#[inline(never)]
fn return_float() -> f32 {
    PI
}

#[inline(never)]
fn return_i32_box() -> Box<i32> {
    unsafe { Box::new(SOME_GLOBAL) }
}

#[inline(never)]
fn add_two_ints(lhs: i32, rhs: i32) -> i32 {
    lhs + rhs
}

#[inline(never)]
fn add_int_refs(lhs: &i32, rhs: &i32) -> i32 {
    lhs + rhs
}

#[inline(never)]
fn swap_i32_refs<'a>(lhs: &'a mut i32, rhs: &'a mut i32) {
    let tmp = *lhs;
    *lhs = *rhs;
    *rhs = tmp;
}

struct SomeStruct {
    one: i32,
    two: i32,
    nums: Vec<i32>,
}

#[inline(never)]
fn do_some_stuff(s: &mut SomeStruct) {
    s.one += 1;
    s.two += 2;
    s.nums.pop();
}

fn main() {
    nested_no_args();
    print_hello();
    println!("{}", return_global_plus_one());
    println!("{}", return_i32_box());
    println!("{}", return_float());
    println!("{}", add_two_ints(1, 3));

    let lhs = 7i32;
    let rhs = 8i32;
    println!("{}", add_int_refs(&lhs, &rhs));

    let mut lhsmut = lhs;
    let mut rhsmut = rhs;
    swap_i32_refs(&mut lhsmut, &mut rhsmut);
    println!("{} {}", lhsmut, rhsmut);

    let mut s = SomeStruct { one: 1, two: 2, nums: vec![3, 4, 5, 6] };
    do_some_stuff(&mut s);
}
