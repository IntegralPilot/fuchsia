// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use hex::FromHex;
use std::fmt::Display;
use std::str::FromStr;
use thiserror::Error;

#[cfg(feature = "serde")]
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The length of an instance ID, in bytes.
/// An instance ID is 256 bits, normally encoded as a 64-character hex string.
pub const INSTANCE_ID_LEN: usize = 32;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InstanceId([u8; INSTANCE_ID_LEN]);

impl InstanceId {
    /// Returns a random instance ID.
    pub fn new_random(rng: &mut impl rand::Rng) -> Self {
        let mut bytes: [u8; INSTANCE_ID_LEN] = [0; INSTANCE_ID_LEN];
        rng.fill_bytes(&mut bytes);
        InstanceId(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Display for InstanceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        for byte in self.0 {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

#[cfg_attr(feature = "serde", derive(Deserialize, Serialize), serde(rename_all = "snake_case"))]
#[derive(Error, Clone, Debug, PartialEq)]
pub enum InstanceIdError {
    #[error("invalid length; must be 64 characters")]
    InvalidLength,
    #[error("string contains invalid hex character; must be [0-9a-f]")]
    InvalidHexCharacter,
}

impl FromStr for InstanceId {
    type Err = InstanceIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Must have 64 characters.
        // 256 bits in base16 = 64 chars (1 char to represent 4 bits)
        if s.len() != 64 {
            return Err(InstanceIdError::InvalidLength);
        }
        // Must be a lower-cased hex string.
        if !s.chars().all(|ch| (ch.is_numeric() || ch.is_lowercase()) && ch.is_digit(16)) {
            return Err(InstanceIdError::InvalidHexCharacter);
        }
        // The following unwrap is safe because the validation above covers all FromHexError cases.
        let bytes = <[u8; INSTANCE_ID_LEN]>::from_hex(&s).unwrap();
        Ok(InstanceId(bytes))
    }
}

#[cfg(feature = "serde")]
impl Serialize for InstanceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for InstanceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?.parse().map_err(|err| {
            let instance_id = InstanceId::new_random(&mut rand::rng());
            serde::de::Error::custom(format!(
                "Invalid instance ID: {}\n\nHere is a valid, randomly generated ID: {}\n",
                err, instance_id
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rand::{Rng as _, SeedableRng as _};
    use test_case::test_case;

    #[test]
    fn to_string() {
        let id_str = "8c90d44863ff67586cf6961081feba4f760decab8bbbee376a3bfbc77b351280";
        let id = id_str.parse::<InstanceId>().unwrap();
        assert_eq!(id.to_string(), id_str);
    }

    proptest! {
        #[test]
        fn parse(id in "[a-f0-9]{64}") {
            prop_assert!(id.parse::<InstanceId>().is_ok());
        }
    }

    #[test_case("8c90d44863ff67586cf6961081feba4f760decab8bbbee376a3bfbc77b351280b351280"; "too long")]
    #[test_case("8c90d44863ff67586cf6961081feba4f760decab8bbbee376a"; "too short")]
    #[test_case("8C90D44863FF67586CF6961081FEBA4F760DECAB8BBBEE376A3BFBC77B351280"; "upper case chars are invalid")]
    #[test_case("8;90d44863ff67586cf6961081feba4f760decab8bbbee376a3bfbc77b351280"; "hex chars only")]
    fn parse_invalid(id: &str) {
        assert!(id.parse::<InstanceId>().is_err());
    }

    #[test]
    fn new_random_is_unique() {
        let seed = rand::rng().next_u64();
        println!("using seed {}", seed);
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let mut prev_id = InstanceId::new_random(&mut rng);
        for _i in 0..40 {
            let id = InstanceId::new_random(&mut rng);
            assert!(prev_id != id);
            prev_id = id;
        }
    }

    #[test]
    fn parse_new_random() {
        let seed = rand::rng().next_u64();
        println!("using seed {}", seed);
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        for _i in 0..40 {
            let id_str = InstanceId::new_random(&mut rng).to_string();
            assert!(id_str.parse::<InstanceId>().is_ok());
        }
    }
}
