// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_paver as paver;

/// We do not support the Recovery bootslot in verification and commits!
/// Wrapper type so that it's very clear to humans and compilers.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigurationWithoutRecovery {
    A,
    B,
}

impl From<&ConfigurationWithoutRecovery> for paver::Configuration {
    fn from(config: &ConfigurationWithoutRecovery) -> Self {
        match *config {
            ConfigurationWithoutRecovery::A => Self::A,
            ConfigurationWithoutRecovery::B => Self::B,
        }
    }
}

impl ConfigurationWithoutRecovery {
    pub fn to_alternate(&self) -> &Self {
        match *self {
            Self::A => &Self::B,
            Self::B => &Self::A,
        }
    }
}

#[cfg(test)]
impl TryFrom<&paver::Configuration> for ConfigurationWithoutRecovery {
    type Error = ();
    fn try_from(config: &paver::Configuration) -> Result<Self, Self::Error> {
        match *config {
            paver::Configuration::A => Ok(Self::A),
            paver::Configuration::B => Ok(Self::B),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_alternate() {
        assert_eq!(
            ConfigurationWithoutRecovery::A.to_alternate(),
            &ConfigurationWithoutRecovery::B
        );
        assert_eq!(
            ConfigurationWithoutRecovery::B.to_alternate(),
            &ConfigurationWithoutRecovery::A
        );
    }

    #[test]
    fn fidl_conversion() {
        assert_eq!(
            paver::Configuration::from(&ConfigurationWithoutRecovery::A),
            paver::Configuration::A
        );
        assert_eq!(
            paver::Configuration::from(&ConfigurationWithoutRecovery::B),
            paver::Configuration::B
        );
    }
}
