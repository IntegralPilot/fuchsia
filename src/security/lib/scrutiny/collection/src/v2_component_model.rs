// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::collection::DataCollection;
use cm_fidl_analyzer::component_model::ComponentModelForAnalyzer;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Deserialize, Serialize)]
pub struct V2ComponentModel {
    pub deps: HashSet<PathBuf>,
    // TODO(pesk): investigate options for partially serializing `component_model`.
    #[serde(skip)]
    pub component_model: Arc<ComponentModelForAnalyzer>,
    // TODO(pesk): replace this with a serializable error type.
    // TODO(https://fxbug.dev/332407032): Remove or explain #[allow(dead_code)].
    #[allow(dead_code)]
    #[serde(skip)]
    pub errors: Vec<anyhow::Error>,
}

impl V2ComponentModel {
    pub fn new(
        deps: HashSet<PathBuf>,
        component_model: Arc<ComponentModelForAnalyzer>,
        errors: Vec<anyhow::Error>,
    ) -> Self {
        Self { deps, component_model, errors }
    }
}

impl DataCollection for V2ComponentModel {
    fn collection_name() -> String {
        "V2 Component Model Collection".to_string()
    }
    fn collection_description() -> String {
        "Contains a V2 component model based on the root.cm and all v2 manifests found in the build."
            .to_string()
    }
}
