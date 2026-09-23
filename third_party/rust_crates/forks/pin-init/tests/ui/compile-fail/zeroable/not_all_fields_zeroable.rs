extern crate pin_init;
use pin_init::*;

#[derive(Zeroable)]
struct Foo {
    a: usize,
    b: &'static Foo,
}

fn main() {}
