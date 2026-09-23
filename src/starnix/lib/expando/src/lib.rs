// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_sync::Mutex;
use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::marker::{Send, Sync};
use std::ops::Deref;
use std::sync::Arc;

/// A spot in an `Expando`.
///
/// Holds a value of type `Arc<T>`.
#[derive(Debug)]
struct ExpandoSlot {
    value: Arc<dyn Any + Send + Sync>,
}

impl ExpandoSlot {
    fn new(value: Arc<dyn Any + Send + Sync>) -> Self {
        ExpandoSlot { value }
    }

    fn downcast<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        self.value.clone().downcast::<T>().ok()
    }
}

/// A lazy collection of values of every type.
///
/// An Expando contains a single instance of every type. The values are instantiated lazily
/// when accessed. Useful for letting modules add their own state to context objects without
/// requiring the context object itself to know about the types in every module.
///
/// Typically the type a module uses in the Expando will be private to that module, which lets
/// the module know that no other code is accessing its slot on the expando.
#[derive(Debug, Default)]
pub struct Expando {
    properties: Mutex<BTreeMap<TypeId, ExpandoSlot>>,
}

impl Expando {
    /// Get the slot in the expando associated with the given type.
    ///
    /// The slot is added to the expando lazily but the same instance is returned every time the
    /// expando is queried for the same type.
    pub fn get<T: Any + Send + Sync + Default + 'static>(&self) -> Arc<T> {
        let mut properties = self.properties.lock();
        let type_id = TypeId::of::<T>();
        let slot =
            properties.entry(type_id).or_insert_with(|| ExpandoSlot::new(Arc::new(T::default())));
        assert_eq!(type_id, slot.value.deref().type_id());
        slot.downcast().expect("downcast of expando slot was successful")
    }

    /// Get the slot in the expando associated with the given type, running `init` to initialize
    /// the slot if needed.
    ///
    /// The slot is added to the expando lazily but the same instance is returned every time the
    /// expando is queried for the same type.
    pub fn get_or_init<T: Any + Send + Sync + 'static>(&self, init: impl FnOnce() -> T) -> Arc<T> {
        self.get_or_try_init::<T, ()>(|| Ok(init())).expect("infallible initializer")
    }

    /// Get the slot in the expando associated with the given type, running `try_init` to initialize
    /// the slot if needed. Returns an error only if `try_init` returns an error.
    ///
    /// The slot is added to the expando lazily but the same instance is returned every time the
    /// expando is queried for the same type.
    pub fn get_or_try_init<T: Any + Send + Sync + 'static, E>(
        &self,
        try_init: impl FnOnce() -> Result<T, E>,
    ) -> Result<Arc<T>, E> {
        let type_id = TypeId::of::<T>();

        // Acquire the lock each time we want to look at the map so that user-provided initializer
        // can use the expando too.
        if let Some(slot) = self.properties.lock().get(&type_id) {
            assert_eq!(type_id, slot.value.deref().type_id());
            return Ok(slot.downcast().expect("downcast of expando slot was successful"));
        }

        // Initialize the new value without holding the lock.
        let newly_init = Arc::new(try_init()?);

        // Only insert the newly-initialized value if no other threads got there first.
        let mut properties = self.properties.lock();
        let slot = properties.entry(type_id).or_insert_with(|| ExpandoSlot::new(newly_init));
        assert_eq!(type_id, slot.value.deref().type_id());
        Ok(slot.downcast().expect("downcast of expando slot was successful"))
    }

    /// Get the slot in the expando associated with the given type if it has previously been
    /// initialized.
    pub fn peek<T: Any + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let properties = self.properties.lock();
        let type_id = TypeId::of::<T>();
        let slot = properties.get(&type_id)?;
        assert_eq!(type_id, slot.value.deref().type_id());
        Some(slot.downcast().expect("downcast of expando slot was successful"))
    }

    /// Remove the provided type from the expando if it is present.
    pub fn remove<T: Any + Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        let mut properties = self.properties.lock();
        let type_id = TypeId::of::<T>();
        let slot = properties.remove(&type_id)?;
        assert_eq!(type_id, slot.value.deref().type_id());
        Some(slot.downcast().expect("downcast of expando slot was successful"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct MyStruct {
        counter: Mutex<i32>,
    }

    #[test]
    fn basic_test() {
        let expando = Expando::default();
        let first = expando.get::<MyStruct>();
        assert_eq!(*first.counter.lock(), 0);
        *first.counter.lock() += 1;
        let second = expando.get::<MyStruct>();
        assert_eq!(*second.counter.lock(), 1);
    }

    #[test]
    fn user_initializer() {
        let expando = Expando::default();
        let first = expando.get_or_init(|| String::from("hello"));
        assert_eq!(first.as_str(), "hello");
        let second = expando.get_or_init(|| String::from("world"));
        assert_eq!(
            second.as_str(),
            "hello",
            "expando must have preserved value from original initializer"
        );
        assert_eq!(Arc::as_ptr(&first), Arc::as_ptr(&second));
    }

    #[test]
    fn nested_user_initializer() {
        let expando = Expando::default();
        let first = expando.get_or_init(|| expando.get::<u32>().to_string());
        assert_eq!(first.as_str(), "0");
        let second = expando.get_or_init(|| expando.get::<u32>().to_string());
        assert_eq!(Arc::as_ptr(&first), Arc::as_ptr(&second));
    }

    #[test]
    fn failed_init_can_be_retried() {
        let expando = Expando::default();
        let failed = expando.get_or_try_init::<String, String>(|| Err(String::from("oops")));
        assert_eq!(failed.unwrap_err().as_str(), "oops");

        let succeeded = expando.get_or_try_init::<String, String>(|| Ok(String::from("hurray")));
        assert_eq!(succeeded.unwrap().as_str(), "hurray");
    }

    #[test]
    fn peek_works() {
        let expando = Expando::default();
        assert_eq!(expando.peek::<String>(), None);
        let from_init = expando.get_or_init(|| String::from("hello"));
        let from_peek = expando.peek::<String>().unwrap();
        assert_eq!(from_peek.as_str(), "hello");
        assert_eq!(Arc::as_ptr(&from_init), Arc::as_ptr(&from_peek));
    }

    #[test]
    fn remove_works() {
        let expando = Expando::default();
        assert_eq!(expando.peek::<String>(), None);
        let from_init = expando.get_or_init(|| String::from("hello"));
        let removed = expando.remove::<String>().unwrap();
        assert_eq!(removed.as_str(), "hello");
        assert_eq!(Arc::as_ptr(&from_init), Arc::as_ptr(&removed));
        assert_eq!(expando.peek::<String>(), None);
    }
}
