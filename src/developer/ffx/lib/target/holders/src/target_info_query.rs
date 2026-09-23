// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use async_trait::async_trait;
use discovery::query::TargetInfoQuery;

use fho::{FhoEnvironment, TryFromEnv};
use std::ops::Deref;

#[derive(Clone, Debug)]
pub struct TargetInfoQueryHolder(TargetInfoQuery);

impl Deref for TargetInfoQueryHolder {
    type Target = TargetInfoQuery;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<discovery::query::TargetInfoQuery> for TargetInfoQueryHolder {
    fn from(value: TargetInfoQuery) -> Self {
        TargetInfoQueryHolder(value)
    }
}

#[async_trait(?Send)]
impl TryFromEnv for TargetInfoQueryHolder {
    type Error = ffx_command_error::Error;
    async fn try_from_env(env: &FhoEnvironment) -> std::result::Result<Self, Self::Error> {
        let env_context = env.environment_context();
        let target_spec = ffx_target::get_target_specifier(&env_context)?;
        let tiq = TargetInfoQuery::try_from(target_spec).map_err(|e| {
            ffx_command_error::user_error!("Invalid default target specifier: {}", e)
        })?;

        Ok(Self::from(tiq))
    }
}
