// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::FfxFastbootError;
type Result<T> = std::result::Result<T, FfxFastbootError>;
use async_trait::async_trait;

pub mod resolvers;

#[async_trait]
pub trait FileResolver {
    async fn get_file(&mut self, file: &str) -> Result<String>;
}

////////////////////////////////////////////////////////////////////////////////
// tests
pub mod test {
    use super::*;
    use async_trait::async_trait;

    pub struct TestResolver {}

    impl TestResolver {
        pub fn new() -> Self {
            Self {}
        }
    }

    #[async_trait]
    impl FileResolver for TestResolver {
        async fn get_file(&mut self, file: &str) -> Result<String> {
            Ok(file.to_owned())
        }
    }
}
