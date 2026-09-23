// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use async_trait::async_trait;
use ffx_command_error::{Error, Result};
use fho::{FhoEnvironment, TryFromEnv};
use target_behavior::{
    ConnectionBehavior, DirectConnector, FhoTargetEnvironment, target_interface,
};

/// A connector lets a tool make multiple attempts to connect to an object. It
/// retains the environment in the tool body to allow this.
#[derive(Clone)]
pub struct Connector<T: TryFromEnv> {
    env: FhoEnvironment,
    target_env: FhoTargetEnvironment,
    _connects_to: std::marker::PhantomData<T>,
}

impl<T> Connector<T>
where
    T: TryFromEnv<Error = ffx_command_error::Error>,
{
    /// Try to get a `T` from the environment. Will wait for the target to
    /// appear if it is non-responsive. If that occurs, `log_target_wait` will
    /// be called prior to waiting.
    pub async fn try_connect(
        &self,
        mut log_target_wait: impl FnMut(&Option<String>, &Option<Error>) -> Result<()>,
    ) -> Result<T> {
        let behavior =
            self.target_env.init_connection_behavior(self.env.environment_context()).await?;
        // TODO(b/540442481): Clean up naming (e.g. "Direct", "DirectConnector") in a follow-up CL now that direct connection is the only mechanism.
        let ConnectionBehavior::Direct(dc) = &*behavior;
        direct_connector_try_connect::<T>(&self.env, dc, &mut log_target_wait).await
    }

    /// Try to get a `T` from the environment with an indefinite discovery timeout.
    /// Will wait indefinitely for the target to appear.
    pub async fn try_connect_indefinitely(
        &self,
        mut log_target_wait: impl FnMut(&Option<String>, &Option<Error>) -> Result<()>,
    ) -> Result<T> {
        let behavior =
            self.target_env.init_connection_behavior_indef(self.env.environment_context()).await?;
        match *behavior {
            ConnectionBehavior::Direct(ref dc) => {
                direct_connector_try_connect::<T>(&self.env, dc, &mut log_target_wait).await
            }
        }
    }
}

#[async_trait(?Send)]
impl<T> TryFromEnv for Connector<T>
where
    T: TryFromEnv<Error = ffx_command_error::Error>,
{
    type Error = ffx_command_error::Error;
    async fn try_from_env(env: &FhoEnvironment) -> Result<Self, Self::Error> {
        let target_env = target_interface(env);
        Ok(Connector { env: env.clone(), target_env, _connects_to: Default::default() })
    }
}

async fn direct_connector_try_connect<T>(
    env: &FhoEnvironment,
    dc: &DirectConnector,
    log_target_wait: &mut impl FnMut(&Option<String>, &Option<Error>) -> Result<()>,
) -> Result<T>
where
    T: TryFromEnv<Error = ffx_command_error::Error>,
{
    loop {
        let target_spec = {
            let resolution = dc.resolution().await.map_err(|e| e.into_command_error())?;
            let _ = resolution
                .get_connection(env.environment_context())
                .await
                .map_err(|e| e.into_command_error())?;
            resolution.target_spec()
        };
        return match T::try_from_env(env).await {
            Err(conn_error) => {
                let e = conn_error.downcast_non_fatal()?;
                log::debug!("error when trying to connect using TryFromEnv: {e}");
                log_target_wait(&Some(target_spec), &Some(Error::User(e)))?;
                continue;
            }
            Ok(res) => Ok(res),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffx_config::test_env;
    use target_behavior::ConnectionBehavior;

    struct DummyHolder;

    #[async_trait(?Send)]
    impl TryFromEnv for DummyHolder {
        type Error = ffx_command_error::Error;
        async fn try_from_env(_env: &FhoEnvironment) -> Result<Self, Self::Error> {
            Ok(DummyHolder)
        }
    }

    // Tests that `Connector<T>` can successfully connect via `try_connect` (using default
    // discovery timeout) and `try_connect_indefinitely` (using indefinite discovery timeout)
    // without errors when a valid direct connection behavior and target environment are configured.
    #[fuchsia::test]
    async fn test_connector_try_connect_and_try_connect_indefinitely() {
        let env = test_env().build().unwrap();
        let fho_env = FhoEnvironment::new_with_args(&env.context, &["some", "test"]);
        let target_env = target_behavior::target_interface(&fho_env);

        let resolution = ffx_target::Resolution::mock(|| unreachable!());
        #[derive(Debug)]
        struct PlaceholderConnector;
        impl ffx_target::TargetConnector for PlaceholderConnector {
            const CONNECTION_TYPE: &'static str = "placeholder";
            async fn connect(
                &mut self,
            ) -> Result<ffx_target::TargetConnection, ffx_target::TargetConnectionError>
            {
                Ok(ffx_target::TargetConnection::FDomain(ffx_target::FDomainConnection::invalid()))
            }
        }
        let conn = ffx_target::Connection::new(PlaceholderConnector).await.unwrap();
        resolution.set_connection_for_test(Some(conn)).await;
        let behavior = ConnectionBehavior::fake_direct_connector(resolution);
        target_env.set_behavior_for_test(behavior);

        let connector =
            Connector::<DummyHolder>::try_from_env(&fho_env).await.expect("create connector");

        let res = connector.try_connect(|_, _| Ok(())).await;
        assert!(res.is_ok());

        let res_indef = connector.try_connect_indefinitely(|_, _| Ok(())).await;
        assert!(res_indef.is_ok());
    }
}
