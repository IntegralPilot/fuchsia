// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::types::Error;
use diagnostics_data::{Data, Inspect};
use flex_fuchsia_diagnostics::Selector;
use flex_fuchsia_sys2 as fsys2;
use serde::Serialize;
use std::fmt::Display;
use std::future::Future;

pub trait Command {
    type Result: Serialize + Display;
    fn execute<P: DiagnosticsProvider>(
        self,
        provider: &P,
    ) -> impl Future<Output = Result<Self::Result, Error>>;
}

pub trait DiagnosticsProvider: Send + Sync {
    fn snapshot(
        &self,
        accessor: Option<&str>,
        selectors: impl IntoIterator<Item = Selector>,
    ) -> impl Future<Output = Result<Vec<Data<Inspect>>, Error>>;

    /// Lists all ArchiveAccessor selectors.
    fn get_accessor_paths(&self) -> impl Future<Output = Result<Vec<String>, Error>>;

    /// Connect to a RealmQueryProxy for component discovery for fuzzy matching components
    fn realm_query(&self) -> &fsys2::RealmQueryProxy;
}
