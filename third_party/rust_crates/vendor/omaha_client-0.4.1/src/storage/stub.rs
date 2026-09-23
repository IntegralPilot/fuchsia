// Copyright 2020 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

#![cfg(test)]

use super::*;
use futures::future::BoxFuture;
use futures::prelude::*;
use thiserror::Error;

/// The StubStorage return None or Err on everything.
#[derive(Debug)]
pub struct StubStorage;

#[derive(Debug, Error)]
pub enum StubErrors {
    #[error("StubStorage failed intentionally")]
    Intentional,
}

impl Storage for StubStorage {
    type Error = StubErrors;

    fn get_string<'a>(&self, _key: &str) -> BoxFuture<'_, Option<String>> {
        future::ready(None).boxed()
    }

    fn get_int<'a>(&self, _key: &str) -> BoxFuture<'_, Option<i64>> {
        future::ready(None).boxed()
    }

    fn get_bool<'a>(&self, _key: &str) -> BoxFuture<'_, Option<bool>> {
        future::ready(None).boxed()
    }

    fn set_string<'a>(
        &mut self,
        _key: &str,
        _value: &str,
    ) -> BoxFuture<'_, Result<(), Self::Error>> {
        future::ready(Err(StubErrors::Intentional)).boxed()
    }

    fn set_int<'a>(&mut self, _key: &str, _value: i64) -> BoxFuture<'_, Result<(), Self::Error>> {
        future::ready(Err(StubErrors::Intentional)).boxed()
    }

    fn set_bool<'a>(&mut self, _key: &str, _value: bool) -> BoxFuture<'_, Result<(), Self::Error>> {
        future::ready(Err(StubErrors::Intentional)).boxed()
    }

    fn remove<'a>(&mut self, _key: &str) -> BoxFuture<'_, Result<(), Self::Error>> {
        future::ready(Err(StubErrors::Intentional)).boxed()
    }

    fn commit(&mut self) -> BoxFuture<'_, Result<(), Self::Error>> {
        future::ready(Err(StubErrors::Intentional)).boxed()
    }
}

mod tests {
    use super::*;
    use futures::executor::block_on;

    #[test]
    fn test_set_get_remove_string() {
        block_on(async {
            assert!(StubStorage.set_string("key", "value").await.is_err());
            assert_eq!(StubStorage.get_string("key").await, None);
            assert!(StubStorage.remove("key").await.is_err());
        });
    }

    #[test]
    fn test_set_get_remove_int() {
        block_on(async {
            assert!(StubStorage.set_int("key", 123).await.is_err());
            assert_eq!(StubStorage.get_int("key").await, None);
            assert!(StubStorage.remove("key").await.is_err());
        });
    }

    #[test]
    fn test_set_get_remove_bool() {
        block_on(async {
            assert!(StubStorage.set_bool("key", true).await.is_err());
            assert_eq!(StubStorage.get_bool("key").await, None);
            assert!(StubStorage.remove("key").await.is_err());
        });
    }

    #[test]
    fn test_commit() {
        block_on(async {
            assert!(StubStorage.commit().await.is_err());
        });
    }
}
