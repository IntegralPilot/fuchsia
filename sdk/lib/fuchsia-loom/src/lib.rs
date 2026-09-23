// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// These macros apply `#[cfg]`s to each item, avoid repeating the same config
// settings, and cut down on the line noise.

#[cfg(loom)]
pub use ::loom;

#[cfg(loom)]
macro_rules! loom {
    ($($tt:tt)*) => { $($tt)* }
}

#[cfg(not(loom))]
macro_rules! loom {
    ($($tt:tt)*) => {};
}

#[cfg(loom)]
macro_rules! not_loom {
    ($($tt:tt)*) => {};
}

#[cfg(not(loom))]
macro_rules! not_loom {
    ($($tt:tt)*) => { $($tt)* }
}

pub mod cell {
    loom! {
        pub use loom::cell::UnsafeCell;
    }
    not_loom! {
        pub struct UnsafeCell<T: ?Sized>(core::cell::UnsafeCell<T>);

        impl<T: ?Sized> UnsafeCell<T> {
            #[inline]
            pub fn new(value: T) -> Self
            where
                T: Sized,
            {
                Self(core::cell::UnsafeCell::new(value))
            }

            #[inline]
            pub fn with_mut<F, R>(&self, f: F) -> R
            where
                F: FnOnce(*mut T) -> R,
            {
                f(self.0.get())
            }

            #[inline]
            pub fn with<F, R>(&self, f: F) -> R
            where
                F: FnOnce(*const T) -> R,
            {
                f(self.0.get())
            }
        }
    }
}

pub mod future {
    loom! {
        pub use loom::future::AtomicWaker;
    }
    not_loom! {
        pub struct AtomicWaker(futures::task::AtomicWaker);

        impl AtomicWaker {
            #[inline]
            pub fn new() -> Self {
                Self(futures::task::AtomicWaker::new())
            }

            #[inline]
            pub fn register_by_ref(&self, waker: &core::task::Waker) {
                self.0.register(waker);
            }

            #[inline]
            pub fn wake(&self) {
                self.0.wake();
            }
        }
    }
}

pub mod hint {
    loom! {
        pub use loom::hint::unreachable_unchecked;
    }
    not_loom! {
        pub use core::hint::unreachable_unchecked;
    }
}

pub mod sync {
    loom! {
        pub use loom::sync::{Arc, Mutex};
    }
    not_loom! {
        pub use std::sync::{Arc, Mutex};
    }

    pub mod atomic {
        macro_rules! define_atomic {
            ($atomic:ident, $prim:ident) => {
                loom! {
                    pub use loom::sync::atomic::$atomic;
                }
                not_loom! {
                    pub struct $atomic(core::sync::atomic::$atomic);

                    impl $atomic {
                        #[inline]
                        pub fn new(v: $prim) -> Self {
                            Self(core::sync::atomic::$atomic::new(v))
                        }

                        #[inline]
                        pub fn with_mut<R>(&mut self, f: impl FnOnce(&mut $prim) -> R) -> R {
                            f(self.0.get_mut())
                        }

                        #[inline]
                        pub fn load(&self, order: Ordering) -> $prim {
                            self.0.load(order)
                        }

                        #[inline]
                        pub fn store(&self, val: $prim, order: Ordering) {
                            self.0.store(val, order)
                        }

                        #[inline]
                        pub fn fetch_add(&self, val: $prim, order: Ordering) -> $prim {
                            self.0.fetch_add(val, order)
                        }

                        #[inline]
                        pub fn fetch_sub(&self, val: $prim, order: Ordering) -> $prim {
                            self.0.fetch_sub(val, order)
                        }

                        #[inline]
                        pub fn fetch_or(&self, val: $prim, order: Ordering) -> $prim {
                            self.0.fetch_or(val, order)
                        }
                    }
                }
            };
        }

        define_atomic!(AtomicU8, u8);
        define_atomic!(AtomicU16, u16);
        define_atomic!(AtomicU32, u32);
        define_atomic!(AtomicU64, u64);
        define_atomic!(AtomicUsize, usize);
        define_atomic!(AtomicI8, i8);
        define_atomic!(AtomicI16, i16);
        define_atomic!(AtomicI32, i32);
        define_atomic!(AtomicI64, i64);
        define_atomic!(AtomicIsize, isize);

        loom! {
            pub use loom::sync::atomic::AtomicBool;
        }
        not_loom! {
            pub struct AtomicBool(core::sync::atomic::AtomicBool);

            impl AtomicBool {
                #[inline]
                pub fn new(v: bool) -> Self {
                    Self(core::sync::atomic::AtomicBool::new(v))
                }

                #[inline]
                pub fn load(&self, order: Ordering) -> bool {
                    self.0.load(order)
                }

                #[inline]
                pub fn store(&self, val: bool, order: Ordering) {
                    self.0.store(val, order)
                }
            }
        }

        pub use core::sync::atomic::Ordering;
    }

    pub mod mpsc {
        loom! {
            pub use loom::sync::mpsc::{channel, Sender, Receiver};
        }
        not_loom! {
            pub use std::sync::mpsc::{channel, Sender, Receiver};
        }

        pub use std::sync::mpsc::TryRecvError;
    }
}
