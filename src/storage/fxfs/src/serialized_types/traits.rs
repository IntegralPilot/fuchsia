// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::serialized_types::DEFAULT_MAX_SERIALIZED_RECORD_SIZE;
use crate::serialized_types::types::LATEST_VERSION;
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use fprint::TypeFingerprint;
use serde::{Deserialize, Serialize};

/// [Version] are themselves serializable both alone and as part
/// of other [Versioned] structures.
/// (For obvious recursive reasons, this structure can never be [Versioned] itself.)
#[derive(
    Debug, Default, Copy, Clone, Eq, PartialEq, PartialOrd, Serialize, Deserialize, TypeFingerprint,
)]
pub struct Version {
    /// Major version indicates structural layout/encoding changes.
    /// Note that this is encoded as a u24.
    pub major: u32,
    /// Minor version indicates forwards compatible changes.
    /// e.g. The addition of a layer-file index, bloom filters, file attributes or posix
    /// features where reversion to a previous minor will simply lead to loss of these features.
    pub minor: u8,
}
impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}
impl Version {
    pub fn deserialize_from<R: ?Sized>(reader: &mut R) -> anyhow::Result<Self>
    where
        R: std::io::Read,
    {
        Ok(Version { major: reader.read_u24::<LittleEndian>()?, minor: reader.read_u8()? })
    }
    pub fn serialize_into<W>(&self, writer: &mut W) -> anyhow::Result<()>
    where
        W: std::io::Write,
    {
        writer.write_u24::<LittleEndian>(self.major)?;
        writer.write_u8(self.minor)?;
        Ok(())
    }
}

/// This trait is assigned to all versions of a versioned type and is the only means of
/// serialization/deserialization we use.
///
/// It also allows versions to serialize/deserialize themselves differently on a per-version basis.
/// Doing this here enforces consistency at a given filesystem version.
pub trait Versioned: Serialize + for<'de> Deserialize<'de> {
    /// Maximum size of a serialized version of this type. Serialization and deserialization will
    /// fail if the limit is exceeded. If None, there is no limit. None is preferred over u64::MAX
    /// because serializing and deserializing with a limit has runtime overhead.
    fn max_serialized_size() -> Option<u64> {
        Some(DEFAULT_MAX_SERIALIZED_RECORD_SIZE)
    }

    fn deserialize_from<R: ?Sized>(reader: &mut R, _version: Version) -> anyhow::Result<Self>
    where
        R: std::io::Read,
        for<'de> Self: serde::Deserialize<'de>,
    {
        use bincode::Options;
        let options = bincode::DefaultOptions::new().allow_trailing_bytes();
        let result = if let Some(limit) = Self::max_serialized_size() {
            options.with_limit(limit).deserialize_from(reader)
        } else {
            options.deserialize_from(reader)
        };
        result.map_err(map_bincode_error)
    }
    fn serialize_into<W>(&self, writer: &mut W) -> anyhow::Result<()>
    where
        W: std::io::Write,
        Self: serde::Serialize,
    {
        use bincode::Options;
        let options = bincode::DefaultOptions::new().allow_trailing_bytes();
        let result = if let Some(limit) = Self::max_serialized_size() {
            options.with_limit(limit).serialize_into(writer, self)
        } else {
            options.serialize_into(writer, self)
        };
        result.map_err(map_bincode_error)
    }
}

/// This trait is only assigned to the latest version of a type and allows the type to deserialize
/// any older versions and upgrade them to the latest format.
pub trait VersionedLatest: Versioned + TypeFingerprint {
    /// Deserializes from a given version format and upgrades to the latest version.
    fn deserialize_from_version<R>(reader: &mut R, version: Version) -> anyhow::Result<Self>
    where
        R: std::io::Read,
        Self: Sized;

    /// Like `deserialize_from_version` but reads Version from reader first, then uses it to
    /// deserialize self.
    fn deserialize_with_version<R>(reader: &mut R) -> anyhow::Result<(Self, Version)>
    where
        R: std::io::Read,
        Self: Sized,
    {
        let version = Version::deserialize_from(reader)?;
        Ok((Self::deserialize_from_version(reader, version)?, version))
    }
    /// Like `serialize_into` but serialized Version first, then self.
    fn serialize_with_version<W>(&self, writer: &mut W) -> anyhow::Result<()>
    where
        W: std::io::Write,
        Self: Sized,
    {
        LATEST_VERSION.serialize_into(writer)?;
        self.serialize_into(writer)
    }
}

fn map_bincode_error(e: Box<bincode::ErrorKind>) -> anyhow::Error {
    // Strip bincode wrapping. anyhow can take std::io::Error.
    if let bincode::ErrorKind::Io(io) = *e { io.into() } else { e.into() }
}
