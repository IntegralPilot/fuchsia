// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use discovery::TargetHandle;
use ffx_config::EnvironmentContext;
use ffx_diagnostics::{Check, CheckFut, Notifier};
use ffx_diagnostics_analytics::{PointOfFailure, ResultExt};
use ffx_ssh::keys::{MatchingKeysInfo, SshKey};
use std::collections::HashSet;

pub trait SshKeyVerifier {
    /// Finds and verifies SSH keys from the device. Returns an error if no keys were found (the
    /// hashset should guarantee that at least one key has been found if returning `Ok()`).
    async fn verify_keys<N>(
        &self,
        context: &EnvironmentContext,
        handle: &TargetHandle,
        notifier: &mut N,
    ) -> anyhow::Result<HashSet<SshKey>>
    where
        N: Notifier + Sized;
}

pub(crate) struct DefaultKeyVerifier;

impl SshKeyVerifier for DefaultKeyVerifier {
    async fn verify_keys<N>(
        &self,
        context: &EnvironmentContext,
        handle: &TargetHandle,
        notifier: &mut N,
    ) -> anyhow::Result<HashSet<SshKey>>
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
        let MatchingKeysInfo { keys, dirs_searched, io_errors } =
            ffx_ssh::keys::find_matching_ssh_keys(context, addr).await?;
        notifier.info(format!(
            "Searched for ssh keys in ssh-agent and in [{}]",
            dirs_searched
                .into_iter()
                .map(|d| format!("{}", d.display()))
                .collect::<Vec<_>>()
                .join(", ")
        ))?;
        if !io_errors.is_empty() {
            notifier.info(format!(
                "Unable to read the following files/dirs: [{}]",
                io_errors
                    .into_iter()
                    .map(|(path, err)| format!("{}: {}", path.display(), err))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))?;
        }
        Ok(keys)
    }
}

pub struct VerifySshKeys<'a, N, P> {
    _n: std::marker::PhantomData<N>,
    verifier: &'a P,
    context: &'a EnvironmentContext,
    found_keys: Option<HashSet<SshKey>>,
}

impl<'a, N, P> VerifySshKeys<'a, N, P> {
    pub fn new(context: &'a EnvironmentContext, verifier: &'a P) -> Self {
        Self { _n: Default::default(), context, found_keys: None, verifier }
    }
}

impl<N, P> Check for VerifySshKeys<'_, N, P>
where
    N: Notifier + Sized,
    P: SshKeyVerifier + Sized,
{
    type Input = TargetHandle;
    type Output = TargetHandle;
    type Notifier = N;

    fn on_success(
        &self,
        _output: &Self::Output,
        notifier: &mut Self::Notifier,
    ) -> anyhow::Result<()> {
        if let Some(files) = self.found_keys.as_ref() {
            let files = files.iter().map(|k| &k.sources).fold(Vec::new(), |mut acc, x| {
                acc.extend(x.iter());
                acc
            });
            notifier.on_success(format!(
                "Found local ssh keys matching those expected on the device: {:?}",
                files
            ))?;
        }
        Ok(())
    }

    fn check<'a>(
        &'a mut self,
        input: Self::Input,
        notifier: &'a mut Self::Notifier,
    ) -> CheckFut<'a, Self::Output> {
        Box::pin(async {
            self.found_keys = match self.verifier.verify_keys(self.context, &input, notifier).await
            {
                Ok(k) => Some(k),
                Err(e) => {
                    notifier.warn(format!("{e}"))?;
                    None
                }
            };
            Ok(input)
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use discovery::{TargetHandle, TargetState};
    use std::cell::RefCell;
    use std::path::PathBuf;

    #[derive(Debug)]
    struct MockSshKeyVerifier {
        res: RefCell<Option<anyhow::Result<HashSet<SshKey>>>>,
    }

    impl MockSshKeyVerifier {
        fn with_res(res: anyhow::Result<HashSet<SshKey>>) -> Self {
            Self { res: RefCell::new(Some(res)) }
        }
    }

    impl SshKeyVerifier for MockSshKeyVerifier {
        async fn verify_keys<N>(
            &self,
            _context: &EnvironmentContext,
            _handle: &TargetHandle,
            _notifier: &mut N,
        ) -> anyhow::Result<HashSet<SshKey>>
        where
            N: Notifier + Sized,
        {
            self.res.borrow_mut().take().expect("called `verify_keys` once")
        }
    }

    #[fuchsia::test]
    async fn test_verify_ssh_keys_success() {
        let env = ffx_config::test_env().build().unwrap();
        let mut keys = HashSet::new();
        let key_path = ffx_ssh::keys::SshKeySource::File(PathBuf::from("/tmp/test-key"));
        let mut sources = HashSet::new();
        sources.insert(key_path.clone());
        keys.insert(SshKey {
            sources,
            key_type: "ssh-ed25519".to_string(),
            key: vec![0xb2, 0x89, 0x9e, 0x91, 0xec, 0xad, 0x86, 0x29, 0xe0], // "somekeything"
            comment: Some("somekey".to_string()),
        });
        let verifier = MockSshKeyVerifier::with_res(Ok(keys));
        let check = VerifySshKeys::new(&env.context, &verifier);
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let (res, _) = check.check_with_notifier(handle.clone(), &mut notifier).await.unwrap();
        assert_eq!(res, handle);

        let output: String = notifier.into();
        assert!(output.contains("SUCCESS"));
        assert!(output.contains("Found local ssh keys"));
        assert!(output.contains("/tmp/test-key"));
    }

    #[fuchsia::test]
    async fn test_verify_ssh_keys_failure() {
        let env = ffx_config::test_env().build().unwrap();
        let verifier =
            MockSshKeyVerifier::with_res(Err(anyhow::anyhow!("key verification failed")));
        let check = VerifySshKeys::new(&env.context, &verifier);
        let handle = TargetHandle {
            node_name: Some("test-node".to_string()),
            state: TargetState::Unknown,
            manual: false,
        };
        let mut notifier = ffx_diagnostics::StringNotifier::new();
        let res = check.check_with_notifier(handle.clone(), &mut notifier).await;
        assert!(res.is_ok());
        let err: String = notifier.into();
        assert!(err.to_string().contains("key verification failed"));
    }
}
