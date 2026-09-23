// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::metrics;
use fidl_fuchsia_update_installer_ext::options::Range;
use fidl_fuchsia_update_installer_ext::{Initiator as ExtInitiator, Options};
use std::time::{Instant, SystemTime};

/// Configuration for an update attempt.
#[derive(PartialEq, Eq, Clone)]
pub struct Config {
    pub initiator: Initiator,
    pub update_url: http::Uri,
    pub should_write_recovery: bool,
    pub(super) start_time: SystemTime,
    pub(super) start_time_mono: Instant,
    pub allow_attach_to_existing_attempt: bool,
    pub manifest_range: Option<Range>,
    pub manifest_headers: Vec<fidl_fuchsia_net_http::Header>,
}

impl Config {
    /// Constructs update configuration from url, options and signature.
    pub fn new(update_url: http::Uri, options: Options) -> Self {
        let start_time = SystemTime::now();
        let start_time_mono =
            metrics::system_time_to_monotonic_time(start_time).unwrap_or_else(Instant::now);

        Self {
            initiator: options.initiator.into(),
            update_url,
            should_write_recovery: options.should_write_recovery,
            start_time,
            start_time_mono,
            allow_attach_to_existing_attempt: options.allow_attach_to_existing_attempt,
            manifest_range: options.manifest_range,
            manifest_headers: options.manifest_headers,
        }
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("initiator", &self.initiator)
            .field("update_url", &self.update_url.to_string())
            .field("should_write_recovery", &self.should_write_recovery)
            .field("start_time", &chrono::DateTime::<chrono::Utc>::from(self.start_time))
            .field("start_time_mono", &self.start_time_mono)
            .field("allow_attach_to_existing_attempt", &self.allow_attach_to_existing_attempt)
            .field("manifest_range", &self.manifest_range)
            .field("manifest_headers", &self.manifest_headers)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Initiator {
    Automatic,
    Manual,
}

impl From<Initiator> for ExtInitiator {
    fn from(args_initiator: Initiator) -> Self {
        match args_initiator {
            Initiator::Manual => ExtInitiator::User,
            Initiator::Automatic => ExtInitiator::Service,
        }
    }
}

impl From<ExtInitiator> for Initiator {
    fn from(ext_initiator: ExtInitiator) -> Self {
        match ext_initiator {
            ExtInitiator::User => Initiator::Manual,
            ExtInitiator::Service => Initiator::Automatic,
        }
    }
}

#[cfg(test)]
pub struct ConfigBuilder<'a> {
    update_url: &'a str,
    should_write_recovery: bool,
    allow_attach_to_existing_attempt: bool,
}

#[cfg(test)]
impl<'a> ConfigBuilder<'a> {
    pub fn new() -> Self {
        Self {
            update_url: "fuchsia-pkg://fuchsia.test/update",
            should_write_recovery: true,
            allow_attach_to_existing_attempt: false,
        }
    }
    pub fn update_url(mut self, update_url: &'a str) -> Self {
        self.update_url = update_url;
        self
    }
    pub fn allow_attach_to_existing_attempt(
        mut self,
        allow_attach_to_existing_attempt: bool,
    ) -> Self {
        self.allow_attach_to_existing_attempt = allow_attach_to_existing_attempt;
        self
    }
    pub fn should_write_recovery(mut self, should_write_recovery: bool) -> Self {
        self.should_write_recovery = should_write_recovery;
        self
    }
    pub fn build(self) -> Result<Config, anyhow::Error> {
        let Self { update_url, should_write_recovery, allow_attach_to_existing_attempt } = self;
        Ok(Config::new(
            update_url.parse()?,
            Options {
                allow_attach_to_existing_attempt,
                should_write_recovery,
                initiator: ExtInitiator::User,
                manifest_range: None,
                manifest_headers: vec![],
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new() {
        let options = Options {
            initiator: ExtInitiator::User,
            allow_attach_to_existing_attempt: true,
            should_write_recovery: true,
            manifest_range: None,
            manifest_headers: vec![],
        };
        let update_url: http::Uri = "fuchsia-pkg://fuchsia.test/foo".parse().unwrap();

        let config = Config::new(update_url.clone(), options);

        assert_matches::assert_matches!(
            config,
            Config {
                initiator: Initiator::Manual,
                update_url: url,
                should_write_recovery: true,
                allow_attach_to_existing_attempt: true,
                manifest_range: None,
                ..
            } if url == update_url
        );
    }
}
