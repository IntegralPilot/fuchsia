// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// If a type implements DefaultDrop and is then encapsulated in a WithDefaultDrop,
// the provided default_drop fn will be called when leaving scope. This can only
// be avoided by explicitly calling WithDefaultDrop::take().
//
// This is used to ensure that we cannot drop a responder without first sending
// some response, since this will otherwise cause an unwanted channel closure.
pub trait DefaultDrop {
    fn default_drop(self);
}

pub struct WithDefaultDrop<T: DefaultDrop>(Option<T>);

impl<T: DefaultDrop> WithDefaultDrop<T> {
    pub fn new(val: T) -> Self {
        Self(Some(val))
    }

    pub fn take(mut self) -> T {
        self.0.take().unwrap()
    }
}

impl<T: DefaultDrop> std::ops::Drop for WithDefaultDrop<T> {
    fn drop(&mut self) {
        if let Some(inner) = self.0.take() {
            inner.default_drop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_sync::Mutex;
    use std::sync::Arc;

    struct TestDrop(Arc<Mutex<bool>>);

    impl DefaultDrop for TestDrop {
        fn default_drop(self) {
            *self.0.lock() = true;
        }
    }

    #[test]
    fn test_default_drop() {
        let was_dropped = Arc::new(Mutex::new(false));
        {
            let _test_drop = WithDefaultDrop::new(TestDrop(Arc::clone(&was_dropped)));
        }
        assert!(*was_dropped.lock());
    }

    #[test]
    fn test_no_default_drop_after_take() {
        let was_dropped = Arc::new(Mutex::new(false));
        {
            let test_drop = WithDefaultDrop::new(TestDrop(Arc::clone(&was_dropped)));
            let _ = test_drop.take();
        }
        assert!(!*was_dropped.lock());
    }
}
