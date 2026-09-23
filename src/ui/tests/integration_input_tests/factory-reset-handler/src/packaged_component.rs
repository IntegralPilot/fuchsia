// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::traits::test_realm_component::TestRealmComponent;
use fuchsia_component_test::{ChildOptions, RealmBuilder, Ref};

enum LegacyOrModernUrl {
    ModernUrl(String),
}

/// A component which can be instantiated from a Fuchsia package.
pub(crate) struct PackagedComponent {
    name: String,
    source: LegacyOrModernUrl,
    eager: bool,
}

impl PackagedComponent {
    pub(crate) fn new_from_modern_url(
        name: impl Into<String>,
        modern_url: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            source: LegacyOrModernUrl::ModernUrl(modern_url.into()),
            eager: false,
        }
    }

    pub(crate) fn new_eager_from_modern_url(
        name: impl Into<String>,
        modern_url: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            source: LegacyOrModernUrl::ModernUrl(modern_url.into()),
            eager: true,
        }
    }
}

#[async_trait::async_trait]
impl TestRealmComponent for PackagedComponent {
    fn ref_(&self) -> Ref {
        Ref::child(&self.name)
    }

    async fn add_to_builder(&self, builder: &RealmBuilder) {
        match &self.source {
            LegacyOrModernUrl::ModernUrl(url) => {
                let mut options = ChildOptions::new();
                if self.eager {
                    options = options.eager();
                }
                builder.add_child(&self.name, url, options).await.unwrap();
            }
        }
    }
}
