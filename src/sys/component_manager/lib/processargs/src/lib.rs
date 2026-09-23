// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fuchsia_runtime::{HandleInfo, HandleType};

use process_builder::{NamespaceEntry, StartupHandle};
use std::ffi::CString;
use std::iter::once;

#[derive(Default)]
pub struct ProcessArgs {
    /// Command-line arguments passed to the process.
    pub args: Vec<CString>,

    /// Environment variables passed to the process.
    pub environment_variables: Vec<CString>,

    /// Entries added to the process namespace.
    ///
    /// The paths in the namespace must be non-overlapping. See
    /// <https://fuchsia.dev/fuchsia-src/concepts/process/namespaces> for details.
    pub namespace_entries: Vec<NamespaceEntry>,

    /// Handles passed to the process.
    pub handles: Vec<StartupHandle>,
}

impl ProcessArgs {
    pub fn new() -> Self {
        ProcessArgs::default()
    }

    pub fn add_default_loader(&mut self) -> Result<&mut Self, zx::Status> {
        let loader = fuchsia_runtime::loader_svc()?;
        Ok(self.add_handles(once(StartupHandle {
            handle: loader,
            info: HandleInfo::new(HandleType::LdsvcLoader, 0),
        })))
    }

    pub fn add_args<I, T>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = T>,
        T: Into<CString>,
    {
        let args = args.into_iter().map(|s| s.into());
        self.args.extend(args);
        self
    }

    pub fn add_environment_variables<I, T>(&mut self, environment_variables: I) -> &mut Self
    where
        I: IntoIterator<Item = T>,
        T: Into<CString>,
    {
        let environment_variables = environment_variables.into_iter().map(|s| s.into());
        self.environment_variables.extend(environment_variables);
        self
    }

    pub fn add_handles<I>(&mut self, handles: I) -> &mut Self
    where
        I: IntoIterator<Item = StartupHandle>,
    {
        self.handles.extend(handles);
        self
    }
}
