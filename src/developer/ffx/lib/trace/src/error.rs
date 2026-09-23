// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::convert::Infallible;
use zerocopy::{AlignmentError, ConvertError, SizeError};

#[derive(thiserror::Error, Debug)]
pub enum TraceError {
    #[error("Configuration error: {0}")]
    Config(#[from] ffx_config::ConfigError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Codec error: {0}")]
    Codec(#[from] fidl_codec_pure::Error),

    #[error("Failed to decode object info: {0}")]
    ObjectInfoDecode(
        #[from]
        ConvertError<
            AlignmentError<&'static [u8], zx_types::zx_info_handle_basic_t>,
            SizeError<&'static [u8], zx_types::zx_info_handle_basic_t>,
            Infallible,
        >,
    ),

    #[error(
        "Error: no category group found for {group}, you can add this category locally by calling \
              `ffx config set trace.category_groups.{group} '[\"list\", \"of\", \"categories\"]'`\
              or globally by adding it to data/config.json in the ffx trace plugin."
    )]
    CategoryGroupNotFound { group: String },

    #[error("Error: category \"{name}\" is invalid")]
    InvalidCategoryName { name: String },

    #[error("Error: #{group} contains an invalid category \"{category}\"")]
    InvalidCategoryInGroup { group: String, category: String },

    #[error("all_fidl_json.txt was not found in {path:?}")]
    AllFidlJsonNotFound { path: std::path::PathBuf },

    #[error("No build directory found.")]
    NoBuildDirectory,

    #[error("Format error: {0}")]
    Format(#[from] std::fmt::Error),

    #[error("Unknown method ordinal {ordinal}")]
    UnknownMethodOrdinal { ordinal: u64 },

    #[error("Ambiguous request/response decoding:\nRequest: {request}\nResponse: {response}")]
    AmbiguousDecoding { request: fidl_codec_pure::Value, response: fidl_codec_pure::Value },
}

pub type Result<T> = std::result::Result<T, TraceError>;
