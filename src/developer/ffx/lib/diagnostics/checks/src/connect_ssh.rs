// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use discovery::TargetHandle;
use ffx_config::EnvironmentContext;
use ffx_diagnostics::{Check, CheckFut, Notifier};
use ffx_diagnostics_analytics::{PointOfFailure, ResultExt};
use ffx_target::connection::ConnectionError;
use ffx_target::ssh_connector::SshConnector;
use ffx_target::{Connection, TargetConnection, TargetConnectionError, TargetConnector};
use fuchsia_async::TimeoutExt;
use std::time::Duration;
use termio::Colors;

const DEFAULT_SSH_TIMEOUT: Duration = Duration::from_secs(30);

pub trait SshConnectorProvider {
    async fn connector_for_target<N>(
        &self,
        ctx: EnvironmentContext,
        handle: TargetHandle,
        notifier: &mut N,
    ) -> anyhow::Result<impl TargetConnector + 'static>
    where
        N: Notifier + Sized;
}

pub(crate) struct DefaultSshConnectorProvider;

impl SshConnectorProvider for DefaultSshConnectorProvider {
    async fn connector_for_target<N>(
        &self,
        ctx: EnvironmentContext,
        handle: TargetHandle,
        notifier: &mut N,
    ) -> anyhow::Result<impl TargetConnector + 'static>
    where
        N: Notifier + Sized,
    {
        let resolution = ffx_target::Resolution::from_target_handle(handle.clone())
            .or_analytics(PointOfFailure::TargetHandleInBadState { state: handle.state.clone() })
            .await?;
        let addr = resolution
            .addr()
            .or_analytics(PointOfFailure::TargetDoesntSupportNetworking {
                state: handle.state.clone(),
            })
            .await?;
        let addr = netext::ScopedSocketAddr::from_socket_addr(addr)
            .or_analytics(PointOfFailure::TargetAddressBadScope)
            .await?;
        // Note the error here is not checked because the original source only ever returns `Ok()`.
        let connector = ConnectorHolder(SshConnector::new(addr, &ctx)?);
        let fdomain_command = connector
            .0
            .fdomain_command()
            .await
            .or_analytics(PointOfFailure::UnableToBuildFDomainCommand)
            .await?;
        notifier.info(format!("Executing the command: `{}`", fdomain_command))?;
        Ok(connector)
    }
}

pub struct ConnectSsh<'a, N, P> {
    ctx: &'a EnvironmentContext,
    conn_provider: &'a P,
    _w: std::marker::PhantomData<N>,
}

impl<'a, N, P> ConnectSsh<'a, N, P> {
    pub fn new(ctx: &'a EnvironmentContext, conn_provider: &'a P) -> Self {
        Self { ctx, _w: Default::default(), conn_provider }
    }
}

#[derive(Debug)]
struct ConnectorHolder(SshConnector);

impl TargetConnector for ConnectorHolder {
    const CONNECTION_TYPE: &'static str = "ssh";

    async fn connect(&mut self) -> Result<TargetConnection, TargetConnectionError> {
        Ok(TargetConnection::FDomain(self.0.connect_via_fdomain().await?))
    }
}

impl<N, P> Check for ConnectSsh<'_, N, P>
where
    N: Notifier + Sized,
    P: SshConnectorProvider + Sized,
{
    type Input = TargetHandle;
    type Output = Connection;
    type Notifier = N;

    fn write_preamble(
        &self,
        input: &Self::Input,
        notifier: &mut Self::Notifier,
    ) -> anyhow::Result<()> {
        let state_str = ffx_diagnostics_formatting::format_target_state(&input.state);
        if let Some(name) = &input.node_name {
            let colors = Colors::current();
            let safe_name = safe_string::TermSafe::from_str_escaped(name);
            notifier.info(format!(
                "Attempting to connect ssh to device node: \"{}{}{}\" {state_str}",
                colors.green, safe_name, colors.reset
            ))
        } else {
            notifier.info(format!("Attempting to connect ssh to device {state_str}"))
        }
    }

    fn on_success(
        &self,
        _output: &Self::Output,
        notifier: &mut Self::Notifier,
    ) -> anyhow::Result<()> {
        notifier.on_success("Connected")
    }

    fn check<'a>(
        &'a mut self,
        input: Self::Input,
        notifier: &'a mut Self::Notifier,
    ) -> CheckFut<'a, Self::Output> {
        Box::pin(async {
            let timeout = match self.ctx.get::<Option<u64>, _>(ffx_ssh::ssh::CONNECT_TIMEOUT_CONFIG)
            {
                Ok(Some(0)) => {
                    notifier.info("SSH connect timeout is disabled (0 seconds).")?;
                    Duration::ZERO
                }
                Ok(Some(secs)) => {
                    notifier.info(format!(
                        "Using configured SSH connect timeout of {} seconds.",
                        secs
                    ))?;
                    Duration::from_secs(secs)
                }
                Ok(None) => {
                    notifier.info(format!(
                        "No SSH connect timeout configured; using default of {} seconds.",
                        DEFAULT_SSH_TIMEOUT.as_secs()
                    ))?;
                    DEFAULT_SSH_TIMEOUT
                }
                Err(e) => {
                    log::warn!(
                        "Failed to read '{}': {e}. Using default timeout of {} seconds.",
                        ffx_ssh::ssh::CONNECT_TIMEOUT_CONFIG,
                        DEFAULT_SSH_TIMEOUT.as_secs()
                    );
                    notifier.info(format!(
                        "Failed to read '{}': {e}. Using default SSH connect timeout of {} seconds.",
                        ffx_ssh::ssh::CONNECT_TIMEOUT_CONFIG,
                        DEFAULT_SSH_TIMEOUT.as_secs()
                    ))?;
                    DEFAULT_SSH_TIMEOUT
                }
            };
            // All analytics/errors are handled inside this function.
            let connect_fut = async {
                let connector = self
                    .conn_provider
                    .connector_for_target(self.ctx.clone(), input.clone(), notifier)
                    .await?;
                Connection::new(connector).await
            };
            let conn_res = if timeout.is_zero() {
                connect_fut.await
            } else {
                connect_fut
                    .on_timeout(timeout, || {
                        Err(ConnectionError::ConnectionStartError(
                            "ssh".to_string(),
                            format!("connection timed out after {} seconds", timeout.as_secs()),
                        ))
                    })
                    .await
            };
            match conn_res {
                Ok(res) => Ok(res),
                Err(e) => {
                    ffx_diagnostics_analytics::mark_point_of_failure(
                        ffx_target::analytics::PointOfFailure::SshConnectionFailed {
                            state: input.state,
                            reason: &e,
                        },
                    )
                    .await;
                    Err(anyhow::anyhow!(
                        "\nUnable to connect to ssh. Consider running the above `ssh` command and evaluating the output.\nIn addition, consult https://fuchsia.dev/fuchsia-src/development/tools/ffx/workflows/network-connectivity/ssh-daemon\nUnderlying error: {}",
                        match e {
                            ConnectionError::ConnectionStartError(_, s) => anyhow::anyhow!("{}", s),
                            _ => e.into(),
                        }
                    ))
                }
            }
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use discovery::{TargetHandle, TargetState};
    use ffx_target::FDomainConnection;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    #[derive(Debug)]
    struct MockSshConnectorProvider<R> {
        res: RefCell<Option<anyhow::Result<R>>>,
    }

    impl<R> MockSshConnectorProvider<R> {
        fn with_res(res: anyhow::Result<R>) -> Self {
            Self { res: RefCell::new(Some(res)) }
        }
    }

    impl<R> SshConnectorProvider for MockSshConnectorProvider<R>
    where
        R: TargetConnector + 'static,
    {
        async fn connector_for_target<N>(
            &self,
            _ctx: EnvironmentContext,
            _handle: TargetHandle,
            _notifier: &mut N,
        ) -> anyhow::Result<impl TargetConnector + 'static>
        where
            N: Notifier + Sized,
        {
            self.res.borrow_mut().take().expect("called `connector_for_target` once").into()
        }
    }

    struct MockConnector {
        results: RefCell<VecDeque<Result<TargetConnection, TargetConnectionError>>>,
    }

    impl std::fmt::Debug for MockConnector {
        fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(fmt, "MockConnector {{}}")
        }
    }

    impl MockConnector {
        /// Delivers the results one at a time in order declared.
        fn with_results(
            res: impl Into<Vec<Result<TargetConnection, TargetConnectionError>>>,
        ) -> Self {
            Self { results: RefCell::new(res.into().into()) }
        }
    }

    impl TargetConnector for MockConnector {
        const CONNECTION_TYPE: &'static str = "mock";

        async fn connect(&mut self) -> Result<TargetConnection, TargetConnectionError> {
            self.results.borrow_mut().pop_front().expect("should have more mocked results left")
        }
    }

    #[derive(Debug)]
    struct HangingConnector;

    impl TargetConnector for HangingConnector {
        const CONNECTION_TYPE: &'static str = "hanging";

        async fn connect(&mut self) -> Result<TargetConnection, TargetConnectionError> {
            futures::future::pending().await
        }
    }

    #[fuchsia::test]
    async fn test_connect_ssh() {
        let env = ffx_config::test_env().build().unwrap();
        let m = MockSshConnectorProvider::with_res(Ok(MockConnector::with_results([Ok(
            TargetConnection::FDomain(FDomainConnection::invalid()),
        )])));
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let mut check = ConnectSsh::new(&env.context, &m);
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let res = check.check(handle, &mut notifier).await;
        assert!(res.is_ok());
        let output: String = notifier.into();
        assert!(output.contains("No SSH connect timeout configured; using default of 30 seconds."));
    }

    #[fuchsia::test]
    async fn test_connect_ssh_timeout() {
        let env = ffx_config::test_env()
            .runtime_config(ffx_ssh::ssh::CONNECT_TIMEOUT_CONFIG, "1")
            .build()
            .unwrap();
        let m = MockSshConnectorProvider::with_res(Ok(HangingConnector));
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let mut check = ConnectSsh::new(&env.context, &m);
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let res = check.check(handle, &mut notifier).await;
        assert!(res.is_err());
        let err = res.unwrap_err().to_string();
        assert!(err.contains("timed out"), "Expected timeout error, got: {err}");
        let output: String = notifier.into();
        assert!(output.contains("Using configured SSH connect timeout of 1 seconds."));
    }

    #[fuchsia::test]
    async fn test_connect_ssh_disabled_timeout() {
        let env = ffx_config::test_env()
            .runtime_config(ffx_ssh::ssh::CONNECT_TIMEOUT_CONFIG, "0")
            .build()
            .unwrap();
        let m = MockSshConnectorProvider::with_res(Ok(MockConnector::with_results([Ok(
            TargetConnection::FDomain(FDomainConnection::invalid()),
        )])));
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let mut check = ConnectSsh::new(&env.context, &m);
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let res = check.check(handle, &mut notifier).await;
        assert!(res.is_ok());
        let output: String = notifier.into();
        assert!(output.contains("SSH connect timeout is disabled (0 seconds)."));
    }

    #[fuchsia::test]
    async fn test_connect_ssh_failures() {
        let env = ffx_config::test_env().build().unwrap();
        let m = MockSshConnectorProvider::<MockConnector>::with_res(Err(anyhow::anyhow!(
            "Couldn't get a connector for some reason"
        )));
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let mut check = ConnectSsh::new(&env.context, &m);
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let res = check.check(handle, &mut notifier).await;
        assert!(res.is_err());
    }

    #[fuchsia::test]
    async fn test_connect_ssh_failures_connector_fails() {
        let env = ffx_config::test_env().build().unwrap();
        // TODO(b/425474866): This should result in a check where we can see that the connection
        // failed multiple times albeit non-fatally.
        let mock_connector = MockConnector::with_results([
            Err(TargetConnectionError::NonFatal(anyhow::anyhow!("Test non-fatal error"))),
            Err(TargetConnectionError::NonFatal(anyhow::anyhow!(
                "Test non-fatal error two: electric boogaloo"
            ))),
            Ok(TargetConnection::FDomain(FDomainConnection::invalid())),
        ]);
        let m = MockSshConnectorProvider::with_res(Ok(mock_connector));
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let mut check = ConnectSsh::new(&env.context, &m);
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let res = check.check(handle, &mut notifier).await;
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_connect_ssh_escapes_control_characters() {
        let env = ffx_config::test_env().build().unwrap();
        let m = MockSshConnectorProvider::with_res(Ok(MockConnector::with_results([Ok(
            TargetConnection::FDomain(FDomainConnection::invalid()),
        )])));
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let check = ConnectSsh::new(&env.context, &m);
        let handle = TargetHandle {
            node_name: Some("evil\x1b[31m_ssh_node\r\n".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let res = check.check_with_notifier(handle, &mut notifier).await;
        assert!(res.is_ok());

        let output: String = notifier.into();
        assert!(!output.contains('\x1b'));
        assert!(!output.contains('\r'));
        assert!(output.contains("evil\\u{1b}[31m_ssh_node\\r\\n"));
    }
}
