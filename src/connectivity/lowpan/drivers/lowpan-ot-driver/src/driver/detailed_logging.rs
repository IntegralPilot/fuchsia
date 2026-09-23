// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use super::*;
use crate::ot::LogLevel;
use anyhow::Error;
use std::cell::Cell;

#[derive(Debug)]
pub struct DetailedLogging {
    /// Store initial log level
    pub log_level_default: LogLevel,

    /// Store the enablement state
    pub detailed_logging_enabled: Cell<bool>,

    /// Store the logging level state
    pub detailed_logging_level: Cell<LogLevel>,
}

impl DetailedLogging {
    pub fn new() -> Self {
        DetailedLogging {
            log_level_default: ot::LogLevel::Info,
            detailed_logging_enabled: Cell::new(false),
            detailed_logging_level: Cell::new(ot::LogLevel::Info),
        }
    }

    pub fn process_detailed_logging_set(
        &self,
        detailed_logging_enabled: Option<bool>,
        detailed_logging_level: Option<LogLevel>,
    ) -> Result<(), Error> {
        if let Some(enabled) = detailed_logging_enabled {
            self.detailed_logging_enabled.set(enabled);
        };
        if let Some(level) = detailed_logging_level {
            self.detailed_logging_level.set(level);
        };
        if self.detailed_logging_enabled.get() {
            diagnostics_log::set_minimum_severity(self.detailed_logging_level.get());
            ot::set_logging_level(self.detailed_logging_level.get());
        } else {
            diagnostics_log::set_minimum_severity(self.log_level_default);
            ot::set_logging_level(self.log_level_default);
        };

        Ok(())
    }

    pub fn process_detailed_logging_get(&self) -> (bool, LogLevel) {
        let detailed_logging_enabled = self.detailed_logging_enabled.replace(false);
        self.detailed_logging_enabled.replace(detailed_logging_enabled);
        let current_log_level = ot::get_logging_level();
        (detailed_logging_enabled, current_log_level)
    }
}
