// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::vars::is_cacheable_variable;
use async_trait::async_trait;
use chrono::Duration;
use ffx_fastboot_interface::fastboot_interface::{
    Fastboot, FastbootError, FastbootInterface, RebootEvent, UploadProgress, Variable,
};
use ffx_fastboot_interface::stream::StreamCommand;
use std::collections::HashMap;
use tokio::sync::mpsc::Sender;

/// `CachingFastboot` is a wrapper around any [`FastbootInterface`] that caches
/// immutable fastboot variables (such as hardware revision, product, max download size,
/// and partition layout geometry) in memory.
///
/// Dynamic / mutable variables (such as `current-slot`, `unlocked`, and slot state)
/// are not cached and are always forwarded directly to the underlying interface.
/// When reboot or boot commands are issued, the variable cache is automatically invalidated.
#[derive(Debug)]
pub struct CachingFastboot<T> {
    inner: T,
    var_cache: HashMap<String, Result<String, String>>,
}

impl<T> CachingFastboot<T> {
    /// Creates a new `CachingFastboot` wrapping `inner`.
    pub fn new(inner: T) -> Self {
        Self { inner, var_cache: HashMap::new() }
    }

    /// Clears the in-memory variable cache.
    pub fn clear_cache(&mut self) {
        self.var_cache.clear();
    }
}

#[async_trait]
impl<T: Fastboot + Send> Fastboot for CachingFastboot<T> {
    async fn get_var(&mut self, name: &str) -> Result<String, FastbootError> {
        let should_cache = is_cacheable_variable(name);
        if should_cache && let Some(cached) = self.var_cache.get(name) {
            return cached
                .as_ref()
                .map(|val| {
                    log::trace!("Returning cached fastboot var {name}: '{val}'");
                    val.clone()
                })
                .map_err(|msg| {
                    log::trace!("Returning cached failure for fastboot var {name}: '{msg}'");
                    FastbootError::GetVariableError {
                        variable: name.to_string(),
                        message: msg.clone(),
                    }
                });
        }

        match self.inner.get_var(name).await {
            Ok(val) => {
                if should_cache {
                    log::trace!("Caching successful fastboot var {name}: '{val}'");
                    self.var_cache.insert(name.to_string(), Ok(val.clone()));
                }
                Ok(val)
            }
            Err(FastbootError::GetVariableError { ref variable, ref message }) => {
                if should_cache {
                    log::trace!("Caching failed fastboot var {name}: '{message}'");
                    self.var_cache.insert(variable.clone(), Err(message.clone()));
                }
                Err(FastbootError::GetVariableError {
                    variable: variable.clone(),
                    message: message.clone(),
                })
            }
            Err(e) => Err(e),
        }
    }

    async fn get_all_vars(&mut self, listener: Sender<Variable>) -> Result<(), FastbootError> {
        let (cached_tx, mut cached_rx) = tokio::sync::mpsc::channel(100);
        let send_res = self.inner.get_all_vars(cached_tx).await;
        while let Ok(var) = cached_rx.try_recv() {
            if is_cacheable_variable(&var.name) {
                self.var_cache.insert(var.name.clone(), Ok(var.value.clone()));
            }
            let _ = listener.send(var).await;
        }
        send_res
    }

    async fn flash(
        &mut self,
        partition_name: &str,
        path: &str,
        listener: Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        self.inner.flash(partition_name, path, listener, timeout).await
    }

    async fn flash_from_reader(
        &mut self,
        partition_name: &str,
        size: u32,
        reader: &mut (dyn std::io::Read + Send),
        listener: Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        self.inner.flash_from_reader(partition_name, size, reader, listener, timeout).await
    }

    async fn erase(&mut self, partition_name: &str) -> Result<(), FastbootError> {
        self.inner.erase(partition_name).await
    }

    async fn boot(&mut self) -> Result<(), FastbootError> {
        self.var_cache.clear();
        self.inner.boot().await
    }

    async fn reboot(&mut self) -> Result<(), FastbootError> {
        self.var_cache.clear();
        self.inner.reboot().await
    }

    async fn reboot_bootloader(
        &mut self,
        listener: Sender<RebootEvent>,
    ) -> Result<(), FastbootError> {
        self.var_cache.clear();
        self.inner.reboot_bootloader(listener).await
    }

    async fn continue_boot(&mut self) -> Result<(), FastbootError> {
        self.inner.continue_boot().await
    }

    async fn get_staged(&mut self, path: &str) -> Result<(), FastbootError> {
        self.inner.get_staged(path).await
    }

    async fn stage(
        &mut self,
        path: &str,
        listener: Sender<UploadProgress>,
    ) -> Result<(), FastbootError> {
        self.inner.stage(path, listener).await
    }

    async fn set_active(&mut self, slot: &str) -> Result<(), FastbootError> {
        self.inner.set_active(slot).await
    }

    async fn oem(&mut self, command: &str) -> Result<(), FastbootError> {
        self.inner.oem(command).await
    }

    async fn stream<'a>(
        &mut self,
        partition_name: &str,
        stream_command: StreamCommand,
        listener: &Sender<UploadProgress>,
        timeout: Duration,
    ) -> Result<(), FastbootError> {
        self.inner.stream(partition_name, stream_command, listener, timeout).await
    }
}

impl<T: FastbootInterface + Send> FastbootInterface for CachingFastboot<T> {}

#[cfg(test)]
mod test {
    use super::*;
    use ffx_fastboot_interface::test::setup;

    #[fuchsia::test]
    async fn test_caching_immutable_variable() {
        let (state, interface) = setup();
        {
            let mut s = state.lock().unwrap();
            s.set_var("version".to_string(), "0.4".to_string());
        }

        let mut caching = CachingFastboot::new(interface);

        // First read hits underlying interface
        assert_eq!(caching.get_var("version").await.unwrap(), "0.4");
        assert_eq!(state.lock().unwrap().get_var_call_count("version"), (true, 1));

        // Second read hits cache
        assert_eq!(caching.get_var("version").await.unwrap(), "0.4");
        assert_eq!(state.lock().unwrap().get_var_call_count("version"), (true, 1));
    }

    #[fuchsia::test]
    async fn test_uncached_mutable_variable() {
        let (state, interface) = setup();
        {
            let mut s = state.lock().unwrap();
            s.set_var("current-slot".to_string(), "a".to_string());
        }

        let mut caching = CachingFastboot::new(interface);

        assert_eq!(caching.get_var("current-slot").await.unwrap(), "a");
        assert_eq!(state.lock().unwrap().get_var_call_count("current-slot"), (true, 1));

        // Change value in underlying state
        {
            let mut s = state.lock().unwrap();
            s.set_var("current-slot".to_string(), "b".to_string());
        }

        // Second read bypasses cache and reflects updated value
        assert_eq!(caching.get_var("current-slot").await.unwrap(), "b");
        assert_eq!(state.lock().unwrap().get_var_call_count("current-slot"), (true, 2));
    }

    #[fuchsia::test]
    async fn test_cache_cleared_on_reboot() {
        let (state, interface) = setup();
        {
            let mut s = state.lock().unwrap();
            s.set_var("version".to_string(), "0.4".to_string());
        }

        let mut caching = CachingFastboot::new(interface);

        assert_eq!(caching.get_var("version").await.unwrap(), "0.4");
        assert_eq!(state.lock().unwrap().get_var_call_count("version"), (true, 1));

        // Reboot clears the cache
        caching.reboot().await.unwrap();

        // Next read queries underlying interface again
        assert_eq!(caching.get_var("version").await.unwrap(), "0.4");
        assert_eq!(state.lock().unwrap().get_var_call_count("version"), (true, 2));
    }

    #[fuchsia::test]
    async fn test_caching_failure_for_immutable_var() {
        let (state, interface) = setup();
        let mut caching = CachingFastboot::new(interface);

        // Variable does not exist
        assert!(caching.get_var("max-download-size").await.is_err());
        assert_eq!(state.lock().unwrap().get_var_call_count("max-download-size"), (false, 1));

        // Second query returns cached error without calling inner interface
        assert!(caching.get_var("max-download-size").await.is_err());
        assert_eq!(state.lock().unwrap().get_var_call_count("max-download-size"), (false, 1));
    }
}
