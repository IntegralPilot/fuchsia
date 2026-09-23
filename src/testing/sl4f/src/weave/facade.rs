// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::common_utils::common::LazyProxy;
use crate::weave::types::{PairingState, ResetConfig};
use anyhow::Error;
use fidl::endpoints::create_proxy;
use fidl_fuchsia_weave::{
    ErrorCode, FactoryDataManagerMarker, FactoryDataManagerProxy, PairingStateWatcherMarker,
    PairingStateWatcherProxy, ResetConfigFlags, StackMarker, StackProxy,
};
use serde_json::Value;

/// Perform Weave FIDL operations.
///
/// Note this object is shared among all threads created by server.
#[derive(Debug)]
pub struct WeaveFacade {
    factory_data_manager: LazyProxy<FactoryDataManagerMarker>,
    stack: LazyProxy<StackMarker>,
}

impl WeaveFacade {
    pub fn new() -> WeaveFacade {
        WeaveFacade { factory_data_manager: Default::default(), stack: Default::default() }
    }

    /// Returns the FactoryDataManager proxy provided on instantiation
    /// or establishes a new connection.
    fn factory_data_manager(&self) -> Result<FactoryDataManagerProxy, Error> {
        self.factory_data_manager.get_or_connect()
    }

    /// Returns the Stack proxy provided on instantiation or establishes a new connection.
    fn stack(&self) -> Result<StackProxy, Error> {
        self.stack.get_or_connect()
    }

    /// Returns the PairingStateWatcher proxy provided on instantiation.
    fn pairing_state_watcher(&self) -> Result<PairingStateWatcherProxy, Error> {
        let (pairing_proxy, pairing_server_end) = create_proxy::<PairingStateWatcherMarker>();
        self.stack()?.get_pairing_state_watcher(pairing_server_end)?;
        Ok(pairing_proxy)
    }

    /// Returns a string mapped from the provided Weave error code.
    fn map_weave_err(&self, code: ErrorCode) -> anyhow::Error {
        anyhow!(match code {
            ErrorCode::FileNotFound => "FileNotFound",
            ErrorCode::CryptoError => "CryptoError",
            ErrorCode::InvalidArgument => "InvalidArgument",
            ErrorCode::InvalidState => "InvalidState",
            ErrorCode::UnspecifiedError => "UnspecifiedError",
        })
    }

    /// Returns the pairing code from the FactoryDataManager proxy service.
    pub async fn get_pairing_code(&self) -> Result<Vec<u8>, Error> {
        self.factory_data_manager()?.get_pairing_code().await?.map_err(|e| self.map_weave_err(e))
    }

    /// Returns the qr code from the StackManager proxy service.
    pub async fn get_qr_code(&self) -> Result<String, Error> {
        self.stack()?
            .get_qr_code()
            .await?
            .map(|qr_code| qr_code.data)
            .map_err(|e| self.map_weave_err(e))
    }

    /// Returns the pairing state from the PairingStateWatcher service.
    pub async fn get_pairing_state(&self) -> Result<PairingState, Error> {
        let watch = self.pairing_state_watcher()?.watch_pairing_state().await;
        watch.map(|pairing_state| pairing_state.into()).map_err(anyhow::Error::from)
    }

    /// Resets Weave state by wiping the provided configurations.
    ///
    /// # Arguments
    /// * `args`: The JSON indicating the configurations to reset, in the form of a list of bytes.
    ///
    /// # JSON Format
    /// All fields are optional. Fields set to 'true' reset the corresponding configuration in
    /// weave, and fields left unset default to 'false'.
    ///
    /// {
    ///   network_config: true,
    ///   fabric_config: false,
    ///   service_config: true,
    ///   operational_credentials: false
    /// }
    pub async fn reset_config(&self, args: Value) -> Result<(), Error> {
        let flags: ResetConfig = serde_json::from_value(args)?;
        self.stack()?
            .reset_config(ResetConfigFlags::from(flags))
            .await?
            .map_err(|e| self.map_weave_err(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_weave::{
        FactoryDataManagerRequest, PairingStateWatcherRequest, QrCode, StackGetQrCodeResult,
        StackRequest, StackResetConfigResult,
    };
    use fuchsia_async as fasync;
    use futures::prelude::*;
    use serde_json::json;
    use std::sync::LazyLock;

    const PAIRING_CODE: &'static [u8] = b"ABC1234";

    static QR_CODE: LazyLock<QrCode> =
        LazyLock::new(|| QrCode { data: String::from("qrcodedata") });
    static PAIRING_STATE: LazyLock<PairingState> = LazyLock::new(|| PairingState {
        is_wlan_provisioned: Some(true),
        is_fabric_provisioned: Some(true),
        is_service_provisioned: Some(false),
        is_weave_fully_provisioned: Some(false),
        is_thread_provisioned: Some(true),
    });
    static RESET_CONFIG: LazyLock<Value> = LazyLock::new(|| {
        json!({
            "network_config": true,
            // "fabric_config" unset
            "service_config": true,
            // "operational_credentials" unset
        })
    });

    struct MockStackBuilder {
        expected_stack: Vec<Box<dyn FnOnce(StackRequest) + Send + 'static>>,
        expected_pair: Vec<Box<dyn FnOnce(PairingStateWatcherRequest) + Send + 'static>>,
    }

    impl MockStackBuilder {
        fn new() -> Self {
            Self { expected_stack: vec![], expected_pair: vec![] }
        }

        fn push_stack(mut self, request: impl FnOnce(StackRequest) + Send + 'static) -> Self {
            self.expected_stack.push(Box::new(request));
            self
        }

        fn push_pair(
            mut self,
            request: impl FnOnce(PairingStateWatcherRequest) + Send + 'static,
        ) -> Self {
            self.expected_pair.push(Box::new(request));
            self
        }

        fn expect_get_qr_code(self, result: StackGetQrCodeResult) -> Self {
            self.push_stack(move |req| match req {
                StackRequest::GetQrCode { responder } => {
                    responder.send(result.as_ref().map_err(|e| *e)).unwrap()
                }
                req => panic!("unexpected request: {:?}", req),
            })
        }

        fn expect_get_pairing_state(self, result: PairingState) -> Self {
            self.push_pair(move |req| match req {
                PairingStateWatcherRequest::WatchPairingState { responder } => {
                    responder.send(&fidl_fuchsia_weave::PairingState::from(result)).unwrap();
                }
            })
        }

        fn expect_reset_config(
            self,
            _expected_flags: fidl_fuchsia_weave::ResetConfigFlags,
            result: StackResetConfigResult,
        ) -> Self {
            self.push_stack(move |req| match req {
                StackRequest::ResetConfig { responder, flags } => {
                    assert_matches!(flags, _expected_flags);
                    responder.send(result).unwrap()
                }
                req => panic!("unexpected request: {:?}", req),
            })
        }

        fn build_stack(self) -> (WeaveFacade, impl Future<Output = ()>) {
            let (proxy, mut stream) = create_proxy_and_stream::<StackMarker>();
            let fut = async move {
                let _ = &self;
                for expected in self.expected_stack {
                    expected(stream.next().await.unwrap().unwrap());
                }
                assert_matches!(stream.next().await, None);
            };
            let facade = WeaveFacade::new();
            facade.stack.set(proxy).expect("just-created facade should have empty stack");
            (facade, fut)
        }
        fn build_stack_and_pairing_state_watcher(self) -> (WeaveFacade, impl Future<Output = ()>) {
            let (proxy, mut stream) = create_proxy_and_stream::<StackMarker>();
            let stream_fut = async move {
                let _ = &self;
                match stream.next().await {
                    Some(Ok(StackRequest::GetPairingStateWatcher {
                        watcher,
                        control_handle: _,
                    })) => {
                        let mut into_stream = watcher.into_stream();
                        for expected in self.expected_pair {
                            expected(into_stream.next().await.unwrap().unwrap());
                        }
                        assert_matches!(into_stream.next().await, None);
                    }
                    err => panic!("Error in request handler: {:?}", err),
                }
            };
            let facade = WeaveFacade::new();
            facade.stack.set(proxy).expect("just-created facade should have empty stack");
            (facade, stream_fut)
        }
    }

    struct MockFactoryDataManagerBuilder {
        expected: Vec<Box<dyn FnOnce(FactoryDataManagerRequest) + Send + 'static>>,
    }

    impl MockFactoryDataManagerBuilder {
        fn new() -> Self {
            Self { expected: vec![] }
        }

        fn push(
            mut self,
            request: impl FnOnce(FactoryDataManagerRequest) + Send + 'static,
        ) -> Self {
            self.expected.push(Box::new(request));
            self
        }

        fn expect_get_pairing_code(self, result: Result<&'static [u8], ErrorCode>) -> Self {
            self.push(move |req| match req {
                FactoryDataManagerRequest::GetPairingCode { responder } => {
                    responder.send(result).unwrap()
                }
                _ => {}
            })
        }

        fn build(self) -> (WeaveFacade, impl Future<Output = ()>) {
            let (proxy, mut stream) =
                fidl::endpoints::create_proxy_and_stream::<FactoryDataManagerMarker>();
            let fut = async move {
                for expected in self.expected {
                    expected(stream.next().await.unwrap().unwrap());
                }
                assert_matches!(stream.next().await, None);
            };
            let facade = WeaveFacade::new();
            facade
                .factory_data_manager
                .set(proxy)
                .expect("just-created facade should have empty fdm");
            (facade, fut)
        }
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_get_pairing_code() {
        let (facade, pairing_code_fut) =
            MockFactoryDataManagerBuilder::new().expect_get_pairing_code(Ok(PAIRING_CODE)).build();

        let facade_fut = async move {
            assert_eq!(facade.get_pairing_code().await.unwrap(), PAIRING_CODE);
        };

        future::join(facade_fut, pairing_code_fut).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_get_qr_code() {
        let qr_clone: StackGetQrCodeResult = Ok((*QR_CODE).clone());
        let (facade, qr_code_fut) =
            MockStackBuilder::new().expect_get_qr_code(qr_clone).build_stack();

        let facade_fut = async move {
            assert_eq!(facade.get_qr_code().await.unwrap(), *QR_CODE.data);
        };

        future::join(facade_fut, qr_code_fut).await;
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_get_pairing_state() {
        let (facade, pairing_state_fut) = MockStackBuilder::new()
            .expect_get_pairing_state(*PAIRING_STATE)
            .build_stack_and_pairing_state_watcher();

        let facade_fut = async move {
            let pairing_state = facade.get_pairing_state().await.unwrap();
            assert_eq!(pairing_state, *PAIRING_STATE);
        };

        future::join(facade_fut, pairing_state_fut).await;
    }

    #[allow(clippy::unit_cmp)] // TODO(https://fxbug.dev/42176996)
    #[fasync::run_singlethreaded(test)]
    async fn test_reset_config() {
        let flags: ResetConfigFlags = fidl_fuchsia_weave::ResetConfigFlags::NETWORK_CONFIG
            | fidl_fuchsia_weave::ResetConfigFlags::SERVICE_CONFIG;
        let (facade, reset_config_fut) =
            MockStackBuilder::new().expect_reset_config(flags, Ok(())).build_stack();

        let facade_fut =
            async move { assert_eq!(facade.reset_config(RESET_CONFIG.clone()).await.unwrap(), ()) };

        future::join(facade_fut, reset_config_fut).await;
    }
}
