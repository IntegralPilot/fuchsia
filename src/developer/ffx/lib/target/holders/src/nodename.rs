// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use safe_string::{ControlCharError, TermSafe};
use std::ops::Deref;
use target_behavior::{ConnectionBehavior, target_interface};

use fho::{FhoEnvironment, TryFromEnv};

/// Holder struct for the target's Nodename.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodenameHolder(Option<TermSafe>);

impl NodenameHolder {
    /// Returns the nodename as a string slice if present.
    pub fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }

    /// Consumes the holder and returns the inner Option<TermSafe>.
    pub fn into_inner(self) -> Option<TermSafe> {
        self.0
    }
}

impl Deref for NodenameHolder {
    type Target = Option<TermSafe>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<TermSafe> for NodenameHolder {
    fn from(value: TermSafe) -> Self {
        NodenameHolder(Some(value))
    }
}

impl From<Option<TermSafe>> for NodenameHolder {
    fn from(value: Option<TermSafe>) -> Self {
        NodenameHolder(value)
    }
}

impl From<NodenameHolder> for Option<TermSafe> {
    fn from(value: NodenameHolder) -> Self {
        value.0
    }
}

impl TryFrom<&str> for NodenameHolder {
    type Error = ControlCharError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Ok(Self(Some(TermSafe::try_from(value)?)))
    }
}

impl TryFrom<String> for NodenameHolder {
    type Error = ControlCharError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(value.as_str())
    }
}

impl TryFrom<Option<String>> for NodenameHolder {
    type Error = ControlCharError;

    fn try_from(value: Option<String>) -> Result<Self, Self::Error> {
        Ok(Self(value.map(TermSafe::try_from).transpose()?))
    }
}

#[async_trait(?Send)]
impl TryFromEnv for NodenameHolder {
    type Error = ffx_command_error::Error;
    async fn try_from_env(env: &FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        let target_env = target_interface(env);
        let behavior = target_env.init_connection_behavior(env.environment_context()).await?;
        let ConnectionBehavior::Direct(ref dc) = *behavior;
        let identity = dc
            .resolution()
            .await
            .map_err(|e| e.into_command_error())?
            .identify(&env.environment_context())
            .await
            .map_err(|e| e.into_command_error())?
            .nodename;
        NodenameHolder::try_from(identity)
            .map_err(|e| ffx_command_error::user_error!("Target nodename is invalid: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[test]
    fn test_nodename_holder_valid() {
        let holder = NodenameHolder::try_from("fuchsia-1234-5678").unwrap();
        assert_eq!(holder.as_deref(), Some("fuchsia-1234-5678"));
        assert_eq!(*holder, Some(TermSafe::try_from("fuchsia-1234-5678").unwrap()));

        let from_opt: NodenameHolder = Option::<TermSafe>::None.into();
        assert_eq!(from_opt.as_deref(), None);

        let from_opt_str: NodenameHolder =
            NodenameHolder::try_from(Some("valid-target".to_string())).unwrap();
        assert_eq!(from_opt_str.as_deref(), Some("valid-target"));

        let from_opt_none: NodenameHolder =
            NodenameHolder::try_from(Option::<String>::None).unwrap();
        assert_eq!(from_opt_none.as_deref(), None);
    }

    #[test]
    fn test_nodename_holder_rejects_ansi_escape() {
        assert_matches!(
            NodenameHolder::try_from("target\x1b[31m_evil"),
            Err(ControlCharError { byte_index: 6, character: '\x1b' })
        );
        assert_matches!(
            NodenameHolder::try_from(Some("target\x1b[0m".to_string())),
            Err(ControlCharError { byte_index: 6, character: '\x1b' })
        );
        assert_matches!(
            NodenameHolder::try_from("target\nname"),
            Err(ControlCharError { byte_index: 6, character: '\n' })
        );
    }
}
