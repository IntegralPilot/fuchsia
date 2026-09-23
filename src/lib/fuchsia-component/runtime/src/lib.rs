// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This library is only usable at API versions from which fuchsia.component.runtime.Capabilities is
// available.

#[cfg(fuchsia_api_level_at_least = "HEAD")]
pub use everything::*;

#[cfg(fuchsia_api_level_less_than = "HEAD")]
mod everything {
    use fidl as _;
    use fidl_fuchsia_component_runtime as _;
    use fidl_fuchsia_io as _;
    use fuchsia_component_client as _;
    use futures as _;
    use zx as _;

    #[cfg(test)]
    mod tests {
        use assert_matches as _;
    }
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
mod everything {
    use fidl::endpoints::{ServerEnd, create_proxy, create_request_stream};
    use fidl_fuchsia_component_runtime as fruntime;
    use fidl_fuchsia_io as fio;
    use fuchsia_component_client::connect_to_protocol;
    use futures::future::BoxFuture;
    use futures::{Future, Stream, StreamExt};
    use std::pin::{Pin, pin};
    use std::task::{Context, Poll};

    /// The value of a data capability. Can be stored in or retrieved from a [`Data`].
    #[derive(Debug, PartialEq, Clone)]
    pub enum DataValue {
        Bytes(Vec<u8>),
        String(String),
        Int64(i64),
        Uint64(u64),
    }

    impl From<Vec<u8>> for DataValue {
        fn from(val: Vec<u8>) -> Self {
            Self::Bytes(val)
        }
    }

    impl From<String> for DataValue {
        fn from(val: String) -> Self {
            Self::String(val)
        }
    }

    impl From<i64> for DataValue {
        fn from(val: i64) -> Self {
            Self::Int64(val)
        }
    }

    impl From<u64> for DataValue {
        fn from(val: u64) -> Self {
            Self::Uint64(val)
        }
    }

    impl TryFrom<fruntime::Data> for DataValue {
        type Error = ();

        fn try_from(val: fruntime::Data) -> Result<Self, Self::Error> {
            match val {
                fruntime::Data::Bytes(b) => Ok(Self::Bytes(b)),
                fruntime::Data::String(b) => Ok(Self::String(b)),
                fruntime::Data::Int64(b) => Ok(Self::Int64(b)),
                fruntime::Data::Uint64(b) => Ok(Self::Uint64(b)),
                _other_value => Err(()),
            }
        }
    }

    impl From<DataValue> for fruntime::Data {
        fn from(val: DataValue) -> Self {
            match val {
                DataValue::Bytes(b) => Self::Bytes(b),
                DataValue::String(b) => Self::String(b),
                DataValue::Int64(b) => Self::Int64(b),
                DataValue::Uint64(b) => Self::Uint64(b),
            }
        }
    }

    /// Receives new channels sent over a [`Connector`]
    pub struct ConnectorReceiver {
        pub stream: fruntime::ReceiverRequestStream,
    }

    impl From<fruntime::ReceiverRequestStream> for ConnectorReceiver {
        fn from(stream: fruntime::ReceiverRequestStream) -> Self {
            Self { stream }
        }
    }

    impl Stream for ConnectorReceiver {
        type Item = zx::Channel;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let pinned_stream = pin!(&mut self.stream);
            match pinned_stream.poll_next(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Some(Ok(fruntime::ReceiverRequest::Receive { channel, .. }))) => {
                    Poll::Ready(Some(channel))
                }
                _ => Poll::Ready(None),
            }
        }
    }

    /// Receives new fuchsia.io open requests sent over a [`DirConnector`]
    pub struct DirConnectorReceiver {
        pub stream: fruntime::DirReceiverRequestStream,
    }

    impl From<fruntime::DirReceiverRequestStream> for DirConnectorReceiver {
        fn from(stream: fruntime::DirReceiverRequestStream) -> Self {
            Self { stream }
        }
    }

    pub struct DirConnectorRequest {
        pub channel: ServerEnd<fio::DirectoryMarker>,
        pub path: String,
        pub flags: fio::Flags,
    }

    impl Stream for DirConnectorReceiver {
        type Item = DirConnectorRequest;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let pinned_stream = pin!(&mut self.stream);
            match pinned_stream.poll_next(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Some(Ok(fruntime::DirReceiverRequest::Receive {
                    channel,
                    path,
                    rights,
                    ..
                }))) => Poll::Ready(Some(DirConnectorRequest { channel, path, flags: rights })),
                _ => Poll::Ready(None),
            }
        }
    }

    /// A capability is a typed reference to an object owned by component manager. This reference which
    /// may used to interact with the referenced object or this reference may be passed to another
    /// component (or component manager itself).
    #[derive(Debug, Clone)]
    pub enum Capability {
        Connector(Connector),
        ConnectorRouter(ConnectorRouter),
        Data(Data),
        DataRouter(DataRouter),
        Dictionary(Dictionary),
        DictionaryRouter(DictionaryRouter),
        DirConnector(DirConnector),
        DirConnectorRouter(DirConnectorRouter),
        InstanceToken(InstanceToken),
    }

    impl Capability {
        pub fn from_raw_with_proxy(
            capabilities_proxy: fruntime::CapabilitiesProxy,
            handle: zx::EventPair,
            type_: fruntime::CapabilityType,
        ) -> Self {
            match type_ {
                fruntime::CapabilityType::Data => Self::Data(Data { handle, capabilities_proxy }),
                fruntime::CapabilityType::Connector => {
                    Self::Connector(Connector { handle, capabilities_proxy })
                }
                fruntime::CapabilityType::DirConnector => {
                    Self::DirConnector(DirConnector { handle, capabilities_proxy })
                }
                fruntime::CapabilityType::Dictionary => {
                    Self::Dictionary(Dictionary { handle, capabilities_proxy })
                }
                fruntime::CapabilityType::DataRouter => {
                    Self::DataRouter(DataRouter { handle, capabilities_proxy })
                }
                fruntime::CapabilityType::ConnectorRouter => {
                    Self::ConnectorRouter(ConnectorRouter { handle, capabilities_proxy })
                }
                fruntime::CapabilityType::DirConnectorRouter => {
                    Self::DirConnectorRouter(DirConnectorRouter { handle, capabilities_proxy })
                }
                fruntime::CapabilityType::DictionaryRouter => {
                    Self::DictionaryRouter(DictionaryRouter { handle, capabilities_proxy })
                }
                fruntime::CapabilityType::InstanceToken => {
                    Self::InstanceToken(InstanceToken { handle })
                }
                other_type => panic!("unknown capability type: {other_type:?}"),
            }
        }

        pub fn as_event_pair(&self) -> &zx::EventPair {
            match self {
                Self::Data(data) => &data.handle,
                Self::Connector(connector) => &connector.handle,
                Self::DirConnector(dir_connector) => &dir_connector.handle,
                Self::Dictionary(dictionary) => &dictionary.handle,
                Self::DataRouter(data_router) => &data_router.handle,
                Self::ConnectorRouter(connector_router) => &connector_router.handle,
                Self::DirConnectorRouter(dir_connector_router) => &dir_connector_router.handle,
                Self::DictionaryRouter(dictionary_router) => &dictionary_router.handle,
                Self::InstanceToken(instance_token) => &instance_token.handle,
            }
        }
    }

    impl From<Data> for Capability {
        fn from(val: Data) -> Self {
            Self::Data(val)
        }
    }

    impl From<Connector> for Capability {
        fn from(val: Connector) -> Self {
            Self::Connector(val)
        }
    }

    impl From<DirConnector> for Capability {
        fn from(val: DirConnector) -> Self {
            Self::DirConnector(val)
        }
    }

    impl From<Dictionary> for Capability {
        fn from(val: Dictionary) -> Self {
            Self::Dictionary(val)
        }
    }

    impl From<DataRouter> for Capability {
        fn from(val: DataRouter) -> Self {
            Self::DataRouter(val)
        }
    }

    impl From<ConnectorRouter> for Capability {
        fn from(val: ConnectorRouter) -> Self {
            Self::ConnectorRouter(val)
        }
    }

    impl From<DirConnectorRouter> for Capability {
        fn from(val: DirConnectorRouter) -> Self {
            Self::DirConnectorRouter(val)
        }
    }

    impl From<DictionaryRouter> for Capability {
        fn from(val: DictionaryRouter) -> Self {
            Self::DictionaryRouter(val)
        }
    }

    impl From<InstanceToken> for Capability {
        fn from(val: InstanceToken) -> Self {
            Self::InstanceToken(val)
        }
    }

    impl TryFrom<Capability> for Data {
        type Error = ();
        fn try_from(val: Capability) -> Result<Self, Self::Error> {
            match val {
                Capability::Data(val) => Ok(val),
                _ => Err(()),
            }
        }
    }

    impl TryFrom<Capability> for Connector {
        type Error = ();
        fn try_from(val: Capability) -> Result<Self, Self::Error> {
            match val {
                Capability::Connector(val) => Ok(val),
                _ => Err(()),
            }
        }
    }

    impl TryFrom<Capability> for DirConnector {
        type Error = ();
        fn try_from(val: Capability) -> Result<Self, Self::Error> {
            match val {
                Capability::DirConnector(val) => Ok(val),
                _ => Err(()),
            }
        }
    }

    impl TryFrom<Capability> for Dictionary {
        type Error = ();
        fn try_from(val: Capability) -> Result<Self, Self::Error> {
            match val {
                Capability::Dictionary(val) => Ok(val),
                _ => Err(()),
            }
        }
    }

    impl TryFrom<Capability> for DataRouter {
        type Error = ();
        fn try_from(val: Capability) -> Result<Self, Self::Error> {
            match val {
                Capability::DataRouter(val) => Ok(val),
                _ => Err(()),
            }
        }
    }

    impl TryFrom<Capability> for ConnectorRouter {
        type Error = ();
        fn try_from(val: Capability) -> Result<Self, Self::Error> {
            match val {
                Capability::ConnectorRouter(val) => Ok(val),
                _ => Err(()),
            }
        }
    }

    impl TryFrom<Capability> for DirConnectorRouter {
        type Error = ();
        fn try_from(val: Capability) -> Result<Self, Self::Error> {
            match val {
                Capability::DirConnectorRouter(val) => Ok(val),
                _ => Err(()),
            }
        }
    }

    impl TryFrom<Capability> for DictionaryRouter {
        type Error = ();
        fn try_from(val: Capability) -> Result<Self, Self::Error> {
            match val {
                Capability::DictionaryRouter(val) => Ok(val),
                _ => Err(()),
            }
        }
    }

    impl TryFrom<Capability> for InstanceToken {
        type Error = ();
        fn try_from(val: Capability) -> Result<Self, Self::Error> {
            match val {
                Capability::InstanceToken(val) => Ok(val),
                _ => Err(()),
            }
        }
    }

    /// A data capability holds a bit of static data which can be read back.
    #[derive(Debug)]
    pub struct Data {
        /// The handle that references this capability
        pub handle: zx::EventPair,

        /// The proxy used to create this capability, and the proxy which will be used to perform
        /// operations on this capability.
        pub capabilities_proxy: fruntime::CapabilitiesProxy,
    }

    impl From<zx::EventPair> for Data {
        fn from(handle: zx::EventPair) -> Self {
            Self {
                handle,
                capabilities_proxy: connect_to_protocol::<fruntime::CapabilitiesMarker>()
                    .expect("failed to connect to fuchsia.component.runtime.Capabilities"),
            }
        }
    }

    impl Data {
        /// Creates a new [`Data`], connecting to `/svc/fuchsia.component.runtime.Capabilities` to do
        /// so.
        pub async fn new(value: DataValue) -> Self {
            let proxy = connect_to_protocol::<fruntime::CapabilitiesMarker>()
                .expect("failed to connect to fuchsia.component.runtime.Capabilities");
            Self::new_with_proxy(proxy, value).await
        }

        /// Creates a new [`Data`] using the provided `capabilities_proxy`.
        pub async fn new_with_proxy(
            capabilities_proxy: fruntime::CapabilitiesProxy,
            value: DataValue,
        ) -> Self {
            let (handle, handle_other_end) = zx::EventPair::create();
            capabilities_proxy
                .data_create(handle_other_end, &value.into())
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect(
                    "this should be impossible, we passed a valid handle with the correct rights",
                );
            Self { handle, capabilities_proxy }
        }

        /// Associates `other_handle` with the same object referenced by this capability, so that
        /// whoever holds the other end of `other_handle` can refer to our capability.
        pub async fn associate_with_handle(&self, other_handle: zx::EventPair) {
            self.capabilities_proxy
                .capability_associate_handle(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    other_handle,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect("failed to clone onto handle, does it have sufficient rights?");
        }

        pub async fn get_value(&self) -> DataValue {
            self.capabilities_proxy
                .data_get(self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                    "failed to duplicate handle, please only use handles with the duplicate right",
                ))
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect("failed to get data value")
                .try_into()
                .expect("we were sent an invalid data value")
        }
    }

    impl Clone for Data {
        fn clone(&self) -> Self {
            Self {
                handle: self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                    "failed to duplicate handle, please only use handles with the duplicate right",
                ),
                capabilities_proxy: self.capabilities_proxy.clone(),
            }
        }
    }

    /// A connector capability can be invoked to send a channel to the creator of the connector.
    #[derive(Debug)]
    pub struct Connector {
        /// The handle that references this capability
        pub handle: zx::EventPair,

        /// The proxy used to create this capability, and the proxy which will be used to perform
        /// operations on this capability.
        pub capabilities_proxy: fruntime::CapabilitiesProxy,
    }

    impl From<zx::EventPair> for Connector {
        fn from(handle: zx::EventPair) -> Self {
            Self {
                handle,
                capabilities_proxy: connect_to_protocol::<fruntime::CapabilitiesMarker>()
                    .expect("failed to connect to fuchsia.component.runtime.Capabilities"),
            }
        }
    }

    impl Connector {
        /// Creates a new [`Connector`], connecting to `/svc/fuchsia.component.runtime.Capabilities` to
        /// do so.
        pub async fn new() -> (Self, ConnectorReceiver) {
            let proxy = connect_to_protocol::<fruntime::CapabilitiesMarker>()
                .expect("failed to connect to fuchsia.component.runtime.Capabilities");
            Self::new_with_proxy(proxy).await
        }

        /// Creates a new [`Connector`] using the provided `capabilities_proxy`.
        pub async fn new_with_proxy(
            capabilities_proxy: fruntime::CapabilitiesProxy,
        ) -> (Self, ConnectorReceiver) {
            let (handle, handle_other_end) = zx::EventPair::create();
            let (receiver_client_end, stream) = create_request_stream::<fruntime::ReceiverMarker>();
            capabilities_proxy
                .connector_create(handle_other_end, receiver_client_end)
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect(
                    "this should be impossible, we passed a valid handle with the correct rights",
                );
            let connector = Self { handle, capabilities_proxy };
            let receiver = ConnectorReceiver { stream };
            (connector, receiver)
        }

        /// Associates `other_handle` with the same object referenced by this capability, so that
        /// whoever holds the other end of `other_handle` can refer to our capability.
        pub async fn associate_with_handle(&self, other_handle: zx::EventPair) {
            self.capabilities_proxy
                .capability_associate_handle(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    other_handle,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect("failed to clone onto handle, does it have sufficient rights?");
        }

        pub async fn connect(
            &self,
            channel: zx::Channel,
        ) -> Result<(), fruntime::CapabilitiesError> {
            self.capabilities_proxy
                .connector_open(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    channel,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
        }
    }

    impl Clone for Connector {
        fn clone(&self) -> Self {
            Self {
                handle: self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                    "failed to duplicate handle, please only use handles with the duplicate right",
                ),
                capabilities_proxy: self.capabilities_proxy.clone(),
            }
        }
    }

    /// A dir connector can be invoked to send a fuchsia.io open request to the creator of the dir
    /// connector.
    #[derive(Debug)]
    pub struct DirConnector {
        /// The handle that references this capability
        pub handle: zx::EventPair,

        /// The proxy used to create this capability, and the proxy which will be used to perform
        /// operations on this capability.
        pub capabilities_proxy: fruntime::CapabilitiesProxy,
    }

    impl From<zx::EventPair> for DirConnector {
        fn from(handle: zx::EventPair) -> Self {
            Self {
                handle,
                capabilities_proxy: connect_to_protocol::<fruntime::CapabilitiesMarker>()
                    .expect("failed to connect to fuchsia.component.runtime.Capabilities"),
            }
        }
    }

    impl DirConnector {
        /// Creates a new [`DirConnector`], connecting to `/svc/fuchsia.component.runtime.Capabilities`
        /// to do so.
        pub async fn new() -> (Self, DirConnectorReceiver) {
            let proxy = connect_to_protocol::<fruntime::CapabilitiesMarker>()
                .expect("failed to connect to fuchsia.component.runtime.Capabilities");
            Self::new_with_proxy(proxy).await
        }

        /// Creates a new [`DirConnector`] using the provided `capabilities_proxy`.
        pub async fn new_with_proxy(
            capabilities_proxy: fruntime::CapabilitiesProxy,
        ) -> (Self, DirConnectorReceiver) {
            let (handle, handle_other_end) = zx::EventPair::create();
            let (receiver_client_end, stream) =
                create_request_stream::<fruntime::DirReceiverMarker>();
            capabilities_proxy
                .dir_connector_create(handle_other_end, receiver_client_end)
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect(
                    "this should be impossible, we passed a valid handle with the correct rights",
                );
            let connector = Self { handle, capabilities_proxy };
            let receiver = DirConnectorReceiver { stream };
            (connector, receiver)
        }

        /// Associates `other_handle` with the same object referenced by this capability, so that
        /// whoever holds the other end of `other_handle` can refer to our capability.
        pub async fn associate_with_handle(&self, other_handle: zx::EventPair) {
            self.capabilities_proxy
                .capability_associate_handle(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    other_handle,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect("failed to clone onto handle, does it have sufficient rights?");
        }

        pub async fn connect(
            &self,
            server_end: ServerEnd<fio::DirectoryMarker>,
            flags: Option<fio::Flags>,
            path: Option<String>,
        ) -> Result<(), fruntime::CapabilitiesError> {
            self.capabilities_proxy
                .dir_connector_open(fruntime::CapabilitiesDirConnectorOpenRequest {
                    dir_connector: Some(self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    )),
                    channel: Some(server_end),
                    flags,
                    path,
                    ..Default::default()
                })
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
        }
    }

    impl Clone for DirConnector {
        fn clone(&self) -> Self {
            Self {
                handle: self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                    "failed to duplicate handle, please only use handles with the duplicate right",
                ),
                capabilities_proxy: self.capabilities_proxy.clone(),
            }
        }
    }

    /// A dictionary is a key-value mapping of names to other capabilities.
    #[derive(Debug)]
    pub struct Dictionary {
        /// The handle that references this capability
        pub handle: zx::EventPair,

        /// The proxy used to create this capability, and the proxy which will be used to perform
        /// operations on this capability.
        pub capabilities_proxy: fruntime::CapabilitiesProxy,
    }

    impl From<zx::EventPair> for Dictionary {
        fn from(handle: zx::EventPair) -> Self {
            Self {
                handle,
                capabilities_proxy: connect_to_protocol::<fruntime::CapabilitiesMarker>()
                    .expect("failed to connect to fuchsia.component.runtime.Capabilities"),
            }
        }
    }

    impl Dictionary {
        /// Creates a new [`Dictionary`], connecting to `/svc/fuchsia.component.runtime.Capabilities`
        /// to do so.
        pub async fn new() -> Self {
            let proxy = connect_to_protocol::<fruntime::CapabilitiesMarker>()
                .expect("failed to connect to fuchsia.component.runtime.Capabilities");
            Self::new_with_proxy(proxy).await
        }

        /// Creates a new [`Dictionary`] using the provided `capabilities_proxy`.
        pub async fn new_with_proxy(capabilities_proxy: fruntime::CapabilitiesProxy) -> Self {
            let (handle, handle_other_end) = zx::EventPair::create();
            capabilities_proxy
                .dictionary_create(handle_other_end)
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect(
                    "this should be impossible, we passed a valid handle with the correct rights",
                );
            Self { handle, capabilities_proxy }
        }

        /// Associates `other_handle` with the same object referenced by this capability, so that
        /// whoever holds the other end of `other_handle` can refer to our capability.
        pub async fn associate_with_handle(&self, other_handle: zx::EventPair) {
            self.capabilities_proxy
                .capability_associate_handle(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    other_handle,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect("failed to clone onto handle, does it have sufficient rights?");
        }

        pub async fn insert(&self, key: &str, value: impl Into<Capability>) {
            let capability: Capability = value.into();
            let dictionary = self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                "failed to duplicate handle, please only use handles with the duplicate right",
            );
            let handle =
                capability.as_event_pair().duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                    "failed to duplicate handle, please only use handles with the duplicate right",
                );
            self.capabilities_proxy
                .dictionary_insert(dictionary, key, handle)
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect("failed to insert into dictionary")
        }

        pub async fn get(&self, key: &str) -> Option<Capability> {
            let (handle, handle_other_end) = zx::EventPair::create();
            let res = self
                .capabilities_proxy
                .dictionary_get(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    key,
                    handle_other_end,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities");
            match res {
                Ok(type_) => Some(Capability::from_raw_with_proxy(
                    self.capabilities_proxy.clone(),
                    handle,
                    type_,
                )),
                Err(fruntime::CapabilitiesError::NoSuchCapability) => None,
                Err(other_error) => panic!(
                    "this arm should be impossible, we passed a valid handle with the correct rights: {other_error:?}"
                ),
            }
        }

        pub async fn remove(&self, key: &str) -> Option<Capability> {
            let (handle, handle_other_end) = zx::EventPair::create();
            let res = self
                .capabilities_proxy
                .dictionary_remove(fruntime::CapabilitiesDictionaryRemoveRequest {
                    dictionary: Some(self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    )),
                    key: Some(key.to_string()),
                    value: Some(handle_other_end),
                    ..Default::default()
                })
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities");
            match res {
                Ok(type_) => Some(Capability::from_raw_with_proxy(
                    self.capabilities_proxy.clone(),
                    handle,
                    type_,
                )),
                Err(fruntime::CapabilitiesError::NoSuchCapability) => None,
                Err(other_error) => panic!(
                    "this arm should be impossible, we passed a valid handle with the correct rights: {other_error:?}"
                ),
            }
        }

        pub async fn keys(&self) -> DictionaryKeysStream {
            let (key_iterator_proxy, key_iterator_server_end) =
                create_proxy::<fruntime::DictionaryKeyIteratorMarker>();
            self.capabilities_proxy
                .dictionary_iterate_keys(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    key_iterator_server_end,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect("failed to iterate keys");
            DictionaryKeysStream { key_iterator_proxy, key_cache: vec![], more_keys_fut: None }
        }
    }

    impl Clone for Dictionary {
        fn clone(&self) -> Self {
            Self {
                handle: self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                    "failed to duplicate handle, please only use handles with the duplicate right",
                ),
                capabilities_proxy: self.capabilities_proxy.clone(),
            }
        }
    }

    pub struct DictionaryKeysStream {
        key_iterator_proxy: fruntime::DictionaryKeyIteratorProxy,
        key_cache: Vec<String>,
        more_keys_fut: Option<
            fidl::client::QueryResponseFut<
                Vec<String>,
                fidl::encoding::DefaultFuchsiaResourceDialect,
            >,
        >,
    }

    impl Stream for DictionaryKeysStream {
        type Item = String;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            if let Some(key) = self.key_cache.pop() {
                return Poll::Ready(Some(key));
            }
            if self.more_keys_fut.is_none() {
                self.more_keys_fut = Some(self.key_iterator_proxy.get_next());
            }

            let fut = self.more_keys_fut.as_mut().expect("we just checked if this was None");
            let fut = pin!(fut);
            match fut.poll(cx) {
                Poll::Ready(Ok(mut keys)) if !keys.is_empty() => {
                    self.key_cache.append(&mut keys);
                    self.key_cache.reverse();
                    self.more_keys_fut = None;
                    Poll::Ready(self.key_cache.pop())
                }
                Poll::Pending => Poll::Pending,
                _ => Poll::Ready(None),
            }
        }
    }

    /// An instance token is an opaque identifier tied to a specific component instance. Component
    /// manager relies on these internally to identify which component has initiated a given routing
    /// operation.
    #[derive(Debug)]
    pub struct InstanceToken {
        /// The handle that references this capability
        pub handle: zx::EventPair,
    }

    impl From<zx::EventPair> for InstanceToken {
        fn from(handle: zx::EventPair) -> Self {
            Self { handle }
        }
    }

    impl InstanceToken {
        /// Creates a new [`InstanceToken`], connecting to
        /// `/svc/fuchsia.component.runtime.Capabilities` to do so. This instance token will be tied to
        /// the component that `fuchsia.component.runtime.Capabilities` is scoped to (which will
        /// typically be the same component this code is running in).
        pub async fn new() -> Self {
            let proxy = connect_to_protocol::<fruntime::CapabilitiesMarker>()
                .expect("failed to connect to fuchsia.component.runtime.Capabilities");
            Self::new_with_proxy(proxy).await
        }

        /// Creates a new [`InstanceToken`] using the provided `capabilities_proxy`.
        pub async fn new_with_proxy(capabilities_proxy: fruntime::CapabilitiesProxy) -> Self {
            let (handle, handle_other_end) = zx::EventPair::create();
            capabilities_proxy
                .instance_token_create(handle_other_end)
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect(
                    "this should be impossible, we passed a valid handle with the correct rights",
                );
            Self { handle }
        }
    }

    impl Clone for InstanceToken {
        fn clone(&self) -> Self {
            Self {
                handle: self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                    "failed to duplicate handle, please only use handles with the duplicate right",
                ),
            }
        }
    }

    /// A connector router may be used to request it produce a [`Connector`] capability. The router may
    /// decide to do so, decline to do so, or return an error, and it may rely on the contents of the
    /// `metadata` provided when `route` is called to do so. Routers may also delegate the request to
    /// other routers, often mutating `metadata` when they do.
    #[derive(Debug)]
    pub struct ConnectorRouter {
        /// The handle that references this capability
        pub handle: zx::EventPair,

        /// The proxy used to create this capability, and the proxy which will be used to perform
        /// operations on this capability.
        pub capabilities_proxy: fruntime::CapabilitiesProxy,
    }

    impl From<zx::EventPair> for ConnectorRouter {
        fn from(handle: zx::EventPair) -> Self {
            Self {
                handle,
                capabilities_proxy: connect_to_protocol::<fruntime::CapabilitiesMarker>()
                    .expect("failed to connect to fuchsia.component.runtime.Capabilities"),
            }
        }
    }

    impl ConnectorRouter {
        /// Creates a new [`ConnectorRouter`], connecting to
        /// `/svc/fuchsia.component.runtime.Capabilities` to do so.
        pub async fn new() -> (Self, ConnectorRouterReceiver) {
            let proxy = connect_to_protocol::<fruntime::CapabilitiesMarker>()
                .expect("failed to connect to fuchsia.component.runtime.Capabilities");
            Self::new_with_proxy(proxy).await
        }

        /// Creates a new [`ConnectorRouter`] using the provided `capabilities_proxy`.
        pub async fn new_with_proxy(
            capabilities_proxy: fruntime::CapabilitiesProxy,
        ) -> (Self, ConnectorRouterReceiver) {
            let (handle, handle_other_end) = zx::EventPair::create();
            let (client_end, stream) = create_request_stream::<fruntime::ConnectorRouterMarker>();
            capabilities_proxy
                .connector_router_create(handle_other_end, client_end)
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect(
                    "this should be impossible, we passed a valid handle with the correct rights",
                );
            let connector = Self { handle, capabilities_proxy };
            let receiver = ConnectorRouterReceiver { stream };
            (connector, receiver)
        }

        /// Associates `other_handle` with the same object referenced by this capability, so that
        /// whoever holds the other end of `other_handle` can refer to our capability.
        pub async fn associate_with_handle(&self, other_handle: zx::EventPair) {
            self.capabilities_proxy
                .capability_associate_handle(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    other_handle,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect("failed to clone onto handle, does it have sufficient rights?");
        }

        pub async fn route(
            &self,
            request: fruntime::RouteRequest,
            instance_token: &InstanceToken,
        ) -> Result<Option<Connector>, zx::Status> {
            let (connector, connector_other_end) = zx::EventPair::create();
            let res = self
                .capabilities_proxy
                .connector_router_route(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    &request,
                    instance_token.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    connector_other_end,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities");
            match res.map_err(zx::Status::err_from_raw)? {
                fruntime::RouterResponse::Success => Ok(Some(Connector {
                    handle: connector,
                    capabilities_proxy: self.capabilities_proxy.clone(),
                })),
                fruntime::RouterResponse::Unavailable => Ok(None),
                _ => Err(zx::Status::INTERNAL),
            }
        }
    }

    impl Clone for ConnectorRouter {
        fn clone(&self) -> Self {
            Self {
                handle: self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                    "failed to duplicate handle, please only use handles with the duplicate right",
                ),
                capabilities_proxy: self.capabilities_proxy.clone(),
            }
        }
    }

    /// A connector router receiver will receive requests for connector capabilities.
    pub struct ConnectorRouterReceiver {
        pub stream: fruntime::ConnectorRouterRequestStream,
    }

    impl From<fruntime::ConnectorRouterRequestStream> for ConnectorRouterReceiver {
        fn from(stream: fruntime::ConnectorRouterRequestStream) -> Self {
            Self { stream }
        }
    }

    impl Stream for ConnectorRouterReceiver {
        type Item = (
            fruntime::RouteRequest,
            InstanceToken,
            zx::EventPair,
            fruntime::ConnectorRouterRouteResponder,
        );

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let pinned_stream = pin!(&mut self.stream);
            match pinned_stream.poll_next(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Some(Ok(fruntime::ConnectorRouterRequest::Route {
                    request,
                    instance_token,
                    handle,
                    responder,
                }))) => {
                    let instance_token = InstanceToken { handle: instance_token };
                    Poll::Ready(Some((request, instance_token, handle, responder)))
                }
                _ => Poll::Ready(None),
            }
        }
    }

    impl ConnectorRouterReceiver {
        pub async fn handle_with<F>(mut self, f: F)
        where
            F: Fn(
                    fruntime::RouteRequest,
                    InstanceToken,
                ) -> BoxFuture<'static, Result<Option<Connector>, zx::Status>>
                + Sync
                + Send
                + 'static,
        {
            while let Some((request, instance_token, event_pair, responder)) = self.next().await {
                let res = match f(request, instance_token).await {
                    Ok(Some(connector)) => {
                        connector.associate_with_handle(event_pair).await;
                        Ok(fruntime::RouterResponse::Success)
                    }
                    Ok(None) => Ok(fruntime::RouterResponse::Unavailable),
                    Err(e) => Err(e.into_raw()),
                };
                let _ = responder.send(res);
            }
        }
    }

    /// A dir connector router may be used to request it produce a [`DirConnector`] capability. The
    /// router may decide to do so, decline to do so, or return an error, and it may rely on the
    /// contents of the `metadata` provided when `route` is called to do so. Routers may also delegate
    /// the request to other routers, often mutating `metadata` when they do.
    #[derive(Debug)]
    pub struct DirConnectorRouter {
        /// The handle that references this capability
        pub handle: zx::EventPair,

        /// The proxy used to create this capability, and the proxy which will be used to perform
        /// operations on this capability.
        pub capabilities_proxy: fruntime::CapabilitiesProxy,
    }

    impl From<zx::EventPair> for DirConnectorRouter {
        fn from(handle: zx::EventPair) -> Self {
            Self {
                handle,
                capabilities_proxy: connect_to_protocol::<fruntime::CapabilitiesMarker>()
                    .expect("failed to connect to fuchsia.component.runtime.Capabilities"),
            }
        }
    }

    impl DirConnectorRouter {
        /// Creates a new [`DirConnectorRouter`], connecting to
        /// `/svc/fuchsia.component.runtime.Capabilities` to do so.
        pub async fn new() -> (Self, DirConnectorRouterReceiver) {
            let proxy = connect_to_protocol::<fruntime::CapabilitiesMarker>()
                .expect("failed to connect to fuchsia.component.runtime.Capabilities");
            Self::new_with_proxy(proxy).await
        }

        /// Creates a new [`DirConnectorRouter`] using the provided `capabilities_proxy`.
        pub async fn new_with_proxy(
            capabilities_proxy: fruntime::CapabilitiesProxy,
        ) -> (Self, DirConnectorRouterReceiver) {
            let (handle, handle_other_end) = zx::EventPair::create();
            let (client_end, stream) =
                create_request_stream::<fruntime::DirConnectorRouterMarker>();
            capabilities_proxy
                .dir_connector_router_create(handle_other_end, client_end)
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect(
                    "this should be impossible, we passed a valid handle with the correct rights",
                );
            let connector = Self { handle, capabilities_proxy };
            let receiver = DirConnectorRouterReceiver { stream };
            (connector, receiver)
        }

        /// Associates `other_handle` with the same object referenced by this capability, so that
        /// whoever holds the other end of `other_handle` can refer to our capability.
        pub async fn associate_with_handle(&self, other_handle: zx::EventPair) {
            self.capabilities_proxy
                .capability_associate_handle(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    other_handle,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect("failed to clone onto handle, does it have sufficient rights?");
        }

        pub async fn route(
            &self,
            request: fruntime::RouteRequest,
            instance_token: &InstanceToken,
        ) -> Result<Option<DirConnector>, zx::Status> {
            let (connector, connector_other_end) = zx::EventPair::create();
            let res = self
                .capabilities_proxy
                .dir_connector_router_route(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    &request,
                    instance_token.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    connector_other_end,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities");
            match res.map_err(zx::Status::err_from_raw)? {
                fruntime::RouterResponse::Success => Ok(Some(DirConnector {
                    handle: connector,
                    capabilities_proxy: self.capabilities_proxy.clone(),
                })),
                fruntime::RouterResponse::Unavailable => Ok(None),
                _ => Err(zx::Status::INTERNAL),
            }
        }
    }

    impl Clone for DirConnectorRouter {
        fn clone(&self) -> Self {
            Self {
                handle: self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                    "failed to duplicate handle, please only use handles with the duplicate right",
                ),
                capabilities_proxy: self.capabilities_proxy.clone(),
            }
        }
    }

    /// A dir connector router receiver will receive requests for dir connector capabilities.
    pub struct DirConnectorRouterReceiver {
        pub stream: fruntime::DirConnectorRouterRequestStream,
    }

    impl From<fruntime::DirConnectorRouterRequestStream> for DirConnectorRouterReceiver {
        fn from(stream: fruntime::DirConnectorRouterRequestStream) -> Self {
            Self { stream }
        }
    }

    impl Stream for DirConnectorRouterReceiver {
        type Item = (
            fruntime::RouteRequest,
            InstanceToken,
            zx::EventPair,
            fruntime::DirConnectorRouterRouteResponder,
        );

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let pinned_stream = pin!(&mut self.stream);
            match pinned_stream.poll_next(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Some(Ok(fruntime::DirConnectorRouterRequest::Route {
                    request,
                    instance_token,
                    handle,
                    responder,
                }))) => {
                    let instance_token = InstanceToken { handle: instance_token };
                    Poll::Ready(Some((request, instance_token, handle, responder)))
                }
                _ => Poll::Ready(None),
            }
        }
    }

    impl DirConnectorRouterReceiver {
        pub async fn handle_with<F>(mut self, f: F)
        where
            F: Fn(
                    fruntime::RouteRequest,
                    InstanceToken,
                ) -> BoxFuture<'static, Result<Option<DirConnector>, zx::Status>>
                + Sync
                + Send
                + 'static,
        {
            while let Some((request, instance_token, event_pair, responder)) = self.next().await {
                let res = match f(request, instance_token).await {
                    Ok(Some(dictionary)) => {
                        dictionary.associate_with_handle(event_pair).await;
                        Ok(fruntime::RouterResponse::Success)
                    }
                    Ok(None) => Ok(fruntime::RouterResponse::Unavailable),
                    Err(e) => Err(e.into_raw()),
                };
                let _ = responder.send(res);
            }
        }
    }

    /// A dictionary router may be used to request it produce a [`Dictionary`] capability. The router
    /// may decide to do so, decline to do so, or return an error, and it may rely on the contents of
    /// the `metadata` provided when `route` is called to do so. Routers may also delegate the request
    /// to other routers, often mutating `metadata` when they do.
    #[derive(Debug)]
    pub struct DictionaryRouter {
        /// The handle that references this capability
        pub handle: zx::EventPair,

        /// The proxy used to create this capability, and the proxy which will be used to perform
        /// operations on this capability.
        pub capabilities_proxy: fruntime::CapabilitiesProxy,
    }

    impl From<zx::EventPair> for DictionaryRouter {
        fn from(handle: zx::EventPair) -> Self {
            Self {
                handle,
                capabilities_proxy: connect_to_protocol::<fruntime::CapabilitiesMarker>()
                    .expect("failed to connect to fuchsia.component.runtime.Capabilities"),
            }
        }
    }

    impl DictionaryRouter {
        /// Creates a new [`DictionaryRouter`], connecting to
        /// `/svc/fuchsia.component.runtime.Capabilities` to do so.
        pub async fn new() -> (Self, DictionaryRouterReceiver) {
            let proxy = connect_to_protocol::<fruntime::CapabilitiesMarker>()
                .expect("failed to connect to fuchsia.component.runtime.Capabilities");
            Self::new_with_proxy(proxy).await
        }

        /// Creates a new [`DictionaryRouter`] using the provided `capabilities_proxy`.
        pub async fn new_with_proxy(
            capabilities_proxy: fruntime::CapabilitiesProxy,
        ) -> (Self, DictionaryRouterReceiver) {
            let (handle, handle_other_end) = zx::EventPair::create();
            let (client_end, stream) = create_request_stream::<fruntime::DictionaryRouterMarker>();
            capabilities_proxy
                .dictionary_router_create(handle_other_end, client_end)
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect(
                    "this should be impossible, we passed a valid handle with the correct rights",
                );
            let connector = Self { handle, capabilities_proxy };
            let receiver = DictionaryRouterReceiver { stream };
            (connector, receiver)
        }

        /// Associates `other_handle` with the same object referenced by this capability, so that
        /// whoever holds the other end of `other_handle` can refer to our capability.
        pub async fn associate_with_handle(&self, other_handle: zx::EventPair) {
            self.capabilities_proxy
                .capability_associate_handle(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    other_handle,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect("failed to clone onto handle, does it have sufficient rights?");
        }

        pub async fn route(
            &self,
            request: fruntime::RouteRequest,
            instance_token: &InstanceToken,
        ) -> Result<Option<Dictionary>, zx::Status> {
            let (connector, connector_other_end) = zx::EventPair::create();
            let res = self
                .capabilities_proxy
                .dictionary_router_route(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    &request,
                    instance_token.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    connector_other_end,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities");
            match res.map_err(zx::Status::err_from_raw)? {
                fruntime::RouterResponse::Success => Ok(Some(Dictionary {
                    handle: connector,
                    capabilities_proxy: self.capabilities_proxy.clone(),
                })),
                fruntime::RouterResponse::Unavailable => Ok(None),
                _ => Err(zx::Status::INTERNAL),
            }
        }
    }

    impl Clone for DictionaryRouter {
        fn clone(&self) -> Self {
            Self {
                handle: self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                    "failed to duplicate handle, please only use handles with the duplicate right",
                ),
                capabilities_proxy: self.capabilities_proxy.clone(),
            }
        }
    }

    /// A dictionary router receiver will receive requests for dictionary capabilities.
    pub struct DictionaryRouterReceiver {
        pub stream: fruntime::DictionaryRouterRequestStream,
    }

    impl From<fruntime::DictionaryRouterRequestStream> for DictionaryRouterReceiver {
        fn from(stream: fruntime::DictionaryRouterRequestStream) -> Self {
            Self { stream }
        }
    }

    impl Stream for DictionaryRouterReceiver {
        type Item = (
            fruntime::RouteRequest,
            InstanceToken,
            zx::EventPair,
            fruntime::DictionaryRouterRouteResponder,
        );

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let pinned_stream = pin!(&mut self.stream);
            match pinned_stream.poll_next(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Some(Ok(fruntime::DictionaryRouterRequest::Route {
                    request,
                    instance_token,
                    handle,
                    responder,
                }))) => {
                    let instance_token = InstanceToken { handle: instance_token };
                    Poll::Ready(Some((request, instance_token, handle, responder)))
                }
                _ => Poll::Ready(None),
            }
        }
    }

    impl DictionaryRouterReceiver {
        pub async fn handle_with<F>(mut self, f: F)
        where
            F: Fn(
                    fruntime::RouteRequest,
                    InstanceToken,
                ) -> BoxFuture<'static, Result<Option<Dictionary>, zx::Status>>
                + Sync
                + Send
                + 'static,
        {
            while let Some((request, instance_token, event_pair, responder)) = self.next().await {
                let res = match f(request, instance_token).await {
                    Ok(Some(dictionary)) => {
                        dictionary.associate_with_handle(event_pair).await;
                        Ok(fruntime::RouterResponse::Success)
                    }
                    Ok(None) => Ok(fruntime::RouterResponse::Unavailable),
                    Err(e) => Err(e.into_raw()),
                };
                let _ = responder.send(res);
            }
        }
    }

    /// A data router may be used to request it produce a [`Data`] capability. The router may decide to
    /// do so, decline to do so, or return an error, and it may rely on the contents of the `metadata`
    /// provided when `route` is called to do so. Routers may also delegate the request to other
    /// routers, often mutating `metadata` when they do.
    #[derive(Debug)]
    pub struct DataRouter {
        /// The handle that references this capability
        pub handle: zx::EventPair,

        /// The proxy used to create this capability, and the proxy which will be used to perform
        /// operations on this capability.
        pub capabilities_proxy: fruntime::CapabilitiesProxy,
    }

    impl From<zx::EventPair> for DataRouter {
        fn from(handle: zx::EventPair) -> Self {
            Self {
                handle,
                capabilities_proxy: connect_to_protocol::<fruntime::CapabilitiesMarker>()
                    .expect("failed to connect to fuchsia.component.runtime.Capabilities"),
            }
        }
    }

    impl DataRouter {
        /// Creates a new [`DataRouter`], connecting to `/svc/fuchsia.component.runtime.Capabilities`
        /// to do so.
        pub async fn new() -> (Self, DataRouterReceiver) {
            let proxy = connect_to_protocol::<fruntime::CapabilitiesMarker>()
                .expect("failed to connect to fuchsia.component.runtime.Capabilities");
            Self::new_with_proxy(proxy).await
        }

        /// Creates a new [`DataRouter`] using the provided `capabilities_proxy`.
        pub async fn new_with_proxy(
            capabilities_proxy: fruntime::CapabilitiesProxy,
        ) -> (Self, DataRouterReceiver) {
            let (handle, handle_other_end) = zx::EventPair::create();
            let (client_end, stream) = create_request_stream::<fruntime::DataRouterMarker>();
            capabilities_proxy
                .data_router_create(handle_other_end, client_end)
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect(
                    "this should be impossible, we passed a valid handle with the correct rights",
                );
            let connector = Self { handle, capabilities_proxy };
            let receiver = DataRouterReceiver { stream };
            (connector, receiver)
        }

        /// Associates `other_handle` with the same object referenced by this capability, so that
        /// whoever holds the other end of `other_handle` can refer to our capability.
        pub async fn associate_with_handle(&self, other_handle: zx::EventPair) {
            self.capabilities_proxy
                .capability_associate_handle(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    other_handle,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities")
                .expect("failed to clone onto handle, does it have sufficient rights?");
        }

        pub async fn route(
            &self,
            request: fruntime::RouteRequest,
            instance_token: &InstanceToken,
        ) -> Result<Option<Data>, zx::Status> {
            let (connector, connector_other_end) = zx::EventPair::create();
            let res = self
                .capabilities_proxy
                .data_router_route(
                    self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    &request,
                    instance_token.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                        "failed to duplicate handle, please only use handles with the duplicate right",
                    ),
                    connector_other_end,
                )
                .await
                .expect("failed to use fuchsia.component.runtime.Capabilities");
            match res.map_err(zx::Status::err_from_raw)? {
                fruntime::RouterResponse::Success => Ok(Some(Data {
                    handle: connector,
                    capabilities_proxy: self.capabilities_proxy.clone(),
                })),
                fruntime::RouterResponse::Unavailable => Ok(None),
                _ => Err(zx::Status::INTERNAL),
            }
        }
    }

    impl Clone for DataRouter {
        fn clone(&self) -> Self {
            Self {
                handle: self.handle.duplicate_handle(zx::Rights::SAME_RIGHTS).expect(
                    "failed to duplicate handle, please only use handles with the duplicate right",
                ),
                capabilities_proxy: self.capabilities_proxy.clone(),
            }
        }
    }

    /// A data router receiver will receive requests for data capabilities.
    pub struct DataRouterReceiver {
        pub stream: fruntime::DataRouterRequestStream,
    }

    impl From<fruntime::DataRouterRequestStream> for DataRouterReceiver {
        fn from(stream: fruntime::DataRouterRequestStream) -> Self {
            Self { stream }
        }
    }

    impl Stream for DataRouterReceiver {
        type Item = (
            fruntime::RouteRequest,
            InstanceToken,
            zx::EventPair,
            fruntime::DataRouterRouteResponder,
        );

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let pinned_stream = pin!(&mut self.stream);
            match pinned_stream.poll_next(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Some(Ok(fruntime::DataRouterRequest::Route {
                    request,
                    instance_token,
                    handle,
                    responder,
                }))) => {
                    let instance_token = InstanceToken { handle: instance_token };
                    Poll::Ready(Some((request, instance_token, handle, responder)))
                }
                _ => Poll::Ready(None),
            }
        }
    }

    impl DataRouterReceiver {
        pub async fn handle_with<F>(mut self, f: F)
        where
            F: Fn(
                    fruntime::RouteRequest,
                    InstanceToken,
                ) -> BoxFuture<'static, Result<Option<Data>, zx::Status>>
                + Sync
                + Send
                + 'static,
        {
            while let Some((request, instance_token, event_pair, responder)) = self.next().await {
                let res = match f(request, instance_token).await {
                    Ok(Some(data)) => {
                        data.associate_with_handle(event_pair).await;
                        Ok(fruntime::RouterResponse::Success)
                    }
                    Ok(None) => Ok(fruntime::RouterResponse::Unavailable),
                    Err(e) => Err(e.into_raw()),
                };
                let _ = responder.send(res);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use assert_matches::assert_matches;
        use futures::StreamExt;
        use std::collections::HashSet;
        use zx::AsHandleRef;

        #[fuchsia::test]
        async fn connector_test() {
            let (connector, mut receiver) = Connector::new().await;
            let (c1, c2) = zx::Channel::create();
            connector.connect(c1).await.unwrap();
            let c1 = receiver.next().await.unwrap();
            assert_eq!(c1.basic_info().unwrap().koid, c2.basic_info().unwrap().related_koid);
        }

        #[fuchsia::test]
        async fn dir_connector_test() {
            let (dir_connector, mut dir_receiver) = DirConnector::new().await;
            let (client_end, server_end) =
                fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
            dir_connector
                .connect(server_end, Some(fio::PERM_WRITABLE), Some("foo/bar".to_string()))
                .await
                .unwrap();
            let dir_request = dir_receiver.next().await.unwrap();
            assert_eq!(
                client_end.as_handle_ref().basic_info().unwrap().koid,
                dir_request.channel.as_handle_ref().basic_info().unwrap().related_koid
            );
            assert_eq!("foo/bar", dir_request.path);
            assert_eq!(fio::PERM_WRITABLE, dir_request.flags);
        }

        #[fuchsia::test]
        async fn dictionary_test() {
            let dictionary = Dictionary::new().await;
            assert!(dictionary.get("foo").await.is_none());
            dictionary.insert("foo", Dictionary::new().await).await;
            assert_matches!(dictionary.get("foo").await, Some(Capability::Dictionary(_)));
            let mut keys_stream = dictionary.keys().await;
            assert_eq!(Some("foo".to_string()), keys_stream.next().await);
            assert_eq!(None, keys_stream.next().await);

            assert_matches!(dictionary.remove("foo").await, Some(Capability::Dictionary(_)));
            assert_matches!(dictionary.remove("foo").await, None);
            assert!(dictionary.get("foo").await.is_none());
        }

        #[fuchsia::test]
        async fn dictionary_key_iterator_many_keys_test() {
            let dictionary = Dictionary::new().await;

            // We want to make more keys than will fit in a single FIDL message.
            let mut keys = HashSet::new();
            for i in 0..zx::sys::ZX_CHANNEL_MAX_MSG_BYTES / 50 {
                keys.insert(format!("{:0100}", i));
            }
            for key in keys.iter() {
                dictionary.insert(key.as_str(), Dictionary::new().await).await;
            }

            let mut keys_stream = dictionary.keys().await;
            let mut returned_keys = HashSet::new();
            while let Some(key) = keys_stream.next().await {
                returned_keys.insert(key);
            }
            assert_eq!(keys, returned_keys);
        }

        #[fuchsia::test]
        async fn all_capabilities_into_and_out_of_a_dictionary_test() {
            let dictionary = Dictionary::new().await;
            let (connector, _receiver) = Connector::new().await;
            dictionary.insert("connector", connector).await;
            let (dir_connector, _dir_receiver) = DirConnector::new().await;
            dictionary.insert("dir_connector", dir_connector).await;
            dictionary.insert("dictionary", Dictionary::new().await).await;
            dictionary.insert("data", Data::new(DataValue::Int64(1)).await).await;
            let (connector_router, _connector_router_receiver) = ConnectorRouter::new().await;
            dictionary.insert("connector_router", connector_router).await;
            let (dir_connector_router, _dir_connector_router_receiver) =
                DirConnectorRouter::new().await;
            dictionary.insert("dir_connector_router", dir_connector_router).await;
            let (dictionary_router, _dictionary_router_receiver) = DictionaryRouter::new().await;
            dictionary.insert("dictionary_router", dictionary_router).await;
            let (data_router, _data_router_receiver) = DataRouter::new().await;
            dictionary.insert("data_router", data_router).await;
            dictionary.insert("instance_token", InstanceToken::new().await).await;

            assert_matches!(dictionary.get("connector").await, Some(Capability::Connector(_)));
            assert_matches!(
                dictionary.get("dir_connector").await,
                Some(Capability::DirConnector(_))
            );
            assert_matches!(dictionary.get("dictionary").await, Some(Capability::Dictionary(_)));
            assert_matches!(dictionary.get("data").await, Some(Capability::Data(_)));
            assert_matches!(
                dictionary.get("connector_router").await,
                Some(Capability::ConnectorRouter(_))
            );
            assert_matches!(
                dictionary.get("dir_connector_router").await,
                Some(Capability::DirConnectorRouter(_))
            );
            assert_matches!(
                dictionary.get("dictionary_router").await,
                Some(Capability::DictionaryRouter(_))
            );
            assert_matches!(dictionary.get("data_router").await, Some(Capability::DataRouter(_)));
            assert_matches!(
                dictionary.get("instance_token").await,
                Some(Capability::InstanceToken(_))
            );
        }

        #[fuchsia::test]
        async fn data_test() {
            let data = Data::new(DataValue::Uint64(100)).await;
            assert_eq!(DataValue::Uint64(100), data.get_value().await);
        }

        #[fuchsia::test]
        async fn connector_router_test() {
            let (connector_router, mut connector_router_receiver) = ConnectorRouter::new().await;

            let result_fut = fuchsia_async::Task::spawn(async move {
                let instance_token = InstanceToken::new().await;
                connector_router.route(fruntime::RouteRequest::default(), &instance_token).await
            });

            let (_route_request, _instance_token, handle, responder) =
                connector_router_receiver.next().await.unwrap();
            let (connector, mut receiver) = Connector::new().await;
            connector.associate_with_handle(handle).await;
            responder.send(Ok(fruntime::RouterResponse::Success)).unwrap();

            let connector = match result_fut.await {
                Ok(Some(connector)) => connector,
                other_value => panic!("unexpected route result: {other_value:?}"),
            };

            let (c1, c2) = zx::Channel::create();
            connector.connect(c1).await.unwrap();
            let c1 = receiver.next().await.unwrap();
            assert_eq!(c1.basic_info().unwrap().koid, c2.basic_info().unwrap().related_koid);
        }

        #[fuchsia::test]
        async fn dir_connector_router_test() {
            let (dir_connector_router, mut dir_connector_router_receiver) =
                DirConnectorRouter::new().await;

            let result_fut = fuchsia_async::Task::spawn(async move {
                let instance_token = InstanceToken::new().await;
                dir_connector_router.route(fruntime::RouteRequest::default(), &instance_token).await
            });

            let (_route_request, _instance_token, handle, responder) =
                dir_connector_router_receiver.next().await.unwrap();
            let (dir_connector, mut dir_receiver) = DirConnector::new().await;
            dir_connector.associate_with_handle(handle).await;
            responder.send(Ok(fruntime::RouterResponse::Success)).unwrap();

            let dir_connector = match result_fut.await {
                Ok(Some(dir_connector)) => dir_connector,
                other_value => panic!("unexpected route result: {other_value:?}"),
            };

            let (client_end, server_end) =
                fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
            dir_connector
                .connect(server_end, Some(fio::PERM_WRITABLE), Some("foo/bar".to_string()))
                .await
                .unwrap();
            let dir_request = dir_receiver.next().await.unwrap();
            assert_eq!(
                client_end.as_handle_ref().basic_info().unwrap().koid,
                dir_request.channel.as_handle_ref().basic_info().unwrap().related_koid
            );
            assert_eq!("foo/bar", dir_request.path);
            assert_eq!(fio::PERM_WRITABLE, dir_request.flags);
        }

        #[fuchsia::test]
        async fn dictionary_router_test() {
            let (dictionary_router, mut dictionary_router_receiver) = DictionaryRouter::new().await;

            let result_fut = fuchsia_async::Task::spawn(async move {
                let instance_token = InstanceToken::new().await;
                dictionary_router.route(fruntime::RouteRequest::default(), &instance_token).await
            });

            let (_route_request, _instance_token, handle, responder) =
                dictionary_router_receiver.next().await.unwrap();
            let dictionary = Dictionary::new().await;
            dictionary.insert("foo", Data::new(DataValue::String("bar".to_string())).await).await;
            dictionary.associate_with_handle(handle).await;
            responder.send(Ok(fruntime::RouterResponse::Success)).unwrap();

            let dictionary = match result_fut.await {
                Ok(Some(dictionary)) => dictionary,
                other_value => panic!("unexpected route result: {other_value:?}"),
            };

            let data = match dictionary.get("foo").await {
                Some(Capability::Data(data)) => data,
                other_value => panic!("unexpected dictionary contents: {other_value:?}"),
            };
            assert_eq!(data.get_value().await, DataValue::String("bar".to_string()));
        }
    }
}
