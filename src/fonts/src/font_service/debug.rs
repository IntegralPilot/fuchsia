// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, Result};
use fidl_fuchsia_fonts as fonts;
use log::debug;
use std::stringify;

macro_rules! format_field {
    ($debug_struct:expr, $parent:expr, $field:ident, $wrapper:path) => {
        if let Some(f) = &$parent.0.$field {
            $debug_struct.field(std::stringify!($field), &$wrapper(f));
        }
    };
    ($debug_struct:expr, $parent:expr, $field:ident) => {
        if let Some(f) = &$parent.0.$field {
            $debug_struct.field(std::stringify!($field), f);
        }
    };
}

/// Returns true if this configuration allows verbose logging.
pub fn is_verbose_logging() -> bool {
    let verbosity = Verbosity::load_from_config_data();
    let verbose = verbosity.is_verbose_logging();
    debug!("verbosity: {}", verbose);
    verbose
}

/// Logging verbosity settings.
///
/// Allows printing different verbosity log lines, depending on the
/// product configuration.
struct Verbosity {
    // The verbatim structured configuration.
    verbose_logging: bool,
}

impl Verbosity {
    // Loads the verbosity configuration. It may be called only once.
    fn load_from_config_data() -> Verbosity {
        let verbose_logging = crate::font_service::config::as_ref().verbose_logging;
        Verbosity { verbose_logging }
    }

    /// Returns true if logging should be verbose.
    pub fn is_verbose_logging(&self) -> bool {
        self.verbose_logging
    }
}

impl std::fmt::Display for Verbosity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self.is_verbose_logging() {
            true => "debug",
            _ => "silent",
        };
        write!(f, "{}", s)
    }
}

/// Same strings as `Display`.
impl std::fmt::Debug for Verbosity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self, f)
    }
}

impl std::str::FromStr for Verbosity {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Verbosity {
            verbose_logging: match s {
                "debug" => true,
                _ => false,
            },
        })
    }
}

/// Formats a [`fidl_fuchsia_fonts::TypefaceRequest`], skipping empty fields.
pub struct TypefaceRequestFormatter<'a>(pub &'a fonts::TypefaceRequest);

impl<'a> std::fmt::Debug for TypefaceRequestFormatter<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct(stringify!(TypefaceRequest));

        format_field!(d, self, query, TypefaceQueryFormatter);
        format_field!(d, self, flags);
        format_field!(d, self, cache_miss_policy);

        d.finish()
    }
}

/// Formats a [`fidl_fuchsia_fonts::TypefaceQuery`], skipping empty fields.
pub struct TypefaceQueryFormatter<'a>(pub &'a fonts::TypefaceQuery);

impl<'a> std::fmt::Debug for TypefaceQueryFormatter<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct(stringify!(TypefaceQuery));

        format_field!(d, self, family);
        format_field!(d, self, style, Style2Formatter);
        format_field!(d, self, languages);
        format_field!(d, self, code_points);
        format_field!(d, self, fallback_family);

        d.finish()
    }
}

/// Formats a [`fidl_fuchsia_fonts::Style2`], skipping empty fields.
pub struct Style2Formatter<'a>(pub &'a fonts::Style2);

impl<'a> std::fmt::Debug for Style2Formatter<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct(stringify!(Style2));

        format_field!(d, self, slant);
        format_field!(d, self, weight);
        format_field!(d, self, width);

        d.finish()
    }
}

pub struct TypefaceResponseFormatter<'a>(pub &'a fonts::TypefaceResponse);

impl<'a> std::fmt::Debug for TypefaceResponseFormatter<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct(stringify!(TypefaceResponse));

        format_field!(d, self, buffer);
        format_field!(d, self, buffer_id);
        format_field!(d, self, font_index);

        d.finish()
    }
}
