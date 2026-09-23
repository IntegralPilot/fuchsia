// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::dictionary::{EntryUpdate, UpdateNotifierRetention};
use crate::fidl::registry::{self, try_from_handle_in_registry};
use crate::{Capability, ConversionError, Dictionary, RemoteError, WeakInstanceToken};
use fidl_fuchsia_component_sandbox as fsandbox;
use fuchsia_sync::Mutex;
use futures::FutureExt;
use futures::channel::oneshot;
use log::warn;
use std::sync::{Arc, Weak};
use vfs::directory::entry::DirectoryEntry;
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::immutable::simple as pfs;
use vfs::execution_scope::ExecutionScope;
use vfs::name::Name;

impl crate::fidl::IntoFsandboxCapability for Arc<Dictionary> {
    fn into_fsandbox_capability(self, _token: Arc<WeakInstanceToken>) -> fsandbox::Capability {
        fsandbox::Capability::Dictionary(fsandbox::DictionaryRef {
            token: registry::insert_token(self.into()),
        })
    }
}

impl Dictionary {
    // Conversion from legacy channel type.
    pub fn from_channel(dict: fidl::Channel) -> Result<Arc<Self>, RemoteError> {
        let any = try_from_handle_in_registry(dict.as_handle_ref())?;
        let Capability::Dictionary(dict) = any else {
            panic!("BUG: registry has a non-Dictionary capability under a Dictionary koid");
        };
        Ok(dict)
    }

    pub fn try_from_fsandbox(
        dictionary: fsandbox::DictionaryRef,
    ) -> Result<Arc<Self>, RemoteError> {
        let any = try_from_handle_in_registry(dictionary.token.as_handle_ref())?;
        let Capability::Dictionary(dictionary) = any else {
            panic!("BUG: registry has a non-dictionary capability under a dictionary koid");
        };
        Ok(dictionary)
    }

    pub fn to_fsandbox(self: Arc<Self>) -> fsandbox::DictionaryRef {
        fsandbox::DictionaryRef { token: registry::insert_token(self.into()) }
    }

    /// Like [CapabilityBound::try_into_directory_entry], but this version actually consumes
    /// the contents of the [Dictionary]. In other words, if this function returns `Ok`, `self`
    /// will be empty. If any items are added to `self` later, they will not appear in the
    /// directory. This method is useful when the caller has no need to keep the original
    /// [Dictionary]. Note that even if there is only one reference to the [Dictionary], calling
    /// [CapabilityBound::try_into_directory_entry] does not have the same effect because the
    /// `vfs` keeps alive reference to the [Dictionary] -- see the comment in the implementation.
    ///
    /// This is transitive: any [Dictionary]s nested in this one will be consumed as well.
    pub fn try_into_directory_entry_oneshot(
        self: Arc<Self>,
        scope: ExecutionScope,
        token: Arc<WeakInstanceToken>,
    ) -> Result<Arc<dyn DirectoryEntry>, ConversionError> {
        let directory = if let Some(handler) = self.lock().not_found.take() {
            pfs::Simple::new_with_not_found_handler(handler)
        } else {
            pfs::Simple::new()
        };
        for (key, value) in self.drain() {
            let dir_entry = match value {
                Capability::Dictionary(value) => {
                    value.try_into_directory_entry_oneshot(scope.clone(), token.clone())?
                }
                value => value.try_into_directory_entry(scope.clone(), token.clone())?,
            };
            let key =
                Name::try_from(key.to_string()).expect("cm_types::Name is always a valid vfs Name");
            directory
                .add_entry_impl(key, dir_entry, false)
                .expect("dictionary values must be unique")
        }

        Ok(directory)
    }

    pub(crate) fn try_into_directory_entry_inner(
        self: Arc<Self>,
        scope: ExecutionScope,
        token: Arc<WeakInstanceToken>,
    ) -> Result<Arc<dyn DirectoryEntry>, ConversionError> {
        let self_clone = self.clone();
        let directory = pfs::Simple::new_with_not_found_handler(move |path| {
            // We hold a reference to the dictionary in this closure to solve an ownership problem.
            // In `try_into_directory_entry` we return a `pfs::Simple` that provides a directory
            // projection of a dictionary. The directory is live-updated, so that as items are
            // added to or removed from the dictionary the directory contents are updated to match.
            //
            // The live-updating semantics introduce a problem: when all references to a dictionary
            // reach the end of their lifetime and the dictionary is dropped, all entries in the
            // dictionary are marked as removed. This means if one creates a dictionary, adds
            // entries to it, turns it into a directory, and drops the only dictionary reference,
            // then the directory is immediately emptied of all of its contents.
            //
            // Ideally at least one reference to the dictionary would be kept alive as long as the
            // directory exists. We accomplish that by giving the directory ownership over a
            // reference to the dictionary here.
            self_clone.not_found(path);
        });
        let weak_dir: Weak<pfs::Simple> = Arc::downgrade(&directory);
        let (error_sender, error_receiver) = oneshot::channel();
        let error_sender = Mutex::new(Some(error_sender));
        // `register_update_notifier` calls the closure with any existing entries before returning,
        // so there won't be a race with us returning this directory and the entries being added to
        // it.
        self.register_update_notifier(Box::new(move |update: EntryUpdate<'_>| {
            let Some(directory) = weak_dir.upgrade() else {
                return UpdateNotifierRetention::Drop_;
            };
            match update {
                EntryUpdate::Add(key, value) => {
                    let dir_entry = match value
                        .clone()
                        .try_into_directory_entry(scope.clone(), token.clone())
                    {
                        Ok(dir_entry) => dir_entry,
                        Err(err) => {
                            if let Some(error_sender) = error_sender.lock().take() {
                                let _ = error_sender.send(err);
                            } else {
                                warn!(
                                    "value in dictionary cannot be converted to directory entry: \
                                    {err:?}"
                                )
                            }
                            return UpdateNotifierRetention::Retain;
                        }
                    };
                    let name = Name::try_from(key.to_string())
                        .expect("cm_types::Name is always a valid vfs Name");
                    directory
                        .add_entry_impl(name, dir_entry, false)
                        .expect("dictionary values must be unique")
                }
                EntryUpdate::Remove(key) => {
                    let name = Name::try_from(key.to_string())
                        .expect("cm_types::Name is always a valid vfs Name");
                    let _ = directory.remove_entry_impl(name, false);
                }
                EntryUpdate::Idle => (),
            }
            UpdateNotifierRetention::Retain
        }));
        if let Some(Ok(error)) = error_receiver.now_or_never() {
            // We encountered an error processing the initial contents of this dictionary. Let's
            // return that instead of the directory we've created.
            return Err(error);
        }
        Ok(directory)
    }
}

// These tests only run on target because the vfs library is not generally available on host.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityBound;
    use crate::dictionary::{
        BorrowedKey, HYBRID_SWITCH_INSERTION_LEN, HYBRID_SWITCH_REMOVAL_LEN, HybridMap, Key,
    };
    use crate::fidl::IntoFsandboxCapability;
    use crate::{Data, Dictionary, DirConnector, Handle, serve_capability_store};
    use assert_matches::assert_matches;
    use fidl::endpoints::{Proxy, create_proxy, create_proxy_and_stream};
    use fidl::handle::{Channel, Status};
    use fidl_fuchsia_io as fio;
    use fuchsia_async as fasync;
    use fuchsia_fs::directory;
    use futures::StreamExt;
    use std::sync::LazyLock;
    use std::{fmt, iter};
    use test_case::test_case;
    use test_util::Counter;
    use vfs::directory::entry::{
        DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest, serve_directory,
    };
    use vfs::execution_scope::ExecutionScope;
    use vfs::path::Path;
    use vfs::remote::RemoteLike;
    use vfs::{ObjectRequestRef, pseudo_directory};

    static CAP_KEY: LazyLock<Key> = LazyLock::new(|| "Cap".parse().unwrap());

    #[derive(Debug, Clone, Copy)]
    enum TestType {
        // Test dictionary stored as vector
        Small,
        // Test dictionary stored as map
        Big,
    }

    #[fuchsia::test]
    async fn create() {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });
        let id_gen = sandbox::CapabilityIdGenerator::new();

        let dict_id = id_gen.next();
        assert_matches!(store.dictionary_create(dict_id).await.unwrap(), Ok(()));
        assert_matches!(
            store.dictionary_create(dict_id).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdAlreadyExists)
        );

        let value = 10;
        store.import(value, Data::Int64(1).into()).await.unwrap().unwrap();
        store
            .dictionary_insert(dict_id, &fsandbox::DictionaryItem { key: "k".into(), value })
            .await
            .unwrap()
            .unwrap();

        // The dictionary has one item.
        let (iterator, server_end) = create_proxy();
        store.dictionary_keys(dict_id, server_end).await.unwrap().unwrap();
        let keys = iterator.get_next().await.unwrap();
        assert!(iterator.get_next().await.unwrap().is_empty());
        assert_eq!(keys, ["k"]);
    }

    #[fuchsia::test]
    async fn create_error() {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let cap = Capability::Data(Data::Int64(42));
        assert_matches!(
            store
                .import(1, cap.into_fsandbox_capability(WeakInstanceToken::new_invalid()))
                .await
                .unwrap(),
            Ok(())
        );
        assert_matches!(
            store.dictionary_create(1).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdAlreadyExists)
        );
    }

    #[fuchsia::test]
    async fn legacy_import() {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let dict_id = 1;
        assert_matches!(store.dictionary_create(dict_id).await.unwrap(), Ok(()));
        assert_matches!(
            store.dictionary_create(dict_id).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdAlreadyExists)
        );

        let value = 10;
        store.import(value, Data::Int64(1).into()).await.unwrap().unwrap();
        store
            .dictionary_insert(dict_id, &fsandbox::DictionaryItem { key: "k".into(), value })
            .await
            .unwrap()
            .unwrap();

        // Export and re-import the capability using the legacy import/export APIs.
        let (client, server) = fidl::Channel::create();
        store.dictionary_legacy_export(dict_id, server).await.unwrap().unwrap();
        let dict_id = 2;
        store.dictionary_legacy_import(dict_id, client).await.unwrap().unwrap();

        // The dictionary has one item.
        let (iterator, server_end) = create_proxy();
        store.dictionary_keys(dict_id, server_end).await.unwrap().unwrap();
        let keys = iterator.get_next().await.unwrap();
        assert!(iterator.get_next().await.unwrap().is_empty());
        assert_eq!(keys, ["k"]);
    }

    #[fuchsia::test]
    async fn legacy_import_error() {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        store.dictionary_create(10).await.unwrap().unwrap();
        let (dict_ch, server) = fidl::Channel::create();
        store.dictionary_legacy_export(10, server).await.unwrap().unwrap();

        let cap1 = Capability::Data(Data::Int64(42));
        store
            .import(1, cap1.into_fsandbox_capability(WeakInstanceToken::new_invalid()))
            .await
            .unwrap()
            .unwrap();
        assert_matches!(
            store.dictionary_legacy_import(1, dict_ch).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdAlreadyExists)
        );

        let (ch, _) = fidl::Channel::create();
        assert_matches!(
            store.dictionary_legacy_import(2, ch.into()).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::BadCapability)
        );
    }

    #[fuchsia::test]
    async fn legacy_export_error() {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let (_dict_ch, server) = fidl::Channel::create();
        assert_matches!(
            store.dictionary_legacy_export(1, server).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdNotFound)
        );
    }

    #[test_case(TestType::Small)]
    #[test_case(TestType::Big)]
    #[fuchsia::test]
    async fn insert(test_type: TestType) {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let dict = new_dict(test_type);
        let dict_ref = Capability::Dictionary(dict.clone())
            .into_fsandbox_capability(WeakInstanceToken::new_invalid());
        let dict_id = 1;
        store.import(dict_id, dict_ref).await.unwrap().unwrap();

        let data = Data::Int64(1).into();
        let value = 2;
        store.import(value, data).await.unwrap().unwrap();
        store
            .dictionary_insert(
                dict_id,
                &fsandbox::DictionaryItem { key: CAP_KEY.to_string(), value },
            )
            .await
            .unwrap()
            .unwrap();

        // Inserting adds the entry to `entries`.
        assert_eq!(adjusted_len(&dict, test_type), 1);

        // The entry that was inserted should now be in `entries`.
        let cap = dict.remove(&*CAP_KEY).expect("not in entries after insert");
        let Capability::Data(data) = cap else { panic!("Bad capability type: {:#?}", cap) };
        assert_eq!(&data, &Data::Int64(1));
    }

    #[fuchsia::test]
    async fn insert_error() {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let data = Data::Int64(1).into();
        let value = 2;
        store.import(value, data).await.unwrap().unwrap();

        assert_matches!(
            store
                .dictionary_insert(1, &fsandbox::DictionaryItem { key: "k".into(), value })
                .await
                .unwrap(),
            Err(fsandbox::CapabilityStoreError::IdNotFound)
        );
        assert_matches!(
            store
                .dictionary_insert(2, &fsandbox::DictionaryItem { key: "k".into(), value })
                .await
                .unwrap(),
            Err(fsandbox::CapabilityStoreError::WrongType)
        );

        store.dictionary_create(1).await.unwrap().unwrap();
        assert_matches!(
            store
                .dictionary_insert(1, &fsandbox::DictionaryItem { key: "^bad".into(), value })
                .await
                .unwrap(),
            Err(fsandbox::CapabilityStoreError::InvalidKey)
        );

        assert_matches!(
            store
                .dictionary_insert(1, &fsandbox::DictionaryItem { key: "k".into(), value })
                .await
                .unwrap(),
            Ok(())
        );

        let data = Data::Int64(1).into();
        let value = 3;
        store.import(value, data).await.unwrap().unwrap();
        assert_matches!(
            store
                .dictionary_insert(1, &fsandbox::DictionaryItem { key: "k".into(), value })
                .await
                .unwrap(),
            Err(fsandbox::CapabilityStoreError::ItemAlreadyExists)
        );
    }

    #[test_case(TestType::Small)]
    #[test_case(TestType::Big)]
    #[fuchsia::test]
    async fn remove(test_type: TestType) {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let dict = new_dict(test_type);

        // Insert a Data into the Dictionary.
        assert!(dict.insert(CAP_KEY.clone(), Capability::Data(Data::Int64(1))).is_none());
        assert_eq!(adjusted_len(&dict, test_type), 1);

        let dict_ref = Capability::Dictionary(dict.clone())
            .into_fsandbox_capability(WeakInstanceToken::new_invalid());
        let dict_id = 1;
        store.import(dict_id, dict_ref).await.unwrap().unwrap();

        let dest_id = 2;
        store
            .dictionary_remove(
                dict_id,
                &CAP_KEY.to_string(),
                Some(&fsandbox::WrappedNewCapabilityId { id: dest_id }),
            )
            .await
            .unwrap()
            .unwrap();
        let cap = store.export(dest_id).await.unwrap().unwrap();
        // The value should be the same one that was previously inserted.
        assert_eq!(cap, Data::Int64(1).into());

        // Removing the entry with Remove should remove it from `entries`.
        assert_eq!(adjusted_len(&dict, test_type), 0);
    }

    #[fuchsia::test]
    async fn remove_error() {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        assert_matches!(
            store.dictionary_remove(1, "k".into(), None).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdNotFound)
        );

        store.dictionary_create(1).await.unwrap().unwrap();

        let data = Data::Int64(1).into();
        store.import(2, data).await.unwrap().unwrap();

        assert_matches!(
            store.dictionary_remove(2, "k".into(), None).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::WrongType)
        );
        store
            .dictionary_insert(1, &fsandbox::DictionaryItem { key: "k".into(), value: 2 })
            .await
            .unwrap()
            .unwrap();
        assert_matches!(
            store
                .dictionary_remove(1, "k".into(), Some(&fsandbox::WrappedNewCapabilityId { id: 1 }))
                .await
                .unwrap(),
            Err(fsandbox::CapabilityStoreError::IdAlreadyExists)
        );
        assert_matches!(
            store.dictionary_remove(1, "^bad".into(), None).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::InvalidKey)
        );
        assert_matches!(
            store.dictionary_remove(1, "not_found".into(), None).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::ItemNotFound)
        );
    }

    #[test_case(TestType::Small)]
    #[test_case(TestType::Big)]
    #[fuchsia::test]
    async fn get(test_type: TestType) {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let dict = new_dict(test_type);

        assert!(dict.insert(CAP_KEY.clone(), Capability::Data(Data::Int64(1))).is_none());
        assert_eq!(adjusted_len(&dict, test_type), 1);
        let (ch, _) = fidl::Channel::create();
        let handle = Handle::new(ch.into_handle());
        assert!(dict.insert("h".parse().unwrap(), Capability::Handle(handle)).is_none());

        let dict_ref = Capability::Dictionary(dict.clone())
            .into_fsandbox_capability(WeakInstanceToken::new_invalid());
        let dict_id = 1;
        store.import(dict_id, dict_ref).await.unwrap().unwrap();

        let dest_id = 2;
        store.dictionary_get(dict_id, CAP_KEY.as_str(), dest_id).await.unwrap().unwrap();
        let cap = store.export(dest_id).await.unwrap().unwrap();
        assert_eq!(cap, Data::Int64(1).into());
    }

    #[fuchsia::test]
    async fn get_error() {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        assert_matches!(
            store.dictionary_get(1, "k".into(), 2).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdNotFound)
        );

        store.dictionary_create(1).await.unwrap().unwrap();

        store.import(2, Data::Int64(1).into()).await.unwrap().unwrap();

        assert_matches!(
            store.dictionary_get(2, "k".into(), 3).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::WrongType)
        );
        store
            .dictionary_insert(1, &fsandbox::DictionaryItem { key: "k".into(), value: 2 })
            .await
            .unwrap()
            .unwrap();

        store.import(2, Data::Int64(1).into()).await.unwrap().unwrap();
        assert_matches!(
            store.dictionary_get(1, "k".into(), 2).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdAlreadyExists)
        );
        assert_matches!(
            store.dictionary_get(1, "^bad".into(), 3).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::InvalidKey)
        );
        assert_matches!(
            store.dictionary_get(1, "not_found".into(), 3).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::ItemNotFound)
        );
    }

    #[test_case(TestType::Small)]
    #[test_case(TestType::Big)]
    #[fuchsia::test]
    async fn copy(test_type: TestType) {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        // Create a Dictionary with a Data inside, and copy the Dictionary.
        let dict = new_dict(test_type);
        assert!(dict.insert("data1".parse().unwrap(), Capability::Data(Data::Int64(1))).is_none());
        store
            .import(
                1,
                dict.clone().into_fsandbox_capability(WeakInstanceToken::new_invalid()).into(),
            )
            .await
            .unwrap()
            .unwrap();
        store.dictionary_copy(1, 2).await.unwrap().unwrap();

        // Insert a Data into the copy.
        store.import(3, Data::Int64(1).into()).await.unwrap().unwrap();
        store
            .dictionary_insert(2, &fsandbox::DictionaryItem { key: "k".into(), value: 3 })
            .await
            .unwrap()
            .unwrap();

        // The copy should have two Data values.
        let copy = store.export(2).await.unwrap().unwrap();
        let copy = Capability::try_from(copy).unwrap();
        let Capability::Dictionary(copy) = copy else { panic!() };
        {
            assert_eq!(adjusted_len(&copy, test_type), 2);
            let copy = copy.lock();
            assert!(copy.entries.iter().all(|(_, value)| matches!(value, Capability::Data(_))));
        }

        // The original Dictionary should have only one Data.
        {
            assert_eq!(adjusted_len(&dict, test_type), 1);
            let dict = dict.lock();
            assert!(dict.entries.iter().all(|(_, value)| matches!(value, Capability::Data(_))));
        }
    }

    #[test_case(TestType::Small)]
    #[test_case(TestType::Big)]
    #[fuchsia::test]
    async fn duplicate(test_type: TestType) {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let dict = new_dict(test_type);
        store
            .import(1, dict.clone().into_fsandbox_capability(WeakInstanceToken::new_invalid()))
            .await
            .unwrap()
            .unwrap();
        store.duplicate(1, 2).await.unwrap().unwrap();

        // Add a Data into the duplicate.
        store.import(3, Data::Int64(1).into()).await.unwrap().unwrap();
        store
            .dictionary_insert(2, &fsandbox::DictionaryItem { key: "k".into(), value: 3 })
            .await
            .unwrap()
            .unwrap();
        let dict_dup = store.export(2).await.unwrap().unwrap();
        let dict_dup = Capability::try_from(dict_dup).unwrap();
        let Capability::Dictionary(dict_dup) = dict_dup else { panic!() };
        assert_eq!(adjusted_len(&dict_dup, test_type), 1);

        // The original dict should now have an entry because it shares entries with the clone.
        assert_eq!(adjusted_len(&dict_dup, test_type), 1);
    }

    /// Tests basic functionality of read APIs.
    #[test_case(TestType::Small)]
    #[test_case(TestType::Big)]
    #[fuchsia::test]
    async fn read(test_type: TestType) {
        let dict = new_dict(test_type);
        let id_gen = sandbox::CapabilityIdGenerator::new();

        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });
        let dict_ref =
            Capability::Dictionary(dict).into_fsandbox_capability(WeakInstanceToken::new_invalid());
        let dict_id = id_gen.next();
        store.import(dict_id, dict_ref).await.unwrap().unwrap();

        // Create two Data capabilities.
        let mut data_caps = vec![];
        for i in 1..3 {
            let id = id_gen.next();
            store.import(id, Data::Int64(i.try_into().unwrap()).into()).await.unwrap().unwrap();
            data_caps.push(id);
        }

        // Add the Data capabilities to the dict.
        store
            .dictionary_insert(
                dict_id,
                &fsandbox::DictionaryItem { key: "Cap1".into(), value: data_caps.remove(0) },
            )
            .await
            .unwrap()
            .unwrap();
        store
            .dictionary_insert(
                dict_id,
                &fsandbox::DictionaryItem { key: "Cap2".into(), value: data_caps.remove(0) },
            )
            .await
            .unwrap()
            .unwrap();
        let (ch, _) = fidl::Channel::create();
        let handle = ch.into_handle();
        let id = id_gen.next();
        store.import(id, fsandbox::Capability::Handle(handle)).await.unwrap().unwrap();
        store
            .dictionary_insert(dict_id, &fsandbox::DictionaryItem { key: "Cap3".into(), value: id })
            .await
            .unwrap()
            .unwrap();

        // Keys
        {
            let (iterator, server_end) = create_proxy();
            store.dictionary_keys(dict_id, server_end).await.unwrap().unwrap();
            let keys = iterator.get_next().await.unwrap();
            assert!(iterator.get_next().await.unwrap().is_empty());
            match test_type {
                TestType::Small => assert_eq!(keys, ["Cap1", "Cap2", "Cap3"]),
                TestType::Big => {
                    assert_eq!(keys[0..3], ["Cap1", "Cap2", "Cap3"]);
                    assert_eq!(keys.len(), 3 + HYBRID_SWITCH_INSERTION_LEN);
                }
            }
        }
        // Enumerate
        {
            let (iterator, server_end) = create_proxy();
            store.dictionary_enumerate(dict_id, server_end).await.unwrap().unwrap();
            let start_id = 100;
            let ofs: u32 = match test_type {
                TestType::Small => 0,
                TestType::Big => HYBRID_SWITCH_INSERTION_LEN as u32,
            };
            let limit = 4 + ofs;
            let (mut items, end_id) = iterator.get_next(start_id, limit).await.unwrap().unwrap();
            assert_eq!(end_id, 103 + ofs as u64);
            let (last, end_id) = iterator.get_next(end_id, limit).await.unwrap().unwrap();
            assert!(last.is_empty());
            assert_eq!(end_id, 103 + ofs as u64);

            assert_matches!(
                items.remove(0),
                fsandbox::DictionaryOptionalItem {
                    key,
                    value: Some(value)
                }
                if key == "Cap1" && value.id == 100
            );
            assert_matches!(
                store.export(100).await.unwrap().unwrap(),
                fsandbox::Capability::Data(fsandbox::Data::Int64(1))
            );
            assert_matches!(
                items.remove(0),
                fsandbox::DictionaryOptionalItem {
                    key,
                    value: Some(value)
                }
                if key == "Cap2" && value.id == 101
            );
            assert_matches!(
                store.export(101).await.unwrap().unwrap(),
                fsandbox::Capability::Data(fsandbox::Data::Int64(2))
            );
            assert_matches!(
                items.remove(0),
                fsandbox::DictionaryOptionalItem {
                    key,
                    value: Some(value)
                }
                if key == "Cap3" && value.id == 102
            );
            match test_type {
                TestType::Small => {}
                TestType::Big => {
                    assert_eq!(items.len(), HYBRID_SWITCH_INSERTION_LEN);
                }
            }
        }
    }

    #[test_case(TestType::Small)]
    #[test_case(TestType::Big)]
    #[fuchsia::test]
    async fn drain(test_type: TestType) {
        let dict = new_dict(test_type);

        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });
        let dict_ref = Capability::Dictionary(dict.clone())
            .into_fsandbox_capability(WeakInstanceToken::new_invalid());
        let dict_id = 1;
        store.import(dict_id, dict_ref).await.unwrap().unwrap();

        // Create two Data capabilities.
        let mut data_caps = vec![];
        for i in 1..3 {
            let value = 10 + i;
            store.import(value, Data::Int64(i.try_into().unwrap()).into()).await.unwrap().unwrap();
            data_caps.push(value);
        }

        // Add the Data capabilities to the dict.
        store
            .dictionary_insert(
                dict_id,
                &fsandbox::DictionaryItem { key: "Cap1".into(), value: data_caps.remove(0) },
            )
            .await
            .unwrap()
            .unwrap();
        store
            .dictionary_insert(
                dict_id,
                &fsandbox::DictionaryItem { key: "Cap2".into(), value: data_caps.remove(0) },
            )
            .await
            .unwrap()
            .unwrap();
        let (ch, _) = fidl::Channel::create();
        let handle = ch.into_handle();
        let handle_koid = handle.koid().unwrap();
        let value = 20;
        store.import(value, fsandbox::Capability::Handle(handle)).await.unwrap().unwrap();
        store
            .dictionary_insert(dict_id, &fsandbox::DictionaryItem { key: "Cap3".into(), value })
            .await
            .unwrap()
            .unwrap();

        let (iterator, server_end) = create_proxy();
        store.dictionary_drain(dict_id, Some(server_end)).await.unwrap().unwrap();
        let ofs: u32 = match test_type {
            TestType::Small => 0,
            TestType::Big => HYBRID_SWITCH_INSERTION_LEN as u32,
        };
        let start_id = 100;
        let limit = 4 + ofs;
        let (mut items, end_id) = iterator.get_next(start_id, limit).await.unwrap().unwrap();
        assert_eq!(end_id, 103 + ofs as u64);
        let (last, end_id) = iterator.get_next(end_id, limit).await.unwrap().unwrap();
        assert!(last.is_empty());
        assert_eq!(end_id, 103 + ofs as u64);

        assert_matches!(
            items.remove(0),
            fsandbox::DictionaryItem {
                key,
                value: 100
            }
            if key == "Cap1"
        );
        assert_matches!(
            store.export(100).await.unwrap().unwrap(),
            fsandbox::Capability::Data(fsandbox::Data::Int64(1))
        );
        assert_matches!(
            items.remove(0),
            fsandbox::DictionaryItem {
                key,
                value: 101
            }
            if key == "Cap2"
        );
        assert_matches!(
            store.export(101).await.unwrap().unwrap(),
            fsandbox::Capability::Data(fsandbox::Data::Int64(2))
        );
        assert_matches!(
            items.remove(0),
            fsandbox::DictionaryItem {
                key,
                value: 102
            }
            if key == "Cap3"
        );
        assert_matches!(
            store.export(102).await.unwrap().unwrap(),
            fsandbox::Capability::Handle(handle)
            if handle.koid().unwrap() == handle_koid
        );

        // Dictionary should now be empty.
        assert!(dict.is_empty());
    }

    /// Tests batching for read APIs.
    #[fuchsia::test]
    async fn read_batches() {
        // Number of entries in the Dictionary that will be enumerated.
        //
        // This value was chosen such that that GetNext returns multiple chunks of different sizes.
        const NUM_ENTRIES: u32 = fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK * 2 + 1;

        // Number of items we expect in each chunk, for every chunk we expect to get.
        const EXPECTED_CHUNK_LENGTHS: &[u32] =
            &[fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK, fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK, 1];

        let id_gen = sandbox::CapabilityIdGenerator::new();

        // Create a Dictionary with [NUM_ENTRIES] entries that have Data values.
        let dict = Dictionary::new();
        for i in 0..NUM_ENTRIES {
            assert!(
                dict.insert(format!("{}", i).parse().unwrap(), Capability::Data(Data::Int64(1)))
                    .is_none()
            );
        }

        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });
        let dict_ref = Capability::Dictionary(dict.clone())
            .into_fsandbox_capability(WeakInstanceToken::new_invalid());
        let dict_id = id_gen.next();
        store.import(dict_id, dict_ref).await.unwrap().unwrap();

        let (key_iterator, server_end) = create_proxy();
        store.dictionary_keys(dict_id, server_end).await.unwrap().unwrap();
        let (item_iterator, server_end) = create_proxy();
        store.dictionary_enumerate(dict_id, server_end).await.unwrap().unwrap();

        // Get all the entries from the Dictionary with `GetNext`.
        let mut num_got_items: u32 = 0;
        let mut start_id = 100;
        let limit = fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK;
        for expected_len in EXPECTED_CHUNK_LENGTHS {
            let keys = key_iterator.get_next().await.unwrap();
            let (items, end_id) = item_iterator.get_next(start_id, limit).await.unwrap().unwrap();
            if keys.is_empty() && items.is_empty() {
                break;
            }
            assert_eq!(*expected_len, keys.len() as u32);
            assert_eq!(*expected_len, items.len() as u32);
            assert_eq!(u64::from(*expected_len), end_id - start_id);
            start_id = end_id;
            num_got_items += *expected_len;
        }

        // GetNext should return no items once all items have been returned.
        let (items, _) = item_iterator.get_next(start_id, limit).await.unwrap().unwrap();
        assert!(items.is_empty());
        assert!(key_iterator.get_next().await.unwrap().is_empty());

        assert_eq!(num_got_items, NUM_ENTRIES);
    }

    #[fuchsia::test]
    async fn drain_batches() {
        // Number of entries in the Dictionary that will be enumerated.
        //
        // This value was chosen such that that GetNext returns multiple chunks of different sizes.
        const NUM_ENTRIES: u32 = fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK * 2 + 1;

        // Number of items we expect in each chunk, for every chunk we expect to get.
        const EXPECTED_CHUNK_LENGTHS: &[u32] =
            &[fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK, fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK, 1];

        // Create a Dictionary with [NUM_ENTRIES] entries that have Data values.
        let dict = Dictionary::new();
        for i in 0..NUM_ENTRIES {
            assert!(
                dict.insert(format!("{}", i).parse().unwrap(), Capability::Data(Data::Int64(1)))
                    .is_none()
            );
        }

        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });
        let dict_ref = Capability::Dictionary(dict.clone())
            .into_fsandbox_capability(WeakInstanceToken::new_invalid());
        let dict_id = 1;
        store.import(dict_id, dict_ref).await.unwrap().unwrap();

        let (item_iterator, server_end) = create_proxy();
        store.dictionary_drain(dict_id, Some(server_end)).await.unwrap().unwrap();

        // Get all the entries from the Dictionary with `GetNext`.
        let mut num_got_items: u32 = 0;
        let mut start_id = 100;
        let limit = fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK;
        for expected_len in EXPECTED_CHUNK_LENGTHS {
            let (items, end_id) = item_iterator.get_next(start_id, limit).await.unwrap().unwrap();
            if items.is_empty() {
                break;
            }
            assert_eq!(*expected_len, items.len() as u32);
            assert_eq!(u64::from(*expected_len), end_id - start_id);
            start_id = end_id;
            num_got_items += *expected_len;
        }

        // GetNext should return no items once all items have been returned.
        let (items, _) = item_iterator.get_next(start_id, limit).await.unwrap().unwrap();
        assert!(items.is_empty());

        assert_eq!(num_got_items, NUM_ENTRIES);

        // Dictionary should now be empty.
        assert!(dict.is_empty());
    }

    #[fuchsia::test]
    async fn read_error() {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        store.import(2, Data::Int64(1).into()).await.unwrap().unwrap();

        let (_, server_end) = create_proxy();
        assert_matches!(
            store.dictionary_keys(1, server_end).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdNotFound)
        );
        let (_, server_end) = create_proxy();
        assert_matches!(
            store.dictionary_enumerate(1, server_end).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdNotFound)
        );
        assert_matches!(
            store.dictionary_drain(1, None).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdNotFound)
        );

        let (_, server_end) = create_proxy();
        assert_matches!(
            store.dictionary_keys(2, server_end).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::WrongType)
        );
        let (_, server_end) = create_proxy();
        assert_matches!(
            store.dictionary_enumerate(2, server_end).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::WrongType)
        );
        assert_matches!(
            store.dictionary_drain(2, None).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::WrongType)
        );
    }

    #[fuchsia::test]
    async fn read_iterator_error() {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        store.dictionary_create(1).await.unwrap().unwrap();

        {
            let (iterator, server_end) = create_proxy();
            store.dictionary_enumerate(1, server_end).await.unwrap().unwrap();
            assert_matches!(
                iterator.get_next(2, fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK + 1).await.unwrap(),
                Err(fsandbox::CapabilityStoreError::InvalidArgs)
            );
            let (iterator, server_end) = create_proxy();
            store.dictionary_enumerate(1, server_end).await.unwrap().unwrap();
            assert_matches!(
                iterator.get_next(2, 0).await.unwrap(),
                Err(fsandbox::CapabilityStoreError::InvalidArgs)
            );

            let (iterator, server_end) = create_proxy();
            store.dictionary_drain(1, Some(server_end)).await.unwrap().unwrap();
            assert_matches!(
                iterator.get_next(2, fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK + 1).await.unwrap(),
                Err(fsandbox::CapabilityStoreError::InvalidArgs)
            );
            let (iterator, server_end) = create_proxy();
            store.dictionary_drain(1, Some(server_end)).await.unwrap().unwrap();
            assert_matches!(
                iterator.get_next(2, 0).await.unwrap(),
                Err(fsandbox::CapabilityStoreError::InvalidArgs)
            );
        }

        store.import(4, Data::Int64(1).into()).await.unwrap().unwrap();
        for i in 0..3 {
            store.import(2, Data::Int64(1).into()).await.unwrap().unwrap();
            store
                .dictionary_insert(1, &fsandbox::DictionaryItem { key: format!("k{i}"), value: 2 })
                .await
                .unwrap()
                .unwrap();
        }

        // Range overlaps with id 4
        {
            let (iterator, server_end) = create_proxy();
            store.dictionary_enumerate(1, server_end).await.unwrap().unwrap();
            assert_matches!(
                iterator.get_next(2, 3).await.unwrap(),
                Err(fsandbox::CapabilityStoreError::IdAlreadyExists)
            );

            let (iterator, server_end) = create_proxy();
            store.dictionary_drain(1, Some(server_end)).await.unwrap().unwrap();
            assert_matches!(
                iterator.get_next(2, 3).await.unwrap(),
                Err(fsandbox::CapabilityStoreError::IdAlreadyExists)
            );
        }
    }

    #[fuchsia::test]
    async fn try_into_open_error_not_supported() {
        let dict = Dictionary::new();
        assert!(dict.insert(CAP_KEY.clone(), Capability::Data(Data::Int64(1))).is_none());
        let scope = ExecutionScope::new();
        assert_matches!(
            dict.try_into_directory_entry(scope, WeakInstanceToken::new_invalid()).err(),
            Some(ConversionError::NotSupported)
        );
    }

    struct MockDir(Counter);
    impl DirectoryEntry for MockDir {
        fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
            request.open_remote(self)
        }
    }
    impl GetEntryInfo for MockDir {
        fn entry_info(&self) -> EntryInfo {
            EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
        }
    }
    impl RemoteLike for MockDir {
        fn open(
            self: Arc<Self>,
            _scope: ExecutionScope,
            relative_path: Path,
            _flags: fio::Flags,
            _object_request: ObjectRequestRef<'_>,
        ) -> Result<(), Status> {
            assert_eq!(relative_path.as_ref(), "bar");
            self.0.inc();
            Ok(())
        }
    }

    #[fuchsia::test]
    async fn try_into_open_success() {
        let dict = Dictionary::new();
        let mock_dir = Arc::new(MockDir(Counter::new(0)));
        assert!(
            dict.insert(
                CAP_KEY.clone(),
                DirConnector::from_directory_entry(mock_dir.clone(), fio::PERM_READABLE).into(),
            )
            .is_none()
        );
        let scope = ExecutionScope::new();
        let remote =
            dict.try_into_directory_entry(scope.clone(), WeakInstanceToken::new_invalid()).unwrap();

        let dir_client_end = serve_directory(remote.clone(), &scope, fio::PERM_READABLE).unwrap();

        assert_eq!(mock_dir.0.get(), 0);
        let (client_end, server_end) = Channel::create();
        let dir = dir_client_end.channel();
        fdio::service_connect_at(dir, &format!("{}/bar", *CAP_KEY), server_end).unwrap();
        fasync::Channel::from_channel(client_end).on_closed().await.unwrap();
        assert_eq!(mock_dir.0.get(), 1);
    }

    #[fuchsia::test]
    async fn try_into_open_success_nested() {
        let inner_dict = Dictionary::new();
        let mock_dir = Arc::new(MockDir(Counter::new(0)));
        assert!(
            inner_dict
                .insert(
                    CAP_KEY.clone(),
                    DirConnector::from_directory_entry(mock_dir.clone(), fio::PERM_READABLE).into(),
                )
                .is_none()
        );
        let dict = Dictionary::new();
        assert!(dict.insert(CAP_KEY.clone(), Capability::Dictionary(inner_dict)).is_none());

        let scope = ExecutionScope::new();
        let remote = dict
            .try_into_directory_entry(scope.clone(), WeakInstanceToken::new_invalid())
            .expect("convert dict into Open capability");

        let dir_client_end = serve_directory(remote.clone(), &scope, fio::PERM_READABLE).unwrap();

        // List the outer directory and verify the contents.
        let dir = dir_client_end.into_proxy();
        assert_eq!(
            fuchsia_fs::directory::readdir(&dir).await.unwrap(),
            vec![directory::DirEntry {
                name: CAP_KEY.to_string(),
                kind: fio::DirentType::Directory
            },]
        );

        // Open the inner most capability.
        assert_eq!(mock_dir.0.get(), 0);
        let (client_end, server_end) = Channel::create();
        let dir = dir.into_channel().unwrap().into_zx_channel();
        fdio::service_connect_at(&dir, &format!("{}/{}/bar", *CAP_KEY, *CAP_KEY), server_end)
            .unwrap();
        fasync::Channel::from_channel(client_end).on_closed().await.unwrap();
        assert_eq!(mock_dir.0.get(), 1)
    }

    #[fuchsia::test]
    async fn switch_between_vec_and_map() {
        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let dict = Dictionary::new();
        let dict_ref = Capability::Dictionary(dict.clone())
            .into_fsandbox_capability(WeakInstanceToken::new_invalid());
        let dict_id = 1;
        store.import(dict_id, dict_ref).await.unwrap().unwrap();

        // Just one less one the switchover point. Count down instead of up to test sorting.
        {
            for i in (1..=HYBRID_SWITCH_INSERTION_LEN - 1).rev() {
                let data = Data::Int64(1).into();
                let value = (i + 10) as u64;
                store.import(value, data).await.unwrap().unwrap();
                store
                    .dictionary_insert(
                        dict_id,
                        &fsandbox::DictionaryItem { key: key_for(i).into(), value },
                    )
                    .await
                    .unwrap()
                    .unwrap();
            }

            let entries = &dict.lock().entries;
            let HybridMap::Vec(v) = entries else { panic!() };
            v.iter().for_each(|(_, v)| assert_matches!(v, Capability::Data(_)));
            let actual_keys: Vec<Key> = v.iter().map(|(k, _)| k.clone()).collect();
            let expected: Vec<Key> =
                (1..=HYBRID_SWITCH_INSERTION_LEN - 1).map(|i| key_for(i)).collect();
            assert_eq!(actual_keys, expected);
        }

        // Add one more, and the switch happens.
        {
            let i = HYBRID_SWITCH_INSERTION_LEN;
            let data = Data::Int64(1).into();
            let value = (i + 10) as u64;
            store.import(value, data).await.unwrap().unwrap();
            store
                .dictionary_insert(
                    dict_id,
                    &fsandbox::DictionaryItem { key: key_for(i).into(), value },
                )
                .await
                .unwrap()
                .unwrap();

            let entries = &dict.lock().entries;
            let HybridMap::Map(m) = entries else { panic!() };
            m.iter().for_each(|(_, m)| assert_matches!(m, Capability::Data(_)));
            let actual_keys: Vec<Key> = m.iter().map(|(k, _)| k.clone()).collect();
            let expected: Vec<Key> =
                (1..=HYBRID_SWITCH_INSERTION_LEN).map(|i| key_for(i)).collect();
            assert_eq!(actual_keys, expected);
        }

        // Now go in reverse: remove just one less than the switchover point for removal.
        {
            for i in (HYBRID_SWITCH_INSERTION_LEN - HYBRID_SWITCH_REMOVAL_LEN + 1
                ..=HYBRID_SWITCH_INSERTION_LEN)
                .rev()
            {
                store.dictionary_remove(dict_id, key_for(i).as_str(), None).await.unwrap().unwrap();
            }

            let entries = &dict.lock().entries;
            let HybridMap::Map(m) = entries else { panic!() };
            m.iter().for_each(|(_, v)| assert_matches!(v, Capability::Data(_)));
            let actual_keys: Vec<Key> = m.iter().map(|(k, _)| k.clone()).collect();
            let expected: Vec<Key> =
                (1..=HYBRID_SWITCH_REMOVAL_LEN + 1).map(|i| key_for(i)).collect();
            assert_eq!(actual_keys, expected);
        }

        // Finally, remove one more, and it switches back to a map.
        {
            let i = HYBRID_SWITCH_REMOVAL_LEN + 1;
            store.dictionary_remove(dict_id, key_for(i).as_str(), None).await.unwrap().unwrap();

            let entries = &dict.lock().entries;
            let HybridMap::Vec(v) = entries else { panic!() };
            v.iter().for_each(|(_, v)| assert_matches!(v, Capability::Data(_)));
            let actual_keys: Vec<Key> = v.iter().map(|(k, _)| k.clone()).collect();
            let expected: Vec<Key> = (1..=HYBRID_SWITCH_REMOVAL_LEN).map(|i| key_for(i)).collect();
            assert_eq!(actual_keys, expected);
        }
    }

    #[test_case(TestType::Small)]
    #[test_case(TestType::Big)]
    #[fuchsia::test]
    async fn register_update_notifier(test_type: TestType) {
        // We would like to use futures::channel::oneshot here but because the sender is captured
        // by the `FnMut` to `register_update_notifier`, it would not compile because `send` would
        // consume the sender. std::sync::mpsc is used instead of futures::channel::mpsc because
        // the register_update_notifier callback is not async-aware.
        use std::sync::mpsc;

        let (store, stream) = create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        #[derive(PartialEq)]
        enum Update {
            Add(Key),
            Remove(Key),
            Idle,
        }
        impl fmt::Debug for Update {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self {
                    Self::Add(k) => write!(f, "Add({k})"),
                    Self::Remove(k) => write!(f, "Remove({k})"),
                    Self::Idle => write!(f, "Idle"),
                }
            }
        }

        let dict = new_dict(test_type);
        let (update_tx, update_rx) = mpsc::channel();
        let subscribed = Arc::new(Mutex::new(true));
        let subscribed2 = subscribed.clone();
        dict.register_update_notifier(Box::new(move |update: EntryUpdate<'_>| {
            let u = match update {
                EntryUpdate::Add(k, v) => {
                    assert_matches!(v, Capability::Data(_));
                    Update::Add(k.into())
                }
                EntryUpdate::Remove(k) => Update::Remove(k.into()),
                EntryUpdate::Idle => Update::Idle,
            };
            update_tx.send(u).unwrap();
            if *subscribed2.lock() {
                UpdateNotifierRetention::Retain
            } else {
                UpdateNotifierRetention::Drop_
            }
        }));
        let dict_ref = Capability::Dictionary(dict.clone())
            .into_fsandbox_capability(WeakInstanceToken::new_invalid());
        let dict_id = 1;
        store.import(dict_id, dict_ref).await.unwrap().unwrap();

        // 1. Three inserts, one of which overlaps
        let i = 1;
        let data = Data::Int64(1).into();
        let value = (i + 10) as u64;
        store.import(value, data).await.unwrap().unwrap();
        store
            .dictionary_insert(dict_id, &fsandbox::DictionaryItem { key: key_for(i).into(), value })
            .await
            .unwrap()
            .unwrap();

        for expected_result in [Ok(()), Err(fsandbox::CapabilityStoreError::ItemAlreadyExists)] {
            let i = 2;
            let data = Data::Int64(1).into();
            let value = (i + 10) as u64;
            store.import(value, data).await.unwrap().unwrap();
            let result = store
                .dictionary_insert(
                    dict_id,
                    &fsandbox::DictionaryItem { key: key_for(i).into(), value },
                )
                .await
                .unwrap();
            assert_eq!(result, expected_result);
        }

        // 2. Remove the same item twice. Second time is a no-op
        for expected_result in [Ok(()), Err(fsandbox::CapabilityStoreError::ItemNotFound)] {
            let i = 1;
            let result =
                store.dictionary_remove(dict_id, &key_for(i).as_str(), None).await.unwrap();
            assert_eq!(result, expected_result);
        }

        // 3. One more insert, then drain
        let i = 3;
        let data = Data::Int64(1).into();
        let value = (i + 10) as u64;
        store.import(value, data).await.unwrap().unwrap();
        store
            .dictionary_insert(dict_id, &fsandbox::DictionaryItem { key: key_for(i).into(), value })
            .await
            .unwrap()
            .unwrap();
        store.dictionary_drain(dict_id, None).await.unwrap().unwrap();

        // 4. Unsubscribe to updates
        *subscribed.lock() = false;
        let i = 4;
        let data = Data::Int64(1).into();
        let value = (i + 10) as u64;
        store.import(value, data).await.unwrap().unwrap();
        store
            .dictionary_insert(dict_id, &fsandbox::DictionaryItem { key: key_for(i).into(), value })
            .await
            .unwrap()
            .unwrap();
        // This Remove shouldn't appear in the updates because we unsubscribed
        store.dictionary_remove(dict_id, key_for(i).as_str(), None).await.unwrap().unwrap();

        // Check the updates
        let updates: Vec<_> = iter::from_fn(move || match update_rx.try_recv() {
            Ok(e) => Some(e),
            Err(mpsc::TryRecvError::Disconnected) => None,
            // The producer should die before we get here because we unsubscribed
            Err(mpsc::TryRecvError::Empty) => unreachable!(),
        })
        .collect();
        let expected_updates = [
            Update::Idle,
            // 1.
            Update::Add(key_for(1)),
            Update::Add(key_for(2)),
            Update::Remove(key_for(2)),
            Update::Add(key_for(2)),
            // 2.
            Update::Remove(key_for(1)),
            // 3.
            Update::Add(key_for(3)),
            Update::Remove(key_for(2)),
            Update::Remove(key_for(3)),
            // 4.
            Update::Add(key_for(4)),
        ];
        match test_type {
            TestType::Small => {
                assert_eq!(updates, expected_updates);
            }
            TestType::Big => {
                // Skip over items populated to make the dict big
                let updates = &updates[HYBRID_SWITCH_INSERTION_LEN..];
                let nexpected = expected_updates.len() - 1;
                assert_eq!(updates[..nexpected], expected_updates[..nexpected]);

                // Skip over these items again when they are drained
                let expected_updates = &expected_updates[nexpected..];
                let updates = &updates[nexpected + HYBRID_SWITCH_INSERTION_LEN..];
                assert_eq!(updates, expected_updates);
            }
        }
    }

    #[fuchsia::test]
    async fn live_update_add_nodes() {
        let dict = Dictionary::new();
        let scope = ExecutionScope::new();
        let remote = dict
            .clone()
            .try_into_directory_entry(scope.clone(), WeakInstanceToken::new_invalid())
            .unwrap();
        let dir_proxy =
            serve_directory(remote.clone(), &scope, fio::PERM_READABLE).unwrap().into_proxy();
        let mut watcher = fuchsia_fs::directory::Watcher::new(&dir_proxy)
            .await
            .expect("failed to create watcher");

        // Assert that the directory is empty, because the dictionary is empty.
        assert_eq!(fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap(), vec![]);
        assert_eq!(
            watcher.next().await,
            Some(Ok(fuchsia_fs::directory::WatchMessage {
                event: fuchsia_fs::directory::WatchEvent::EXISTING,
                filename: ".".into(),
            })),
        );
        assert_eq!(
            watcher.next().await,
            Some(Ok(fuchsia_fs::directory::WatchMessage {
                event: fuchsia_fs::directory::WatchEvent::IDLE,
                filename: "".into(),
            })),
        );

        // Add an item to the dictionary, and assert that the projected directory contains the
        // added item.
        let fs = pseudo_directory! {};
        let dir_connector = DirConnector::from_directory_entry(fs, fio::PERM_READABLE);
        assert!(dict.insert("a".parse().unwrap(), dir_connector.clone().into()).is_none());

        assert_eq!(
            fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap(),
            vec![directory::DirEntry { name: "a".to_string(), kind: fio::DirentType::Directory },]
        );
        assert_eq!(
            watcher.next().await,
            Some(Ok(fuchsia_fs::directory::WatchMessage {
                event: fuchsia_fs::directory::WatchEvent::ADD_FILE,
                filename: "a".into(),
            })),
        );

        // Add an item to the dictionary, and assert that the projected directory contains the
        // added item.
        assert!(dict.insert("b".parse().unwrap(), dir_connector.into()).is_none());
        let mut readdir_results = fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap();
        readdir_results.sort_by(|entry_1, entry_2| entry_1.name.cmp(&entry_2.name));
        assert_eq!(
            readdir_results,
            vec![
                directory::DirEntry { name: "a".to_string(), kind: fio::DirentType::Directory },
                directory::DirEntry { name: "b".to_string(), kind: fio::DirentType::Directory },
            ]
        );
        assert_eq!(
            watcher.next().await,
            Some(Ok(fuchsia_fs::directory::WatchMessage {
                event: fuchsia_fs::directory::WatchEvent::ADD_FILE,
                filename: "b".into(),
            })),
        );
    }

    #[fuchsia::test]
    async fn live_update_remove_nodes() {
        let dict = Dictionary::new();
        let fs = pseudo_directory! {};
        let dir_connector = DirConnector::from_directory_entry(fs, fio::PERM_READABLE);
        assert!(dict.insert("a".parse().unwrap(), dir_connector.clone().into()).is_none());
        assert!(dict.insert("b".parse().unwrap(), dir_connector.clone().into()).is_none());
        assert!(dict.insert("c".parse().unwrap(), dir_connector.clone().into()).is_none());

        let scope = ExecutionScope::new();
        let remote = dict
            .clone()
            .try_into_directory_entry(scope.clone(), WeakInstanceToken::new_invalid())
            .unwrap();
        let dir_proxy =
            serve_directory(remote.clone(), &scope, fio::PERM_READABLE).unwrap().into_proxy();
        let mut watcher = fuchsia_fs::directory::Watcher::new(&dir_proxy)
            .await
            .expect("failed to create watcher");

        // The dictionary already had three entries in it when the directory proxy was created, so
        // we should see those in the directory. We check both readdir and via the watcher API.
        let mut readdir_results = fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap();
        readdir_results.sort_by(|entry_1, entry_2| entry_1.name.cmp(&entry_2.name));
        assert_eq!(
            readdir_results,
            vec![
                directory::DirEntry { name: "a".to_string(), kind: fio::DirentType::Directory },
                directory::DirEntry { name: "b".to_string(), kind: fio::DirentType::Directory },
                directory::DirEntry { name: "c".to_string(), kind: fio::DirentType::Directory },
            ]
        );
        let mut existing_files = vec![];
        loop {
            match watcher.next().await {
                Some(Ok(fuchsia_fs::directory::WatchMessage { event, filename }))
                    if event == fuchsia_fs::directory::WatchEvent::EXISTING =>
                {
                    existing_files.push(filename)
                }
                Some(Ok(fuchsia_fs::directory::WatchMessage { event, filename: _ }))
                    if event == fuchsia_fs::directory::WatchEvent::IDLE =>
                {
                    break;
                }
                other_message => panic!("unexpected message: {:?}", other_message),
            }
        }
        existing_files.sort();
        let expected_files: Vec<std::path::PathBuf> =
            vec![".".into(), "a".into(), "b".into(), "c".into()];
        assert_eq!(existing_files, expected_files,);

        // Remove each entry from the dictionary, and observe the directory watcher API inform us
        // that it has been removed.
        let _ =
            dict.remove(&BorrowedKey::new("a").unwrap()).expect("capability was not in dictionary");
        assert_eq!(
            watcher.next().await,
            Some(Ok(fuchsia_fs::directory::WatchMessage {
                event: fuchsia_fs::directory::WatchEvent::REMOVE_FILE,
                filename: "a".into(),
            })),
        );

        let _ =
            dict.remove(&BorrowedKey::new("b").unwrap()).expect("capability was not in dictionary");
        assert_eq!(
            watcher.next().await,
            Some(Ok(fuchsia_fs::directory::WatchMessage {
                event: fuchsia_fs::directory::WatchEvent::REMOVE_FILE,
                filename: "b".into(),
            })),
        );

        let _ =
            dict.remove(&BorrowedKey::new("c").unwrap()).expect("capability was not in dictionary");
        assert_eq!(
            watcher.next().await,
            Some(Ok(fuchsia_fs::directory::WatchMessage {
                event: fuchsia_fs::directory::WatchEvent::REMOVE_FILE,
                filename: "c".into(),
            })),
        );

        // At this point there are no entries left in the dictionary, so the directory should be
        // empty too.
        assert_eq!(fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap(), vec![],);
    }

    #[fuchsia::test]
    async fn into_directory_oneshot() {
        let dict = Dictionary::new();
        let inner_dict = Dictionary::new();
        let fs = pseudo_directory! {};
        let dir_connector = DirConnector::from_directory_entry(fs, fio::PERM_READABLE);
        assert!(inner_dict.insert("x".parse().unwrap(), dir_connector.clone().into()).is_none());
        assert!(dict.insert("a".parse().unwrap(), dir_connector.clone().into()).is_none());
        assert!(dict.insert("b".parse().unwrap(), dir_connector.clone().into()).is_none());
        assert!(
            dict.insert("c".parse().unwrap(), Capability::Dictionary(inner_dict.clone())).is_none()
        );

        let scope = ExecutionScope::new();
        let remote = dict
            .clone()
            .try_into_directory_entry_oneshot(scope.clone(), WeakInstanceToken::new_invalid())
            .unwrap();
        let dir_proxy =
            serve_directory(remote.clone(), &scope, fio::PERM_READABLE).unwrap().into_proxy();

        // The dictionary already had three entries in it when the directory proxy was created, so
        // we should see those in the directory.
        let mut readdir_results = fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap();
        readdir_results.sort_by(|entry_1, entry_2| entry_1.name.cmp(&entry_2.name));
        assert_eq!(
            readdir_results,
            vec![
                directory::DirEntry { name: "a".to_string(), kind: fio::DirentType::Directory },
                directory::DirEntry { name: "b".to_string(), kind: fio::DirentType::Directory },
                directory::DirEntry { name: "c".to_string(), kind: fio::DirentType::Directory },
            ]
        );

        let (inner_proxy, server) = create_proxy::<fio::DirectoryMarker>();
        dir_proxy.open("c", fio::PERM_READABLE, &Default::default(), server.into()).unwrap();
        let readdir_results = fuchsia_fs::directory::readdir(&inner_proxy).await.unwrap();
        assert_eq!(
            readdir_results,
            vec![directory::DirEntry { name: "x".to_string(), kind: fio::DirentType::Directory }]
        );

        // The Dict should be empty because `try_into_directory_entry_oneshot consumed it.
        assert!(dict.is_empty());
        assert!(inner_dict.is_empty());

        // Adding to the empty Dictionary has no impact on the directory.
        assert!(dict.insert("z".parse().unwrap(), dir_connector.clone().into()).is_none());
        let mut readdir_results = fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap();
        readdir_results.sort_by(|entry_1, entry_2| entry_1.name.cmp(&entry_2.name));
        assert_eq!(
            readdir_results,
            vec![
                directory::DirEntry { name: "a".to_string(), kind: fio::DirentType::Directory },
                directory::DirEntry { name: "b".to_string(), kind: fio::DirentType::Directory },
                directory::DirEntry { name: "c".to_string(), kind: fio::DirentType::Directory },
            ]
        );
    }

    /// Generates a key from an integer such that if i < j, key_for(i) < key_for(j).
    /// (A simple string conversion doesn't work because 1 < 10 but "1" > "10" in terms of
    /// string comparison.)
    fn key_for(i: usize) -> Key {
        iter::repeat("A").take(i).collect::<String>().parse().unwrap()
    }

    fn new_dict(test_type: TestType) -> Arc<Dictionary> {
        let dict = Dictionary::new();
        match test_type {
            TestType::Small => {}
            TestType::Big => {
                for i in 1..=HYBRID_SWITCH_INSERTION_LEN {
                    // These items will come last in the order as long as all other keys begin with
                    // a capital letter
                    assert!(
                        dict.insert(
                            format!("_{i}").parse().unwrap(),
                            Capability::Data(Data::Int64(1))
                        )
                        .is_none()
                    );
                }
            }
        }
        dict
    }

    fn adjusted_len(dict: &Dictionary, test_type: TestType) -> usize {
        let ofs = match test_type {
            TestType::Small => 0,
            TestType::Big => HYBRID_SWITCH_INSERTION_LEN,
        };
        dict.lock().entries.len() - ofs
    }
}
