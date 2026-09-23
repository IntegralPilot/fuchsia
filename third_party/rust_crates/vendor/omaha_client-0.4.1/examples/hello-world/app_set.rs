// Copyright 2024 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

use omaha_client::{app_set::AppSet, common::App};

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum AppIdSource {
    #[expect(dead_code)]
    VbMetadata,
    #[expect(dead_code)]
    ChannelConfig,
    DefaultEmpty,
}

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct AppMetadata {
    /// The source from which the app's ID was derived.
    pub appid_source: AppIdSource,
}

pub struct MinimalAppSet {
    system_app: App,
    system_app_metadata: AppMetadata,
}

impl MinimalAppSet {
    pub fn new(system_app: App, system_app_metadata: AppMetadata) -> Self {
        Self { system_app, system_app_metadata }
    }

    /// Get the system product id.
    /// Returns empty string if product id not set for the system app.
    #[expect(dead_code)]
    pub fn get_system_product_id(&self) -> &str {
        self.system_app.extra_fields.get("product_id").map(|s| &**s).unwrap_or("")
    }

    /// Get the current channel name from cohort name, returns empty string if no cohort name set
    /// for the app.
    #[expect(dead_code)]
    pub fn get_system_current_channel(&self) -> &str {
        self.system_app.get_current_channel()
    }

    /// Get the target channel name from cohort hint, fallback to current channel if no hint.
    #[expect(dead_code)]
    pub fn get_system_target_channel(&self) -> &str {
        self.system_app.get_target_channel()
    }

    /// Set the cohort hint of system app to |channel| and |id|.
    #[expect(dead_code)]
    pub fn set_system_target_channel(&mut self, channel: Option<String>, id: Option<String>) {
        self.system_app.set_target_channel(channel, id);
    }

    #[expect(dead_code)]
    pub fn get_system_app_metadata(&self) -> &AppMetadata {
        &self.system_app_metadata
    }
}

impl AppSet for MinimalAppSet {
    fn get_apps(&self) -> Vec<App> {
        std::iter::once(&self.system_app).cloned().collect()
    }
    fn iter_mut_apps(&mut self) -> Box<dyn Iterator<Item = &mut App> + '_> {
        Box::new(std::iter::once(&mut self.system_app))
    }
    fn get_system_app_id(&self) -> &str {
        &self.system_app.id
    }
}
