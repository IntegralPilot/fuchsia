// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::any::Any;
use std::fmt::Debug;
use std::sync::Arc;

/// The trait that `WeakInstanceToken` holds.
pub trait WeakInstanceTokenAny: Debug + Send + Sync {
    fn as_any(&self) -> &dyn Any;
}

/// A type representing a weak pointer to a component.
/// This is type erased because the bedrock library shouldn't depend on
/// Component Manager types.
#[derive(Debug)]
pub struct WeakInstanceToken {
    pub inner: Box<dyn WeakInstanceTokenAny>,
}

impl WeakInstanceToken {
    /// Creates a new WeakInstanceToken that cannot be typecast into anything useful. Primarily
    /// useful in tests.
    pub fn new_invalid() -> Arc<Self> {
        #[derive(Debug)]
        struct Nothing;
        impl WeakInstanceTokenAny for Nothing {
            fn as_any(&self) -> &dyn Any {
                self
            }
        }
        Arc::new(Self { inner: Box::new(Nothing {}) })
    }

    #[cfg(target_os = "fuchsia")]
    pub fn try_into_directory_entry(
        self: Arc<Self>,
        _scope: vfs::execution_scope::ExecutionScope,
        _token: Arc<crate::WeakInstanceToken>,
    ) -> Result<Arc<dyn vfs::directory::entry::DirectoryEntry>, crate::ConversionError> {
        Err(crate::ConversionError::NotSupported)
    }
}
