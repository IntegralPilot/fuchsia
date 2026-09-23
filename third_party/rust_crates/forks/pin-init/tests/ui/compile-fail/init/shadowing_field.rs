use pin_init::*;

struct Foo {
    x: usize,
    y: usize,
}

fn main() {
    let x = 42;
    let _ = init!(Foo { x, y: x });
}
