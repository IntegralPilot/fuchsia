// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod json_writer;
mod test_buffer;
mod tool_io;
mod writer;

pub use json_writer::{JsonWriter, format_output};
pub use test_buffer::{TestBuffer, TestBuffers};
pub use tool_io::ToolIO;
pub use writer::Writer;

/// Enum indicating output formatting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    JsonPretty,
    Raw,
}

impl std::str::FromStr for Format {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_ref() {
            "json-pretty" => Ok(Format::JsonPretty),
            "json" | "j" => Ok(Format::Json),
            "raw" => Ok(Format::Raw),
            other => Err(Error::InvalidFormat(other.into())),
        }
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Error while presenting output")]
pub enum Error {
    #[error("Error on the underlying IO stream")]
    Io(#[from] std::io::Error),
    #[error("Error formatting JSON output")]
    Json(#[from] serde_json::Error),
    #[error("Error parsing utf8 from buffer")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("`{0}` is not a valid machine format")]
    InvalidFormat(String),
    #[error("Schema validation failed: {0}")]
    SchemaFailure(String),
}

impl Error {
    pub fn is_broken_pipe(&self) -> bool {
        match self {
            Error::Io(e) => e.kind() == std::io::ErrorKind::BrokenPipe,
            _ => false,
        }
    }
}

pub type Result<O, E = Error> = std::result::Result<O, E>;

impl From<Error> for ffx_command_error::Error {
    fn from(error: Error) -> Self {
        use Error::*;
        use ffx_command_error::Error::*;
        match error {
            error @ (Io(_) | Json(_) | Utf8(_) | SchemaFailure(_)) => Unexpected(error.into()),
            error @ InvalidFormat(_) => User(error.into()),
        }
    }
}
