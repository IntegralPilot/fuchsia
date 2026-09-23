// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common::crypto::unlock_device;
use crate::common::is_locked;
use crate::error::FfxFastbootError;
use crate::file_resolver::FileResolver;
use crate::util;
use crate::util::Event;

type Result<T> = std::result::Result<T, FfxFastbootError>;
use ffx_fastboot_interface::fastboot_interface::FastbootInterface;
use tokio::sync::mpsc::Sender;

pub async fn unlock<F: FileResolver + Sync, T: FastbootInterface>(
    messages: Sender<Event>,
    file_resolver: &mut F,
    credentials: &Vec<String>,
    fastboot_interface: &mut T,
) -> Result<()> {
    if !is_locked(fastboot_interface).await? {
        return Err(FfxFastbootError::AlreadyUnlocked);
    }

    if credentials.len() == 0 {
        return Err(FfxFastbootError::MissingCredentials);
    }

    unlock_device(&messages, file_resolver, credentials, fastboot_interface).await?;
    messages.send(util::Event::Unlock(util::UnlockEvent::Done)).await?;
    Ok(())
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use tokio::sync::mpsc;

    use super::*;
    type Result<T> = std::result::Result<T, anyhow::Error>;
    use crate::common::vars::LOCKED_VAR;
    use crate::file_resolver::resolvers::EmptyResolver;
    use ffx_fastboot_interface::test::setup;

    #[fuchsia::test]
    async fn test_unlocked_device_throws_err() -> Result<()> {
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            // is_locked
            state.set_var(LOCKED_VAR.to_string(), "no".to_string());
        }
        let (client, _server) = mpsc::channel(1);
        let result =
            unlock(client, &mut EmptyResolver::new()?, &vec!["test".to_string()], &mut proxy).await;
        assert!(result.is_err());
        Ok(())
    }

    #[fuchsia::test]
    async fn test_missing_creds_throws_err() -> Result<()> {
        let (state, mut proxy) = setup();
        {
            let mut state = state.lock().unwrap();
            // is_locked
            state.set_var(LOCKED_VAR.to_string(), "yes".to_string());
        }
        let (client, _server) = mpsc::channel(1);
        let result = unlock(client, &mut EmptyResolver::new()?, &vec![], &mut proxy).await;
        assert!(result.is_err());
        Ok(())
    }
}
