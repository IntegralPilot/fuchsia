// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub use bitflags as __bitflags;
pub use paste;

#[macro_export]
macro_rules! atomic_bitflags {
    (
        $(#[$outer:meta])*
        $vis:vis struct $BitFlags:ident: $T:ty {
            $($t:tt)*
        }
    ) => {
        $crate::paste::paste! {
            $crate::__bitflags::bitflags! {
                $(#[$outer])*
                $vis struct $BitFlags: $T {
                    $($t)*
                }
            }

            #[allow(dead_code)]
            #[derive(Debug, Default)]
            $vis struct [<Atomic $BitFlags>] {
                inner: std::sync::atomic::[<Atomic $T:camel>],
            }

            #[allow(dead_code)]
            impl [<Atomic $BitFlags>] {
                pub fn new(initial: $BitFlags) -> Self {
                    Self {
                        inner: std::sync::atomic::[<Atomic $T:camel>]::new(initial.bits()),
                    }
                }

                pub fn load(&self, order: std::sync::atomic::Ordering) -> $BitFlags {
                    $BitFlags::from_bits_truncate(self.inner.load(order))
                }

                pub fn store(&self, val: $BitFlags, order: std::sync::atomic::Ordering) {
                    self.inner.store(val.bits(), order);
                }

                pub fn fetch_or(&self, val: $BitFlags, order: std::sync::atomic::Ordering) -> $BitFlags {
                    $BitFlags::from_bits_truncate(self.inner.fetch_or(val.bits(), order))
                }

                pub fn fetch_and(&self, val: $BitFlags, order: std::sync::atomic::Ordering) -> $BitFlags {
                    $BitFlags::from_bits_truncate(self.inner.fetch_and(val.bits(), order))
                }

                pub fn swap(&self, val: $BitFlags, order: std::sync::atomic::Ordering) -> $BitFlags {
                    $BitFlags::from_bits_truncate(self.inner.swap(val.bits(), order))
                }

                pub fn compare_exchange(
                    &self,
                    current: $BitFlags,
                    new: $BitFlags,
                    success: std::sync::atomic::Ordering,
                    failure: std::sync::atomic::Ordering,
                ) -> Result<$BitFlags, $BitFlags> {
                    self.inner.compare_exchange(current.bits(), new.bits(), success, failure)
                        .map($BitFlags::from_bits_truncate)
                        .map_err($BitFlags::from_bits_truncate)
                }

                pub fn update(
                    &self,
                    value: $BitFlags,
                    mask: $BitFlags,
                    set_order: std::sync::atomic::Ordering,
                    fetch_order: std::sync::atomic::Ordering,
                ) -> $BitFlags {
                    self.inner.try_update(set_order, fetch_order, |old| {
                        Some((old & !mask.bits()) | (value.bits() & mask.bits()))
                    }).map($BitFlags::from_bits_truncate).unwrap()
                }
            }

            impl From<$BitFlags> for [<Atomic $BitFlags>] {
                fn from(initial: $BitFlags) -> Self {
                    Self::new(initial)
                }
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    atomic_bitflags! {
        #[derive(PartialEq, Eq, Debug, Clone, Copy)]
        pub struct TestFlags: u32 {
            const A = 1 << 0;
            const B = 1 << 1;
            const C = 1 << 2;
        }
    }

    #[test]
    fn test_atomic_bitflags() {
        let atomic = AtomicTestFlags::new(TestFlags::A);
        assert_eq!(atomic.load(Ordering::Relaxed), TestFlags::A);

        atomic.store(TestFlags::B, Ordering::Relaxed);
        assert_eq!(atomic.load(Ordering::Relaxed), TestFlags::B);

        let prev = atomic.fetch_or(TestFlags::C, Ordering::Relaxed);
        assert_eq!(prev, TestFlags::B);
        assert_eq!(atomic.load(Ordering::Relaxed), TestFlags::B | TestFlags::C);

        let prev = atomic.fetch_and(TestFlags::C, Ordering::Relaxed);
        assert_eq!(prev, TestFlags::B | TestFlags::C);
        assert_eq!(atomic.load(Ordering::Relaxed), TestFlags::C);
    }

    #[test]
    fn test_update() {
        let atomic = AtomicTestFlags::new(TestFlags::A | TestFlags::B);

        // Update A to 0, leaving B as is. Mask is A. Value is 0.
        let prev =
            atomic.update(TestFlags::empty(), TestFlags::A, Ordering::Relaxed, Ordering::Relaxed);
        assert_eq!(prev, TestFlags::A | TestFlags::B);
        assert_eq!(atomic.load(Ordering::Relaxed), TestFlags::B);

        // Update A to 1, leaving B as is. Mask is A. Value is A.
        let prev = atomic.update(TestFlags::A, TestFlags::A, Ordering::Relaxed, Ordering::Relaxed);
        assert_eq!(prev, TestFlags::B);
        assert_eq!(atomic.load(Ordering::Relaxed), TestFlags::A | TestFlags::B);

        // Update B to 0, A to 0. Mask is A | B. Value is 0.
        let prev = atomic.update(
            TestFlags::empty(),
            TestFlags::A | TestFlags::B,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        assert_eq!(prev, TestFlags::A | TestFlags::B);
        assert_eq!(atomic.load(Ordering::Relaxed), TestFlags::empty());
    }

    #[test]
    fn test_from() {
        let atomic: AtomicTestFlags = TestFlags::A.into();
        assert_eq!(atomic.load(Ordering::Relaxed), TestFlags::A);
    }
}
