// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! A library for interacting with the `fuchsia.component.sandbox` FIDL APIs.
//!
//! This library provides type-safe wrappers around the raw FIDL types for
//! capabilities and the capability store, simplifying their use.

use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_io as fio;
use fuchsia_component::client;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

/// A trait for value types that can be imported into a [`CapabilityStore`], such as
/// String, u64, and fsandbox::DictionaryRef.
pub trait Importable<'a>: Sized + Into<Capability> + TryFrom<Capability, Error = Error> {
    /// The type of CapabilityRef that will be returned on import.
    type Ref: CapabilityRef<'a>;
}

/// An error that can occur when interacting with the sandbox APIs.
#[derive(Error, Clone, Debug)]
pub enum Error {
    /// Failed to connect to `fuchsia.component.sandbox.CapabilityStore`.
    #[error("failed to connect to fuchsia.component.sandbox/CapabilityStore: {0}")]
    FailedConnect(zx::Status),
    /// An operation expected a capability of a certain type, but received a different one.
    #[error("wrong Capability type; got \"{got}\", want \"{want}\"")]
    WrongType { got: &'static str, want: &'static str },
    /// A FIDL transport error occurred.
    #[error("FIDL error {0}")]
    Fidl(#[from] fidl::Error),
    /// An error returned from the `fuchsia.component.sandbox.CapabilityStore` protocol.
    #[error("CapabilityStoreError {0:?}")]
    CapabilityStore(fsandbox::CapabilityStoreError),
}

impl From<fsandbox::CapabilityStoreError> for Error {
    fn from(value: fsandbox::CapabilityStoreError) -> Self {
        Self::CapabilityStore(value)
    }
}

/// A helper struct to generate unused capability IDs.
///
/// This is clonable and thread-safe. There should generally be one of these
/// for each [`CapabilityStore`] connection.
#[derive(Clone, Default, Debug)]
pub struct CapabilityIdGenerator {
    next_id: Arc<AtomicU64>,
}

impl CapabilityIdGenerator {
    /// Creates a new ID generator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get the next free id.
    pub fn next(&self) -> u64 {
        self.range(1)
    }

    /// Get a range of free ids of size `size`.
    /// This returns the first id in the range.
    pub fn range(&self, size: u64) -> u64 {
        self.next_id.fetch_add(size, Ordering::Relaxed)
    }
}

/// A wrapper over [`fsandbox::CapabilityStoreProxy`] providing self-contained types.
#[derive(Clone, Debug)]
pub struct CapabilityStore {
    proxy: fsandbox::CapabilityStoreProxy,
    id_gen: CapabilityIdGenerator,
}

impl<'a> CapabilityStore {
    /// Connects to the `fuchsia.component.sandbox/CapabilityStore` protocol.
    pub fn connect() -> Result<Self, Error> {
        let proxy = client::connect_to_protocol::<fsandbox::CapabilityStoreMarker>().map_err(
            |e| match e.downcast_ref() {
                Some(status) => Error::FailedConnect(*status),
                None => Error::FailedConnect(zx::Status::INTERNAL),
            },
        )?;
        Ok(Self { proxy, id_gen: CapabilityIdGenerator::default() })
    }

    /// Imports a capability into the store.
    pub async fn import<T>(&'a self, value: T) -> Result<T::Ref, Error>
    where
        T: Importable<'a>,
    {
        let id = self.id_gen.next();
        self.proxy.import(id, value.into().0).await??;
        Ok(T::Ref::from_store(self, id))
    }

    /// Creates a new, empty dictionary in the capability store.
    pub async fn create_dictionary(&'a self) -> Result<Dictionary<'a>, Error> {
        let id = self.id_gen.next();
        self.proxy.dictionary_create(id).await??;
        Ok(Dictionary { store: self, id })
    }

    /// Creates a new [`Connector`] capability from a `Receiver` client end.
    pub async fn create_connector(
        &'a self,
        client_end: fidl::endpoints::ClientEnd<fsandbox::ReceiverMarker>,
    ) -> Result<Connector<'a>, Error> {
        let id = self.id_gen.next();
        self.proxy.connector_create(id, client_end).await??;
        Ok(Connector { store: self, id })
    }

    /// Creates a new [`DirConnector`] capability from a `DirReceiver` client end.
    pub async fn create_dir_connector(
        &'a self,
        client_end: fidl::endpoints::ClientEnd<fsandbox::DirReceiverMarker>,
    ) -> Result<DirConnector<'a>, Error> {
        let id = self.id_gen.next();
        self.proxy.dir_connector_create(id, client_end).await??;
        Ok(DirConnector { store: self, id })
    }
}

/// A reference to a [`fsandbox::Capability`] stored in a repository held by the
/// component framework.
///
/// This reference represents a capability of an unconfirmed type. Specify type
/// parameters on `export()` methods to derive the type.
pub trait CapabilityRef<'a>: Sized {
    /// Create a [`CapabilityRef`] referencing the given store and id.
    ///
    /// Warning: If the capability stored in the store at the given key is not
    /// of type `T`, the type mismatch will only become apparent when attempting
    /// to call [`CapabilityRef::export`] or call type-specific methods on it.
    fn from_store(store: &'a CapabilityStore, id: fsandbox::CapabilityId) -> Self;

    /// Return a reference to the store containing this Capability.
    fn store(&self) -> &'a CapabilityStore;

    /// Return the identifier for this Capability.
    fn id(&self) -> fsandbox::CapabilityId;

    /// Duplicates the capability referenced by this ref, returning a new
    /// ref to the duplicated capability.
    fn duplicate(&'a self) -> impl Future<Output = Result<Self, Error>> {
        async move {
            let dup_id = self.store().id_gen.next();
            self.store().proxy.duplicate(self.id(), dup_id).await??;
            Ok(Self::from_store(self.store(), dup_id))
        }
    }

    /// Drop the referenced Capability held by the component framework runtime.
    fn drop(self) -> impl Future<Output = Result<(), Error>> {
        async move { Ok(self.store().proxy.drop(self.id()).await??) }
    }

    /// Extract the value of the referenced Capability and remove it from the
    /// component framework runtime.
    fn export<T>(self) -> impl Future<Output = Result<T, Error>>
    where
        T: Importable<'a, Ref = Self>,
    {
        async move {
            let cap = self.store().proxy.export(self.id()).await??;
            T::try_from(cap.into())
        }
    }
}

/// A type-safe wrapper for a [`fsandbox::Capability`].
///
/// This enum is used to represent different kinds of capabilities that can be
/// stored in the [`CapabilityStore`]. See the [`From`] implementations for how to
/// create a [`Capability`] from a specific type like [`Dictionary`] or [`Data`].
#[derive(Debug)]
pub struct Capability(fsandbox::Capability);

impl From<Capability> for fsandbox::Capability {
    fn from(value: Capability) -> Self {
        value.0
    }
}

impl From<fsandbox::Capability> for Capability {
    fn from(value: fsandbox::Capability) -> Self {
        Self(value)
    }
}

/// Returns the type name of a capability as a static string.
fn capability_type_name(cap: fsandbox::Capability) -> &'static str {
    match cap {
        fsandbox::Capability::Unit(_) => "Unit",
        fsandbox::Capability::Handle(_) => "Handle",
        fsandbox::Capability::Data(data) => match data {
            fsandbox::Data::Bytes(_) => "Data::Bytes",
            fsandbox::Data::String(_) => "Data::String",
            fsandbox::Data::Int64(_) => "Data::Int64",
            fsandbox::Data::Uint64(_) => "Data::Uint64",
            _ => "Data::Unknown",
        },
        fsandbox::Capability::Dictionary(_) => "Dictionary",
        fsandbox::Capability::Connector(_) => "Connector",
        fsandbox::Capability::DirConnector(_) => "DirConnector",
        fsandbox::Capability::Directory(_) => "Directory",
        fsandbox::Capability::DirEntry(_) => "DirEntry",
        fsandbox::Capability::ConnectorRouter(_) => "ConnectorRouter",
        fsandbox::Capability::DictionaryRouter(_) => "DictionaryRouter",
        fsandbox::Capability::DirEntryRouter(_) => "DirEntryRouter",
        fsandbox::Capability::DataRouter(_) => "DataRouter",
        fsandbox::Capability::DirConnectorRouter(_) => "DirConnectorRouter",
        _ => "Unknown",
    }
}

/// Generate a struct that implements [`CapabilityRef`]. Optionally generate
/// a [`Importable`] implementation for `Item`.
macro_rules! impl_capability_ref {
    // Generate struct and impls for Item.
    (
        $(#[$attr:meta])*
        $ref:ident, $t:ty, $variant:path
    ) => {
        impl_capability_ref!(
            $(#[$attr])*
            $ref
        );

        impl<'a> Importable<'a> for $t {
            type Ref = $ref<'a>;
        }

        impl From<$t> for Capability {
            fn from(value: $t) -> Self {
                Self($variant(value))
            }
        }

        impl TryFrom<Capability> for $t {
            type Error = Error;
            fn try_from(value: Capability) -> Result<Self, Self::Error> {
                match value.0 {
                    $variant(inner) => Ok(inner),
                    capability => Err(Error::WrongType {
                        got: capability_type_name(capability),
                        want: stringify!($t),
                    }),
                }
            }
        }
    };
    // Generate struct.
    (
        $(#[$attr:meta])*
        $ref:ident
    ) => {
        $(#[$attr])*
        #[derive(Debug)]
        pub struct $ref<'a> {
            store: &'a CapabilityStore,
            id: fsandbox::CapabilityId,
        }

        impl<'a> CapabilityRef<'a> for $ref<'a> {
            fn from_store(store: &'a CapabilityStore, id: fsandbox::CapabilityId) -> Self {
                Self { store, id }
            }

            fn store(&self) -> &'a CapabilityStore {
                self.store
            }

            fn id(&self) -> fsandbox::CapabilityId {
                self.id
            }
        }
    };
}

impl_capability_ref!(
    /// A reference to a Unit capability in the store.
    Unit, fsandbox::Unit, fsandbox::Capability::Unit
);

impl_capability_ref!(
    /// A reference to a Zircon handle capability in the store.
    Handle, fidl::NullableHandle, fsandbox::Capability::Handle
);

impl_capability_ref!(
    /// A reference to a Data capability in the store.
    ///
    /// The underlying Data in the component framework runtime holds primitive
    /// types like strings, integers, and byte vectors.
    Data
);

impl_capability_ref!(
    /// A reference to a Dictionary capability in the store.
    ///
    /// Component framework dictionaries are mutable collections of other
    /// [`fsandbox::Capability`], keyed by strings.
    Dictionary, fsandbox::DictionaryRef, fsandbox::Capability::Dictionary
);

impl<'a> Dictionary<'a> {
    /// Inserts a capability into the dictionary with the given key.
    pub async fn insert<V>(&self, key: impl Into<String>, value: V) -> Result<(), Error>
    where
        V: CapabilityRef<'a>,
    {
        self.store
            .proxy
            .dictionary_insert(
                self.id,
                &fsandbox::DictionaryItem { key: key.into(), value: value.id() },
            )
            .await??;
        Ok(())
    }

    /// Retrieves a capability from the dictionary by its key.
    pub async fn get<T>(&self, key: impl AsRef<str>) -> Result<T, Error>
    where
        T: CapabilityRef<'a>,
    {
        let id = self.store.id_gen.next();
        self.store.proxy.dictionary_get(self.id, key.as_ref(), id).await??;
        Ok(T::from_store(self.store, id))
    }
}

impl_capability_ref!(
    /// A reference to a Connector capability in the store.
    ///
    /// A [`Connector`] can be used to receive a channel from a client component.
    Connector, fsandbox::Connector, fsandbox::Capability::Connector
);

impl<'a> Connector<'a> {
    /// Open the underlying connection to `Receiver`.
    pub async fn open(
        &self,
        server_end: fidl::endpoints::ServerEnd<fsandbox::ReceiverMarker>,
    ) -> Result<(), Error> {
        Ok(self.store.proxy.connector_open(self.id, server_end.into_channel()).await??)
    }
}

impl_capability_ref!(
    /// A reference to a DirConnector capability in the store.
    DirConnector, fsandbox::DirConnector, fsandbox::Capability::DirConnector
);

impl<'a> DirConnector<'a> {
    /// Open the underlying connection to `Directory`.
    pub async fn open(
        &self,
        server_end: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    ) -> Result<(), Error> {
        Ok(self
            .store
            .proxy
            .dir_connector_open(fsandbox::CapabilityStoreDirConnectorOpenRequest {
                id: Some(self.id),
                server_end: Some(server_end),
                flags: None,
                path: None,
                ..Default::default()
            })
            .await??)
    }
}

impl_capability_ref!(
    /// A reference to a Directory capability in the store.
    Directory, fidl::endpoints::ClientEnd<fio::DirectoryMarker>, fsandbox::Capability::Directory
);

impl_capability_ref!(
    /// A reference to a DirEntry capability in the store.
    DirEntry, fsandbox::DirEntry, fsandbox::Capability::DirEntry
);

impl_capability_ref!(
    /// A reference to a ConnectorRouter capability in the store.
    ConnectorRouter, fidl::endpoints::ClientEnd<fsandbox::ConnectorRouterMarker>, fsandbox::Capability::ConnectorRouter
);

impl_capability_ref!(
    /// A reference to a DictionaryRouter capability in the store.
    DictionaryRouter, fidl::endpoints::ClientEnd<fsandbox::DictionaryRouterMarker>, fsandbox::Capability::DictionaryRouter
);

impl_capability_ref!(
    /// A reference to a DirEntryRouter capability in the store.
    DirEntryRouter, fidl::endpoints::ClientEnd<fsandbox::DirEntryRouterMarker>, fsandbox::Capability::DirEntryRouter
);

impl_capability_ref!(
    /// A reference to a DataRouter capability in the store.
    DataRouter, fidl::endpoints::ClientEnd<fsandbox::DataRouterMarker>, fsandbox::Capability::DataRouter
);

impl_capability_ref!(
    /// A reference to a DirConnectorRouter capability in the store.
    DirConnectorRouter, fidl::endpoints::ClientEnd<fsandbox::DirConnectorRouterMarker>, fsandbox::Capability::DirConnectorRouter
);

/// Implement TryFrom on [`fsandbox::Capability::Data`] variants.
macro_rules! impl_data_capability {
    ($t:ty, $variant:path, $variant_type_name:expr) => {
        impl<'a> Importable<'a> for $t {
            type Ref = Data<'a>;
        }

        impl From<$t> for Capability {
            fn from(value: $t) -> Self {
                Self(fsandbox::Capability::Data($variant(value)))
            }
        }

        impl TryFrom<Capability> for $t {
            type Error = Error;
            fn try_from(value: Capability) -> Result<Self, Self::Error> {
                match value.0 {
                    fsandbox::Capability::Data($variant(inner)) => Ok(inner),
                    capability => Err(Error::WrongType {
                        got: capability_type_name(capability),
                        want: $variant_type_name,
                    }),
                }
            }
        }
    };
}

impl_data_capability!(Vec<u8>, fsandbox::Data::Bytes, "Data::Bytes");
impl_data_capability!(String, fsandbox::Data::String, "Data::String");
impl_data_capability!(i64, fsandbox::Data::Int64, "Data::Int64");
impl_data_capability!(u64, fsandbox::Data::Uint64, "Data::Uint64");

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fidl::endpoints::create_proxy_and_stream;
    use fuchsia_async as fasync;
    use futures::future::FutureExt;
    use futures::prelude::*;
    use futures::task::Poll;
    use std::pin::Pin;
    use test_case::test_case;
    use zx::AsHandleRef;

    fn setup_store() -> (CapabilityStore, fsandbox::CapabilityStoreRequestStream) {
        let (proxy, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let store = CapabilityStore { proxy, id_gen: CapabilityIdGenerator::default() };
        (store, stream)
    }

    async fn wait_for_request<F, T>(f: &mut Pin<Box<F>>)
    where
        F: Future<Output = T> + ?Sized,
        T: std::fmt::Debug,
    {
        assert_matches!(fasync::TestExecutor::poll_until_stalled(f.as_mut()).await, Poll::Pending);
    }

    #[test]
    fn capability_id_generator() {
        let id_gen = CapabilityIdGenerator::new();
        assert_eq!(id_gen.next(), 0);
        assert_eq!(id_gen.next(), 1);
        assert_eq!(id_gen.range(5), 2);
        assert_eq!(id_gen.next(), 7);
    }

    #[test_case("hello".to_string(); "string")]
    #[test_case(123u64; "u64")]
    #[test_case(-123i64; "i64")]
    #[test_case(vec![1, 2, 3]; "bytes")]
    fn data_capability_conversions<T>(val: T)
    where
        T: for<'a> Importable<'a, Ref = Data<'a>> + Clone + PartialEq + std::fmt::Debug,
    {
        let cap: Capability = val.clone().into();
        let val_again: T = cap.try_into().unwrap();
        assert_eq!(val_again, val);
    }

    #[test]
    fn data_capability_conversions_wrong_type() {
        let cap: Capability = "hello".to_string().into();
        let res: Result<u64, _> = cap.try_into();
        assert_matches!(res, Err(Error::WrongType { got: "Data::String", want: "Data::Uint64" }));
    }

    #[test_case("hello".to_string(); "string")]
    #[test_case(123u64; "u64")]
    #[test_case(-123i64; "i64")]
    #[test_case(vec![1, 2, 3]; "bytes")]
    #[fuchsia::test(allow_stalls = false)]
    async fn capability_store_import_export<T>(val: T)
    where
        T: for<'a> Importable<'a, Ref: std::fmt::Debug> + Clone + PartialEq + std::fmt::Debug,
    {
        let (store, mut stream) = setup_store();

        // Test import
        let mut import_fut = store.import(val.clone()).boxed_local();
        wait_for_request(&mut import_fut).await;

        let (capability, responder) = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::Import { id, capability, responder } if id == 0 => (capability, responder)
        );

        // Check that the imported capability matches the value.
        let imported_val: T = Capability(capability).try_into().unwrap();
        assert_eq!(imported_val, val);

        responder.send(Ok(())).unwrap();
        let cap = import_fut.await.unwrap();
        assert_eq!(cap.id(), 0);

        // Test export
        let mut export_fut = cap.export::<T>().boxed_local();
        wait_for_request(&mut export_fut).await;

        let responder = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::Export { id, responder } if id == 0 => responder
        );

        let cap_to_return: Capability = val.clone().into();
        responder.send(Ok(cap_to_return.into())).unwrap();

        let result = export_fut.await.unwrap();
        assert_eq!(result, val);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn capability_ref_duplicate() {
        let (store, mut stream) = setup_store();

        // Simulate an existing capability with id 100
        let data = Data { id: 100, store: &store };

        let mut duplicate_fut = data.duplicate().boxed_local();
        wait_for_request(&mut duplicate_fut).await;

        let responder = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::Duplicate { id, dest_id, responder } if id == 100 && dest_id == 0 => responder
        );

        responder.send(Ok(())).unwrap();
        let new_data: Data<'_> = duplicate_fut.await.unwrap();
        assert_eq!(new_data.id, 0);
        assert_eq!(store.id_gen.next(), 1); // next id is 1
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn capability_ref_drop() {
        let (store, mut stream) = setup_store();
        let data = Data { id: 100, store: &store };

        let mut drop_fut = data.drop().boxed_local();
        wait_for_request(&mut drop_fut).await;

        let responder = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::Drop { id, responder } if id == 100 => responder
        );

        responder.send(Ok(())).unwrap();
        drop_fut.await.unwrap();
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn dictionary_create() {
        let (store, mut stream) = setup_store();

        let mut create_fut = store.create_dictionary().boxed_local();
        wait_for_request(&mut create_fut).await;

        let responder = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::DictionaryCreate { id, responder } if id == 0 => responder
        );

        responder.send(Ok(())).unwrap();
        let dict = create_fut.await.unwrap();
        assert_eq!(dict.id, 0);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn dictionary_insert() {
        let (store, mut stream) = setup_store();
        let dict = Dictionary { id: 10, store: &store };
        let data = Data { id: 20, store: &store };

        let mut insert_fut = dict.insert("key", data).boxed_local();
        wait_for_request(&mut insert_fut).await;

        let responder = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::DictionaryInsert { id, item, responder } if id == 10 && item.key == "key" && item.value == 20 => responder
        );

        responder.send(Ok(())).unwrap();
        insert_fut.await.unwrap();
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn dictionary_get() {
        let (store, mut stream) = setup_store();
        let dict = Dictionary { id: 10, store: &store };

        let mut get_fut = dict.get::<Data<'_>>("key").boxed_local();
        wait_for_request(&mut get_fut).await;

        let responder = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::DictionaryGet { id, key, dest_id, responder } if id == 10 && key == "key" && dest_id == 0 => responder
        );

        responder.send(Ok(())).unwrap();
        let data = get_fut.await.unwrap();
        assert_eq!(data.id, 0);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn dictionary_export() {
        let (store, mut stream) = setup_store();
        let dict = Dictionary { id: 10, store: &store };

        let mut export_fut = dict.export().boxed_local();
        wait_for_request(&mut export_fut).await;

        let responder = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::Export { id, responder } if id == 10 => responder
        );

        let (client, _server) = zx::EventPair::create();
        let cap = fsandbox::Capability::Dictionary(fsandbox::DictionaryRef { token: client });
        responder.send(Ok(cap)).unwrap();

        let result: fsandbox::DictionaryRef = export_fut.await.unwrap();
        assert_eq!(result.token.as_handle_ref().is_invalid(), false);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connector_create() {
        let (store, mut stream) = setup_store();
        let (client_end, _server_end) =
            fidl::endpoints::create_endpoints::<fsandbox::ReceiverMarker>();

        let mut create_fut = store.create_connector(client_end).boxed_local();
        wait_for_request(&mut create_fut).await;

        let responder = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::ConnectorCreate { id, receiver, responder } if id == 0 && receiver.as_handle_ref().is_invalid() == false => responder
        );

        responder.send(Ok(())).unwrap();
        let connector = create_fut.await.unwrap();
        assert_eq!(connector.id, 0);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn connector_open() {
        let (store, mut stream) = setup_store();
        let connector = Connector { id: 10, store: &store };

        let (client_end, server_end) =
            fidl::endpoints::create_endpoints::<fsandbox::ReceiverMarker>();
        let mut open_fut = connector.open(server_end).boxed_local();
        wait_for_request(&mut open_fut).await;

        let responder = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::ConnectorOpen { id, server_end, responder } if id == 10 && server_end.as_handle_ref().is_invalid() == false => responder
        );

        responder.send(Ok(())).unwrap();
        open_fut.await.unwrap();
        assert_eq!(client_end.as_handle_ref().is_invalid(), false);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn dir_connector_create() {
        let (store, mut stream) = setup_store();
        let (client_end, _server_end) =
            fidl::endpoints::create_endpoints::<fsandbox::DirReceiverMarker>();

        let mut create_fut = store.create_dir_connector(client_end).boxed_local();
        wait_for_request(&mut create_fut).await;

        let responder = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::DirConnectorCreate { id, receiver, responder } if id == 0 && receiver.as_handle_ref().is_invalid() == false => responder
        );

        responder.send(Ok(())).unwrap();
        let connector = create_fut.await.unwrap();
        assert_eq!(connector.id, 0);
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn dir_connector_open() {
        let (store, mut stream) = setup_store();
        let connector = DirConnector { id: 10, store: &store };

        let (client_end, server_end) = fidl::endpoints::create_endpoints::<fio::DirectoryMarker>();
        let mut open_fut = connector.open(server_end).boxed_local();
        wait_for_request(&mut open_fut).await;

        let responder = assert_matches!(
            stream.next().await.unwrap().unwrap(),
            fsandbox::CapabilityStoreRequest::DirConnectorOpen { payload, responder } if payload.id == Some(10) && payload.server_end.as_ref().expect("server_end").as_handle_ref().is_invalid() == false => responder
        );

        responder.send(Ok(())).unwrap();
        open_fut.await.unwrap();
        assert_eq!(client_end.as_handle_ref().is_invalid(), false);
    }
}
