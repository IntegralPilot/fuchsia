<!-- README.md is generated from the README.tpl file with
      `cargo install cargo-readme && cargo readme > README.md`
 -->
[![Crates.io](https://img.shields.io/crates/v/pin-weak)](https://crates.io/crates/pin-weak)
[![Documentation](https://docs.rs/pin-weak/badge.svg)](https://docs.rs/pin-weak/)

# pin-weak

This create provides weak pointers for `Pin<std::rc::Rc<T>>` and  `Pin<std::rc::Arc<T>>`

### Motivation

`Pin<std::rc::Rc<T>>` and `Pin<std::rc::Arc<T>>` cannot be converted safely to
their `Weak<T>` equivalent if `T` does not implement `Unpin`.
That's because it would otherwise be possible to do something like this:

```rust
struct SomeStruct(PhantomPinned);
let pinned = Rc::pin(SomeStruct(PhantomPinned));

// This is unsafe ...
let weak = unsafe {
    Rc::downgrade(&Pin::into_inner_unchecked(pinned.clone()))
};

// ... because otherwise it would be possible to move the content of pinned:
let mut unpinned_rc = weak.upgrade().unwrap();
std::mem::drop((pinned, weak));
// unpinned_rc is now the only reference so this will work:
let x = std::mem::replace(
    Rc::get_mut(&mut unpinned_rc).unwrap(),
    SomeStruct(PhantomPinned),
);
```

In that example, `x` is the original `SomeStruct` which we moved in memory,
**that is undefined behavior**, do not do that at home.

### `PinWeak`

This crate simply provide a `rc::PinWeak` and `sync::PinWeak` which allow to
get weak pointer from `Pin<std::rc::Rc>` and `Pin<srd::sync::Arc>`.

This is safe because you can one can only get back a `Pin` out of it when
trying to upgrade the weak pointer.

`PinWeak` can be created using the `PinWeak` downgrade function.

### Example

```rust
use pin_weak::rc::*;
struct SomeStruct(PhantomPinned, usize);
let pinned = Rc::pin(SomeStruct(PhantomPinned, 42));
let weak = PinWeak::downgrade(pinned.clone());
assert_eq!(weak.upgrade().unwrap().1, 42);
std::mem::drop(pinned);
assert!(weak.upgrade().is_none());
```


## License

MIT
