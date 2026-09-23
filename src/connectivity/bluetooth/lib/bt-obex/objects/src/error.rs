// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use thiserror::Error;
use xml::reader::Error as XmlReaderError;
use xml::writer::Error as XmlWriterError;

/// The error types for packet parsing.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// Error encountered when trying to write XML.
    #[error("Error writing XML data: {:?}", .0)]
    WriteXml(#[from] XmlWriterError),

    /// Error encountered when trying to read XML.
    #[error("Error reading from XML data: {:?}", .0)]
    ReadXml(#[from] XmlReaderError),

    /// Error returned when invalid data was encountered.
    #[error("Invalid data: {:?}", .0)]
    InvalidData(String),

    /// Error returned when required data is missing.
    #[error("Missing data: {:?}", .0)]
    MissingData(String),

    /// Error returned when duplicate data was encountered.
    #[error("Invalid data: {:?}", .0)]
    DuplicateData(String),

    /// Error returned when unsupported version of the object was encountered.
    #[error("Unsupported object version")]
    UnsupportedVersion,
}

impl Error {
    pub fn invalid_data(msg: impl ToString) -> Self {
        Self::InvalidData(msg.to_string())
    }

    pub fn missing_data(msg: impl ToString) -> Self {
        Self::MissingData(msg.to_string())
    }
}
