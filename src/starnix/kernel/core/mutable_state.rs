// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Macros used with struct containing an immutable state and a RwLock to a mutable state.
//!
//! These macros define a new type of read and write guards that allow access to both the
//! mutable and immutable state. To use it, one must:
//! - Define the main struct (e.g. `Foo`) for the object with the immutable state.
//! - Define a struct for the mutable state (e.g. `FooMutableState`).
//! - Have a RwLock<> in the main struct for the mutable state (e.g. `mutable_state:
//! RwLock<FooMutableState>`).
//! - In the implementation of the main struct, add a call to the `state_accessor` macros:
//! ```
//! impl Foo {
//!   state_accessor!(Foo, mutable_state);
//! }
//! ```
//! - Write the method on the guards using the state_implementation macro:
//! ```
//! #[apply(state_implementation!)]
//! impl FooMutableState<Base=Foo> {
//!     // Some comment
//!     fn do_something(&self) -> i32 {
//!         0
//!     }
//! }
//! ```
//!
//! # Complete example:
//!
//! ```
//! pub struct FooMutableState {
//!     y: i32,
//! }
//!
//! pub struct Foo {
//!     x: i32,
//!     mutable_state: RwLock<FooMutableState>,
//! }
//!
//! impl Foo {
//!     fn new() -> Self {
//!         Self { x: 2, mutable_state: RwLock::new(FooMutableState { y: 3 }) }
//!     }
//!
//!     state_accessor!(Foo, mutable_state);
//! }
//!
//! #[attr(state_implementation!)]
//! impl FooMutableState<Base=Foo> {
//!     // Some comment
//!     fn x_and_y(&self) -> i32 {
//!         self.base.x + self.y
//!     }
//!     /// Some rustdoc.
//!     pub fn pub_x_and_y(&self) -> i32 {
//!         self.x_and_y()
//!     }
//!     fn do_something(&self) {}
//!     fn set_y(&mut self, other_y: i32) {
//!         self.y = other_y;
//!     }
//!     pub fn pub_set_y(&mut self, other_y: i32) {
//!         self.set_y(other_y)
//!     }
//!     fn do_something_mutable(&mut self) {
//!         self.do_something();
//!     }
//!
//!     #[allow(dead_code)]
//!     pub fn with_lifecycle<'a>(&self, _n: &'a u32) {}
//!     #[allow(dead_code)]
//!     pub fn with_type<T>(&self, _n: &T) {}
//!     #[allow(dead_code)]
//!     pub fn with_lifecycle_and_type<'a, T>(&self, _n: &'a T) {}
//!     #[allow(dead_code)]
//!     pub fn with_lifecycle_on_self<'a, T>(&'a self, _n: &'a T) {}
//! }
//! ```
//!
//! # Generated code
//!
//! ```
//! pub struct FooMutableState {
//!     y: i32,
//! }
//! pub struct Foo {
//!     x: i32,
//!     mutable_state: RwLock<FooMutableState>,
//! }
//! impl Foo {
//!     fn new() -> Self {
//!         Self {
//!             x: 2,
//!             mutable_state: RwLock::new(FooMutableState { y: 3 }),
//!         }
//!     }
//!
//!     #[allow(dead_code)]
//!     pub fn read<'a>(self: &'a Foo) -> FooReadGuard<'a> {
//!         ReadGuard::new(self, self.mutable_state.read())
//!     }
//!     #[allow(dead_code)]
//!     pub fn write<'a>(self: &'a Foo) -> FooWriteGuard<'a> {
//!         WriteGuard::new(self, self.mutable_state.write())
//!     }
//! }
//!
//! #[allow(dead_code)]
//! pub type FooReadGuard<'guard_lifetime> = ReadGuard<'guard_lifetime, Foo, FooMutableState>;
//! #[allow(dead_code)]
//! pub type FooWriteGuard<'guard_lifetime> = WriteGuard<'guard_lifetime, Foo, FooMutableState>;
//! #[allow(dead_code)]
//! pub type FooStateRef<'ref_lifetime> = StateRef<'ref_lifetime, Foo, FooMutableState>;
//! #[allow(dead_code)]
//! pub type FooStateMutRef<'ref_lifetime> = StateMutRef<'ref_lifetime, Foo, FooMutableState>;
//!
//! impl<'guard, G: 'guard + std::ops::Deref<Target = FooMutableState>> Guard<'guard, Foo, G> {
//!     fn x_and_y(&self) -> i32 {
//!         self.base.x + self.y
//!     }
//!     /// Some rustdoc.
//!     pub fn pub_x_and_y(&self) -> i32 {
//!         self.x_and_y()
//!     }
//!     fn do_something(&self) {}
//!     #[allow(dead_code)]
//!     pub fn with_lifecycle<'a>(&self, _n: &'a u32) {}
//!     #[allow(dead_code)]
//!     pub fn with_type<T>(&self, _n: &T) {}
//!     #[allow(dead_code)]
//!     pub fn with_lifecycle_and_type<'a, T>(&self, _n: &'a T) {}
//!     #[allow(dead_code)]
//!     pub fn with_lifecycle_on_self<'a, T>(&'a self, _n: &'a T) {}
//! }
//!
//! impl<'guard, G: 'guard + std::ops::DerefMut<Target = FooMutableState>> Guard<'guard, Foo, G> {
//!     fn set_y(&mut self, other_y: i32) {
//!         self.y = other_y;
//!     }
//!     pub fn pub_set_y(&mut self, other_y: i32) {
//!         self.set_y(other_y)
//!     }
//!     fn do_something_mutable(&mut self) {
//!         self.do_something();
//!     }
//! }
//! ```

use starnix_sync::{LockDepGuard, LockDepReadGuard, LockDepWriteGuard, MutexGuard};
use std::ops::{Deref, DerefMut};

/// Create the read() and write() accessor to respectively access the read guard and write guard.
///
/// For a base struct named `Foo`, the read guard will be a struct named `FooReadGuard` and the
/// write guard a struct named `FooWriteGuard`.
macro_rules! state_accessor {
    ($base_name:ident, $field_name:ident, $base_type:ty) => {
        paste::paste! {
        #[allow(dead_code)]
        pub fn read<'a>(self: &'a $base_type) -> [<$base_name ReadGuard>]<'a> {
            $crate::mutable_state::ReadGuard::new(self, self.$field_name.read())
        }
        #[allow(dead_code)]
        pub fn write<'a>(self: &'a $base_type) -> [<$base_name WriteGuard>]<'a> {
            $crate::mutable_state::WriteGuard::new(self, self.$field_name.write())
        }
        }
    };
    ($base_name:ident, $field_name:ident) => {
        state_accessor!($base_name, $field_name, $base_name);
    };
}

/// Create the structs for the read and write guards using the methods defined inside the macro.
macro_rules! state_implementation {
    (impl $mutable_name:ident<Base=$base_name:ident> {
        $(
            $tt:tt
        )*
    }) => {
        state_implementation! {
            impl $mutable_name<Base = $base_name, BaseType = $base_name> {
                $($tt)*
            }
        }
    };
    (impl $mutable_name:ident<Base=$base_name:ident, BaseType = $base_type:ty> {
        $(
            $tt:tt
        )*
    }) => {
        paste::paste! {
        #[allow(dead_code)]
        pub type [<$base_name ReadGuard>]<'guard_lifetime> = $crate::mutable_state::ReadGuard<'guard_lifetime, $base_type,  $mutable_name>;
        #[allow(dead_code)]
        pub type [<$base_name WriteGuard>]<'guard_lifetime> = $crate::mutable_state::WriteGuard<'guard_lifetime, $base_type, $mutable_name>;
        #[allow(dead_code)]
        pub type [<$base_name StateRef>]<'ref_lifetime> = $crate::mutable_state::StateRef<'ref_lifetime, $base_type, $mutable_name>;
        #[allow(dead_code)]
        pub type [<$base_name StateMutRef>]<'ref_lifetime> = $crate::mutable_state::StateMutRef<'ref_lifetime, $base_type, $mutable_name>;

        impl<'guard, G: 'guard + std::ops::Deref<Target=$mutable_name>> $crate::mutable_state::Guard<'guard, $base_type, G> {
            filter_methods_macro::filter_methods!(RoMethod, $($tt)*);
        }

        impl<'guard, G: 'guard + std::ops::DerefMut<Target=$mutable_name>> $crate::mutable_state::Guard<'guard, $base_type, G> {
            filter_methods_macro::filter_methods!(RwMethod, $($tt)*);
        }
        }
    };
}

pub struct Guard<'a, B, G> {
    pub base: &'a B,
    guard: G,
}
pub type ReadGuard<'a, B, S> = Guard<'a, B, LockDepReadGuard<'a, S>>;
pub type WriteGuard<'a, B, S> = Guard<'a, B, LockDepWriteGuard<'a, S>>;
pub type StateRef<'a, B, S> = Guard<'a, B, &'a S>;
pub type StateMutRef<'a, B, S> = Guard<'a, B, &'a mut S>;

impl<'a, B, S> Guard<'a, B, MutexGuard<'a, S>> {
    /// Executes the given closure while temporarily dropping the lock.
    ///
    /// The lock is dropped before the closure executes and re-acquired
    /// after it finishes.
    pub fn unlocked<F, U>(s: &mut Self, f: F) -> U
    where
        F: FnOnce() -> U,
    {
        MutexGuard::unlocked(&mut s.guard, f)
    }
}

impl<'a, B, S> Guard<'a, B, LockDepGuard<'a, S>> {
    /// Executes the given closure while temporarily dropping the lock.
    ///
    /// The lock is dropped before the closure executes and re-acquired
    /// after it finishes.
    pub fn unlocked<F, U>(s: &mut Self, f: F) -> U
    where
        F: FnOnce() -> U,
    {
        LockDepGuard::unlocked(&mut s.guard, f)
    }
}

impl<'guard, B, S, G: 'guard + Deref<Target = S>> Guard<'guard, B, G> {
    pub fn new(base: &'guard B, guard: G) -> Self {
        Self { base, guard }
    }
    pub fn as_ref(&self) -> StateRef<'_, B, S> {
        Guard { base: self.base, guard: self.guard.deref() }
    }
}

impl<'guard, B, S, G: 'guard + DerefMut<Target = S>> Guard<'guard, B, G> {
    pub fn as_mut(&mut self) -> StateMutRef<'_, B, S> {
        Guard { base: self.base, guard: self.guard.deref_mut() }
    }
}

impl<'a, B, S, G: Deref<Target = S>> Deref for Guard<'a, B, G> {
    type Target = S;
    fn deref(&self) -> &Self::Target {
        self.guard.deref()
    }
}

impl<'a, B, S, G: DerefMut<Target = S>> DerefMut for Guard<'a, B, G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.deref_mut()
    }
}

// Public re-export of macros allows them to be used like regular rust items.
pub(crate) use state_accessor;
pub(crate) use state_implementation;

#[cfg(test)]
mod test {
    use macro_rules_attribute::apply;
    use starnix_sync::{LockDepRwLock, lock_ordering};
    lock_ordering! {
        Terminal(FooLock),
    }

    pub struct FooMutableState {
        y: i32,
    }

    pub struct Foo {
        x: i32,
        mutable_state: LockDepRwLock<FooMutableState, FooLock>,
    }

    impl Foo {
        fn new() -> Self {
            Self { x: 2, mutable_state: FooMutableState { y: 3 }.into() }
        }

        state_accessor!(Foo, mutable_state);
    }

    #[apply(state_implementation!)]
    impl FooMutableState<Base = Foo> {
        // Some comment
        fn x_and_y(&self) -> i32 {
            self.base.x + self.y
        }
        /// Some rustdoc.
        pub fn pub_x_and_y(&self) -> i32 {
            self.x_and_y()
        }
        fn do_something(&self) {}
        fn set_y(&mut self, other_y: i32) {
            self.y = other_y;
        }
        pub fn pub_set_y(&mut self, other_y: i32) {
            self.set_y(other_y)
        }
        fn do_something_mutable(&mut self) {
            self.do_something();
        }

        #[allow(dead_code, clippy::needless_lifetimes)]
        pub fn with_lifecycle<'a>(&self, _n: &'a u32) {}
        #[allow(dead_code)]
        pub fn with_type<T>(&self, _n: &T) {}
        #[allow(dead_code)]
        pub fn with_type_and_where<T>(&self, _n: &T)
        where
            T: Copy,
        {
        }
        #[allow(dead_code)]
        pub fn with_type_and_bound<T: Copy>(&self, _n: &T) {}
        #[allow(dead_code)]
        pub fn with_multiple_types_and_bound_and_where<T: Copy, U>(&self, _n: &T)
        where
            U: Copy,
        {
        }
        #[allow(dead_code, clippy::needless_lifetimes)]
        pub fn with_lifecycle_and_type<'a, T>(&self, _n: &'a T) {}
        #[allow(dead_code, clippy::needless_lifetimes)]
        pub fn with_lifecycle_on_self<'a, T>(&'a self, _n: &'a T) {}
    }

    fn take_foo_state(foo_state: &FooStateRef<'_>) -> i32 {
        foo_state.pub_x_and_y()
    }

    #[::fuchsia::test]
    fn test_generation() {
        let foo = Foo::new();

        assert_eq!(foo.read().x_and_y(), 5);
        assert_eq!(foo.read().pub_x_and_y(), 5);
        assert_eq!(foo.write().pub_x_and_y(), 5);
        foo.write().set_y(22);
        assert_eq!(foo.read().pub_x_and_y(), 24);
        assert_eq!(foo.write().pub_x_and_y(), 24);
        foo.write().pub_set_y(20);
        assert_eq!(take_foo_state(&foo.read().as_ref()), 22);
        assert_eq!(take_foo_state(&foo.write().as_ref()), 22);

        foo.read().do_something();
        foo.write().do_something();
        foo.write().do_something_mutable();
    }
}
