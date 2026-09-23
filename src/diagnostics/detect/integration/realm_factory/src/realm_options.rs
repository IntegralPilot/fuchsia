// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_test_detect_factory as ftest;
use std::collections::HashMap;

/// An adaptor for the generated FIDL RealmOptions struct.
#[derive(Default)]
pub(crate) struct RealmOptions {
    /// Canned JSON responses to send when triage-detect fetches inspect data.
    /// Responses are sent in the order they appear in this vector.
    pub inspect_data: Vec<zx::Vmo>,

    /// Config files to provide to the triage-detect component
    /// in /config/data.
    pub config_files: HashMap<String, zx::Vmo>,
}

impl RealmOptions {
    pub fn new() -> Self {
        Self { ..Default::default() }
    }

    /// Adds a file to be written to /config/data.
    pub fn add_config_file(&mut self, filename: impl Into<String>, contents: zx::Vmo) {
        self.config_files.insert(filename.into(), contents);
    }

    /// Adds stub inspect data to send when diagnostics are fetched.
    pub fn add_inspect_data(&mut self, contents: zx::Vmo) {
        self.inspect_data.push(contents);
    }
}

impl From<ftest::RealmOptions> for RealmOptions {
    fn from(mut other: ftest::RealmOptions) -> Self {
        let mut config_file_count = 0;
        let mut realm_opts = Self::new();

        if other.program_config.is_some() {
            realm_opts.add_config_file("config.json", other.program_config.take().unwrap());
        }

        if other.triage_configs.is_some() {
            let triage_configs = other.triage_configs.take().unwrap();
            for contents in triage_configs {
                config_file_count += 1;
                let config_file_name = format!("{config_file_count}.triage");
                realm_opts.add_config_file(config_file_name, contents);
            }
        }

        if other.inspect_data.is_some() {
            let inspect_data = other.inspect_data.take().unwrap();
            for contents in inspect_data {
                realm_opts.add_inspect_data(contents);
            }
        }

        realm_opts
    }
}
