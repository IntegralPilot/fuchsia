// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Exposes the OnceCell crate for use in async code.

use async_lock::Mutex;
use once_cell::sync::OnceCell;

/// Wrapper presenting an async interface to a OnceCell.
#[derive(Debug)]
pub struct Once<T> {
    mutex: Mutex<()>,
    value: OnceCell<T>,
}

impl<T> Default for Once<T> {
    fn default() -> Self {
        Self { mutex: Mutex::new(()), value: OnceCell::new() }
    }
}

impl<T> Once<T> {
    /// Constructor.
    pub fn new() -> Self {
        Self { mutex: Mutex::new(()), value: OnceCell::new() }
    }

    /// Async wrapper around OnceCell's `get_or_init`.
    pub async fn get_or_init<'a, F>(&'a self, f: F) -> &'a T
    where
        F: AsyncFnOnce() -> T,
    {
        if let Some(t) = self.value.get() {
            t
        } else {
            let _mut = self.mutex.lock().await;
            // Someone raced us and just released the lock
            if let Some(t) = self.value.get() {
                t
            } else {
                let t = f().await;
                self.value.set(t).unwrap_or_else(|_| panic!("race in async-cell!"));
                self.value.get().unwrap()
            }
        }
    }

    /// Async wrapper around OnceCell's `get_or_try_init`.
    pub async fn get_or_try_init<'a, F, E>(&'a self, f: F) -> Result<&'a T, E>
    where
        F: AsyncFnOnce() -> Result<T, E>,
    {
        if let Some(t) = self.value.get() {
            Ok(t)
        } else {
            let _mut = self.mutex.lock().await;
            // Someone raced us and just released the lock
            if let Some(t) = self.value.get() {
                Ok(t)
            } else {
                let r = f().await;
                match r {
                    Ok(t) => {
                        self.value.set(t).unwrap_or_else(|_| panic!("race in async-cell!"));
                        Ok(self.value.get().unwrap())
                    }
                    Err(e) => Err(e),
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures_lite::future::block_on;
    use std::sync::LazyLock;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static ONCE: LazyLock<Once<bool>> = LazyLock::new(Once::new);
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn test_get_or_init() {
        let val = block_on(ONCE.get_or_init(async || {
            let _: usize = COUNTER.fetch_add(1, Ordering::SeqCst);
            true
        }));

        assert_eq!(*val, true);
        assert_eq!(COUNTER.load(Ordering::SeqCst), 1);

        let val = block_on(ONCE.get_or_init(async || {
            let _: usize = COUNTER.fetch_add(1, Ordering::SeqCst);
            false
        }));

        assert_eq!(*val, true);
        assert_eq!(COUNTER.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_get_or_init_default_initializer() {
        let val = block_on(ONCE.get_or_init(async || {
            let _: usize = COUNTER.fetch_add(1, Ordering::SeqCst);
            true
        }));

        assert_eq!(*val, true);
        assert_eq!(COUNTER.load(Ordering::SeqCst), 1);

        let val = block_on(ONCE.get_or_init(async || {
            let _: usize = COUNTER.fetch_add(1, Ordering::SeqCst);
            false
        }));

        assert_eq!(*val, true);
        assert_eq!(COUNTER.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_get_or_try_init() {
        let initializer = async || {
            let val = COUNTER.fetch_add(1, Ordering::SeqCst);
            if val == 0 { Err(std::io::Error::other("first attempt fails")) } else { Ok(true) }
        };

        let val = block_on(ONCE.get_or_try_init(initializer));

        assert!(val.is_err());
        assert_eq!(COUNTER.load(Ordering::SeqCst), 1);

        // The initializer gets another chance to run because the first attempt failed.
        let val = block_on(ONCE.get_or_try_init(initializer));
        assert_eq!(*val.unwrap(), true);
        assert_eq!(COUNTER.load(Ordering::SeqCst), 2);

        // The initializer never runs again...
        let val = block_on(ONCE.get_or_try_init(initializer));
        assert_eq!(*val.unwrap(), true);
        assert_eq!(COUNTER.load(Ordering::SeqCst), 2);
    }
}
