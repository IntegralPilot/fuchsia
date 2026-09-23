// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

pub mod folder_listing;

/// The error type used throughout this library.
mod error;
pub use error::Error as ObexObjectError;

/// A builder type can build a Document Type object into raw bytes of encoded data.
pub trait Builder {
    type Error;

    // Returns the MIME type of the raw bytes of data.
    fn mime_type(&self) -> String;

    /// Builds self into raw bytes of the specific Document Type.
    fn build<W: std::io::Write>(&self, buf: W) -> ::core::result::Result<(), Self::Error>;
}

/// An parser type can parse objects from raw bytes of encoded data.
pub trait Parser: ::core::marker::Sized {
    type Error;

    /// Parses from raw bytes of a specific Document Type into specific object, or returns an error.
    fn parse<R: std::io::prelude::Read>(buf: R) -> Result<Self, Self::Error>;
}
