// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{FhoEnvironment, TryFromEnv};

use std::fmt::Display;
use writer::ToolIO;

/// NullWriter, for use in contexts such as ToolSuites which need
/// a "Writer" in order to implement `FfxTool`, but never actually
/// use it.
pub struct NullWriter;

#[async_trait::async_trait(?Send)]
impl TryFromEnv for NullWriter {
    type Error = std::convert::Infallible;
    async fn try_from_env(_env: &FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        Ok(NullWriter)
    }
}
impl std::io::Write for NullWriter {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        unimplemented!()
    }

    fn flush(&mut self) -> std::io::Result<()> {
        unimplemented!()
    }
}
pub struct NullItem;
impl Display for NullItem {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unimplemented!()
    }
}
impl ToolIO for NullWriter {
    type OutputItem = NullItem;

    fn is_machine(&self) -> bool {
        unimplemented!()
    }

    fn stderr(&mut self) -> &mut dyn std::io::Write {
        unimplemented!()
    }

    fn item(&mut self, _value: &Self::OutputItem) -> writer::Result<()>
    where
        Self::OutputItem: std::fmt::Display,
    {
        unimplemented!()
    }
}
