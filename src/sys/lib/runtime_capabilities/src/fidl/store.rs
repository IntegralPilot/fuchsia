// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::dictionary::Key;
use crate::fidl::{IntoFsandboxCapability, registry};
use crate::{Capability, CapabilityBound, Connector, Dictionary, DirConnector, WeakInstanceToken};
use cm_types::RelativePath;
use fidl::handle::Signals;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_component_sandbox::{self as fsandbox, CapabilityStoreRequest};
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use futures::{FutureExt, TryStreamExt};
use log::warn;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{self, Arc, Weak};
use vfs::ExecutionScope;
use vfs::directory::entry::SubNode;
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::simple::Simple;
use vfs::path::Path;

type Store = sync::Mutex<HashMap<u64, Capability>>;

pub async fn serve_capability_store(
    mut stream: fsandbox::CapabilityStoreRequestStream,
    // Receiver tasks launched on behalf of a *Connector may need to outlive this
    // `CapabilityStore`. For example, if a client creates a Connector, Export()s it right away,
    // then drops the `CapabilityStore`, dropping the task at that point deliver a `PEER_CLOSED`
    // to the client and make the Connector they just created unusable.
    //
    // We could simply detach() the Task instead, but fuchsia_async considers that holding the
    // handle is considered better practice.
    receiver_scope: &fasync::Scope,
    token: Arc<WeakInstanceToken>,
) -> Result<(), fidl::Error> {
    let outer_store: Arc<Store> = Arc::new(Store::new(Default::default()));
    while let Some(request) = stream.try_next().await? {
        handle_capability_store_request(request, &outer_store, receiver_scope, &token)
            .boxed()
            .await?;
    }
    Ok(())
}

async fn handle_capability_store_request(
    request: CapabilityStoreRequest,
    outer_store: &Arc<Store>,
    receiver_scope: &fasync::Scope,
    token: &Arc<WeakInstanceToken>,
) -> Result<(), fidl::Error> {
    let mut store = outer_store.lock().unwrap();
    match request {
        fsandbox::CapabilityStoreRequest::Duplicate { id, dest_id, responder } => {
            let result = (|| {
                let cap = store.get(&id).ok_or(fsandbox::CapabilityStoreError::IdNotFound)?.clone();
                insert_capability(&mut store, dest_id, cap)
            })();
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::Drop { id, responder } => {
            let result =
                store.remove(&id).map(|_| ()).ok_or(fsandbox::CapabilityStoreError::IdNotFound);
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::Export { id, responder } => {
            let result = (|| {
                let cap = store.remove(&id).ok_or(fsandbox::CapabilityStoreError::IdNotFound)?;
                Ok(cap.into_fsandbox_capability(token.clone()))
            })();
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::Import { id, capability, responder } => {
            let result = (|| {
                let capability = capability
                    .try_into()
                    .map_err(|_| fsandbox::CapabilityStoreError::BadCapability)?;
                insert_capability(&mut store, id, capability)
            })();
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::ConnectorCreate { id, receiver, responder } => {
            let result = (|| {
                let connector = Connector::new_with_fidl_receiver(receiver, receiver_scope);
                insert_capability(&mut store, id, Capability::Connector(connector))
            })();
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::ConnectorOpen { id, server_end, responder } => {
            let result = (|| {
                let this = get_connector(&store, id)?;
                let _ = this.send(server_end);
                Ok(())
            })();
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::DirConnectorCreate { id, receiver, responder } => {
            let result = (|| {
                let connector = DirConnector::new_with_fidl_receiver(receiver, receiver_scope);
                insert_capability(&mut store, id, Capability::DirConnector(connector))
            })();
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::DirConnectorOpen { payload, responder } => {
            let result = (|| {
                let Some(id) = payload.id else {
                    return Err(fsandbox::CapabilityStoreError::InvalidArgs);
                };
                let Some(server_end) = payload.server_end else {
                    return Err(fsandbox::CapabilityStoreError::InvalidArgs);
                };
                let this = get_dir_connector(&store, id)?;
                let path = payload
                    .path
                    .map(RelativePath::new)
                    .transpose()
                    .map_err(|_| fsandbox::CapabilityStoreError::InvalidArgs)?
                    .unwrap_or_else(|| RelativePath::dot());
                let _ = this.send(server_end, path, payload.flags);
                Ok(())
            })();
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::DictionaryCreate { id, responder } => {
            let result =
                insert_capability(&mut store, id, Capability::Dictionary(Dictionary::new()));
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::DictionaryLegacyImport { id, client_end, responder } => {
            let result = (|| {
                let dictionary = Dictionary::from_channel(client_end)
                    .map_err(|_| fsandbox::CapabilityStoreError::BadCapability)?;
                let capability = Capability::Dictionary(dictionary);
                insert_capability(&mut store, id, capability)
            })();
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::DictionaryLegacyExport { id, server_end, responder } => {
            let result = (|| {
                let cap = store.remove(&id).ok_or(fsandbox::CapabilityStoreError::IdNotFound)?;
                let Capability::Dictionary(_) = &cap else {
                    return Err(fsandbox::CapabilityStoreError::WrongType);
                };
                let koid = server_end.as_handle_ref().basic_info().unwrap().related_koid;
                registry::insert(
                    cap,
                    koid,
                    fasync::OnSignals::new(server_end, Signals::OBJECT_PEER_CLOSED).map(|_| ()),
                );
                Ok(())
            })();
            responder.send(result)?
        }
        fsandbox::CapabilityStoreRequest::DictionaryInsert { id, item, responder } => {
            let result = (|| {
                let this = get_dictionary(&store, id)?;
                let key =
                    item.key.parse().map_err(|_| fsandbox::CapabilityStoreError::InvalidKey)?;
                let value =
                    store.remove(&item.value).ok_or(fsandbox::CapabilityStoreError::IdNotFound)?;
                if this.insert(key, value).is_some() {
                    Err(fsandbox::CapabilityStoreError::ItemAlreadyExists)
                } else {
                    Ok(())
                }
            })();
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::DictionaryGet { id, key, dest_id, responder } => {
            let result = (|| {
                let this = get_dictionary(&store, id)?;
                let key = Key::new(key).map_err(|_| fsandbox::CapabilityStoreError::InvalidKey)?;
                let cap = match this.get(&key) {
                    Some(cap) => Ok(cap),
                    None => {
                        this.not_found(key.as_str());
                        Err(fsandbox::CapabilityStoreError::ItemNotFound)
                    }
                }?;
                insert_capability(&mut store, dest_id, cap)
            })();
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::DictionaryRemove { id, key, dest_id, responder } => {
            let result = (|| {
                let this = get_dictionary(&store, id)?;
                let key = Key::new(key).map_err(|_| fsandbox::CapabilityStoreError::InvalidKey)?;
                // Check this before removing from the dictionary.
                if let Some(dest_id) = dest_id.as_ref() {
                    if store.contains_key(&dest_id.id) {
                        return Err(fsandbox::CapabilityStoreError::IdAlreadyExists);
                    }
                }
                let cap = match this.remove(&key) {
                    Some(cap) => Ok(cap.into()),
                    None => {
                        this.not_found(key.as_str());
                        Err(fsandbox::CapabilityStoreError::ItemNotFound)
                    }
                }?;
                if let Some(dest_id) = dest_id.as_ref() {
                    store.insert(dest_id.id, cap);
                }
                Ok(())
            })();
            responder.send(result)?;
        }
        fsandbox::CapabilityStoreRequest::DictionaryCopy { id, dest_id, responder } => {
            let result = (|| {
                let this = get_dictionary(&store, id)?;
                let dict = this.shallow_copy();
                insert_capability(&mut store, dest_id, Capability::Dictionary(dict))
            })();
            responder.send(result)?
        }
        fsandbox::CapabilityStoreRequest::DictionaryKeys {
            id,
            iterator: server_end,
            responder,
        } => {
            let result = (|| {
                let this = get_dictionary(&store, id)?;
                let keys = this.snapshot_keys_as_strings().into_iter();
                let stream = server_end.into_stream();
                let mut this = this.lock();
                this.tasks().spawn(serve_dictionary_keys_iterator(keys, stream));
                Ok(())
            })();
            responder.send(result)?
        }
        fsandbox::CapabilityStoreRequest::DictionaryEnumerate {
            id,
            iterator: server_end,
            responder,
        } => {
            let result = (|| {
                let this = get_dictionary(&store, id)?;
                let items = this.enumerate().map(|(k, v)| (k, Ok(v)));
                let stream = server_end.into_stream();
                let mut this = this.lock();
                this.tasks().spawn(serve_dictionary_enumerate_iterator(
                    Arc::downgrade(&outer_store),
                    items,
                    stream,
                ));
                Ok(())
            })();
            responder.send(result)?
        }
        fsandbox::CapabilityStoreRequest::DictionaryDrain {
            id,
            iterator: server_end,
            responder,
        } => {
            let result = (|| {
                let this = get_dictionary(&store, id)?;
                // Take out entries, replacing with an empty BTreeMap.
                // They are dropped if the caller does not request an iterator.
                let items = this.drain();
                if let Some(server_end) = server_end {
                    let stream = server_end.into_stream();
                    let mut this = this.lock();
                    this.tasks().spawn(serve_dictionary_drain_iterator(
                        Arc::downgrade(&outer_store),
                        items,
                        stream,
                    ));
                }
                Ok(())
            })();
            responder.send(result)?
        }
        fsandbox::CapabilityStoreRequest::CreateServiceAggregate { sources, responder } => {
            // Store does not use an async-compatible mutex, so we can't hold a MutexGuard for
            // it across await boundaries. This means we must drop the store MutexGuard before
            // calling await, or Rust yells at us that the futures are not Send.
            drop(store);
            responder.send(create_service_aggregate(token.clone(), sources).await)?;
        }
        fsandbox::CapabilityStoreRequest::_UnknownMethod { ordinal, .. } => {
            warn!("Received unknown CapabilityStore request with ordinal {ordinal}");
        }
    }
    Ok(())
}

async fn serve_dictionary_keys_iterator(
    mut keys: impl Iterator<Item = String>,
    mut stream: fsandbox::DictionaryKeysIteratorRequestStream,
) {
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fsandbox::DictionaryKeysIteratorRequest::GetNext { responder } => {
                let mut chunk = vec![];
                for _ in 0..fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK {
                    match keys.next() {
                        Some(key) => {
                            chunk.push(key.into());
                        }
                        None => break,
                    }
                }
                let _ = responder.send(&chunk);
            }
            fsandbox::DictionaryKeysIteratorRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "Unknown DictionaryKeysIterator request");
            }
        }
    }
}

async fn serve_dictionary_enumerate_iterator(
    store: Weak<Store>,
    mut items: impl Iterator<Item = (Key, Result<Capability, ()>)>,
    mut stream: fsandbox::DictionaryEnumerateIteratorRequestStream,
) {
    while let Ok(Some(request)) = stream.try_next().await {
        let Some(store) = store.upgrade() else {
            return;
        };
        let mut store = store.lock().unwrap();
        match request {
            fsandbox::DictionaryEnumerateIteratorRequest::GetNext {
                start_id,
                limit,
                responder,
            } => {
                let result = (|| {
                    let mut next_id = start_id;
                    let chunk = get_next_chunk(&*store, &mut items, &mut next_id, limit)?;
                    let end_id = next_id;

                    let chunk: Vec<_> = chunk
                        .into_iter()
                        .map(|(key, value)| {
                            if let Some((capability, id)) = value {
                                store.insert(id, capability);
                                fsandbox::DictionaryOptionalItem {
                                    key: key.into(),
                                    value: Some(Box::new(fsandbox::WrappedCapabilityId { id })),
                                }
                            } else {
                                fsandbox::DictionaryOptionalItem { key: key.into(), value: None }
                            }
                        })
                        .collect();
                    Ok((chunk, end_id))
                })();
                let err = result.is_err();
                let _ = responder.send(result);
                if err {
                    return;
                }
            }
            fsandbox::DictionaryEnumerateIteratorRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "Unknown DictionaryEnumerateIterator request");
            }
        }
    }
}

async fn serve_dictionary_drain_iterator(
    store: Weak<Store>,
    items: impl Iterator<Item = (Key, Capability)>,
    mut stream: fsandbox::DictionaryDrainIteratorRequestStream,
) {
    // Transform iterator to be compatible with get_next_chunk()
    let mut items = items.map(|(key, capability)| (key, Ok(capability)));
    while let Ok(Some(request)) = stream.try_next().await {
        let Some(store) = store.upgrade() else {
            return;
        };
        let mut store = store.lock().unwrap();
        match request {
            fsandbox::DictionaryDrainIteratorRequest::GetNext { start_id, limit, responder } => {
                let result = (|| {
                    let mut next_id = start_id;
                    let chunk = get_next_chunk(&*store, &mut items, &mut next_id, limit)?;
                    let end_id = next_id;

                    let chunk: Vec<_> = chunk
                        .into_iter()
                        .map(|(key, value)| {
                            let value = value.expect("unreachable: all values are present");
                            let (capability, id) = value;
                            store.insert(id, capability);
                            fsandbox::DictionaryItem { key: key.into(), value: id }
                        })
                        .collect();
                    Ok((chunk, end_id))
                })();
                match result {
                    Ok((chunk, id)) => {
                        let _ = responder.send(Ok((&chunk[..], id)));
                    }
                    Err(e) => {
                        let _ = responder.send(Err(e));
                        return;
                    }
                }
            }
            fsandbox::DictionaryDrainIteratorRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "Unknown DictionaryDrainIterator request");
            }
        }
    }
}

fn get_next_chunk(
    store: &HashMap<u64, Capability>,
    items: &mut impl Iterator<Item = (Key, Result<Capability, ()>)>,
    next_id: &mut u64,
    limit: u32,
) -> Result<Vec<(Key, Option<(Capability, fsandbox::CapabilityId)>)>, fsandbox::CapabilityStoreError>
{
    if limit == 0 || limit > fsandbox::MAX_DICTIONARY_ITERATOR_CHUNK {
        return Err(fsandbox::CapabilityStoreError::InvalidArgs);
    }

    let mut chunk = vec![];
    for _ in 0..limit {
        match items.next() {
            Some((key, value)) => {
                let value = match value {
                    Ok(value) => {
                        let id = *next_id;
                        // Pre-flight check: if an id is unavailable, return early
                        // and don't make any changes to the store.
                        if store.contains_key(&id) {
                            return Err(fsandbox::CapabilityStoreError::IdAlreadyExists);
                        }
                        *next_id += 1;
                        Some((value, id))
                    }
                    Err(_) => None,
                };
                chunk.push((key, value));
            }
            None => break,
        }
    }
    Ok(chunk)
}

fn get_connector(
    store: &HashMap<u64, Capability>,
    id: u64,
) -> Result<&Connector, fsandbox::CapabilityStoreError> {
    let conn = store.get(&id).ok_or(fsandbox::CapabilityStoreError::IdNotFound)?;
    if let Capability::Connector(conn) = conn {
        Ok(conn)
    } else {
        Err(fsandbox::CapabilityStoreError::WrongType)
    }
}

fn get_dir_connector(
    store: &HashMap<u64, Capability>,
    id: u64,
) -> Result<&DirConnector, fsandbox::CapabilityStoreError> {
    let conn = store.get(&id).ok_or(fsandbox::CapabilityStoreError::IdNotFound)?;
    if let Capability::DirConnector(conn) = conn {
        Ok(conn)
    } else {
        Err(fsandbox::CapabilityStoreError::WrongType)
    }
}

fn get_dictionary(
    store: &HashMap<u64, Capability>,
    id: u64,
) -> Result<Arc<Dictionary>, fsandbox::CapabilityStoreError> {
    let dict = store.get(&id).ok_or(fsandbox::CapabilityStoreError::IdNotFound)?;
    if let Capability::Dictionary(dict) = dict {
        Ok(dict.clone())
    } else {
        Err(fsandbox::CapabilityStoreError::WrongType)
    }
}

fn insert_capability(
    store: &mut HashMap<u64, Capability>,
    id: u64,
    cap: Capability,
) -> Result<(), fsandbox::CapabilityStoreError> {
    match store.entry(id) {
        Entry::Occupied(_) => Err(fsandbox::CapabilityStoreError::IdAlreadyExists),
        Entry::Vacant(entry) => {
            entry.insert(cap);
            Ok(())
        }
    }
}

async fn create_service_aggregate(
    route_source: Arc<WeakInstanceToken>,
    sources: Vec<fsandbox::AggregateSource>,
) -> Result<fsandbox::DirConnector, fsandbox::CapabilityStoreError> {
    fn is_set<T>(val: &Option<Vec<T>>) -> bool {
        val.as_ref().map(|v| !v.is_empty()).unwrap_or(false)
    }
    if sources.iter().any(|s| is_set(&s.source_instance_filter) || is_set(&s.renamed_instances)) {
        // This is a renames aggregate
        let dir_connectors_and_renames = sources
            .into_iter()
            .map(|s| {
                let renames = process_renames(&s);
                let sandbox_dir_connector =
                    s.dir_connector.ok_or(fsandbox::CapabilityStoreError::InvalidArgs)?;
                let dir_connector = DirConnector::try_from_fsandbox(sandbox_dir_connector)
                    .map_err(|_| fsandbox::CapabilityStoreError::InvalidArgs)?;
                Ok((dir_connector, renames))
            })
            .collect::<Result<Vec<_>, fsandbox::CapabilityStoreError>>()?;
        let target_directory = Simple::new();
        for (dir_connector, renames) in dir_connectors_and_renames.into_iter() {
            for mapping in renames.into_iter() {
                let source_path = Path::validate_and_split(mapping.source_name)
                    .map_err(|_| fsandbox::CapabilityStoreError::InvalidArgs)?;
                let target_path = Path::validate_and_split(mapping.target_name)
                    .map_err(|_| fsandbox::CapabilityStoreError::InvalidArgs)?;
                let dir_connector_as_dir_entry = dir_connector
                    .clone()
                    .try_into_directory_entry(ExecutionScope::new(), route_source.clone())
                    .expect("this is infallible");
                let sub_node = Arc::new(SubNode::new(
                    dir_connector_as_dir_entry,
                    source_path,
                    fio::DirentType::Directory,
                ));
                let sub_dir_connector =
                    DirConnector::from_directory_entry(sub_node, fio::PERM_READABLE);
                let sub_dir_entry = sub_dir_connector
                    .try_into_directory_entry(ExecutionScope::new(), route_source.clone())
                    .expect("this is infallible");
                target_directory
                    .add_entry(target_path.as_str(), sub_dir_entry)
                    .map_err(|_| fsandbox::CapabilityStoreError::InvalidArgs)?;
            }
        }
        let dir_connector =
            DirConnector::from_directory_entry(target_directory, fio::PERM_READABLE);
        let fsandbox::Capability::DirConnector(dir_connector) =
            dir_connector.into_fsandbox_capability(WeakInstanceToken::new_invalid())
        else {
            unreachable!("the above function always returns a fsandbox::DirConnector value");
        };
        return Ok(dir_connector);
    }
    // Anonymous aggregates are currently unsupported.
    Err(fsandbox::CapabilityStoreError::InvalidArgs)
}

fn process_renames(source: &fsandbox::AggregateSource) -> Vec<fdecl::NameMapping> {
    match (&source.source_instance_filter, &source.renamed_instances) {
        (Some(filter), Some(renames)) if !renames.is_empty() && !filter.is_empty() => renames
            .iter()
            .filter(|mapping| filter.contains(&mapping.target_name))
            .cloned()
            .collect(),
        (Some(filter), _) if !filter.is_empty() => filter
            .iter()
            .map(|name| fdecl::NameMapping { source_name: name.clone(), target_name: name.clone() })
            .collect(),
        (_, Some(renames)) if !renames.is_empty() => renames.clone(),
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Data, DirConnectable, Handle};
    use assert_matches::assert_matches;
    use fidl::endpoints::{ServerEnd, create_endpoints};
    use fidl::{AsHandleRef, endpoints};
    use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

    #[fuchsia::test]
    async fn import_export() {
        let (store, stream) =
            endpoints::create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let (ch, _) = fidl::Channel::create();
        let handle = ch.into_handle();
        let handle_koid = handle.as_handle_ref().koid().unwrap();
        let cap1 = Capability::Handle(Handle::new(handle));
        let cap2 = Capability::Data(Data::Int64(42));
        store
            .import(1, cap1.into_fsandbox_capability(WeakInstanceToken::new_invalid()))
            .await
            .unwrap()
            .unwrap();
        store
            .import(2, cap2.into_fsandbox_capability(WeakInstanceToken::new_invalid()))
            .await
            .unwrap()
            .unwrap();

        let cap1 = store.export(1).await.unwrap().unwrap();
        let cap2 = store.export(2).await.unwrap().unwrap();
        assert_matches!(
            cap1,
            fsandbox::Capability::Handle(h) if h.as_handle_ref().koid().unwrap() == handle_koid
        );
        assert_matches!(
            cap2,
            fsandbox::Capability::Data(fsandbox::Data::Int64(i)) if i == 42
        );
    }

    #[fuchsia::test]
    async fn import_error() {
        let (store, stream) =
            endpoints::create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let cap1 = Capability::Data(Data::Int64(42));
        store
            .import(1, cap1.clone().into_fsandbox_capability(WeakInstanceToken::new_invalid()))
            .await
            .unwrap()
            .unwrap();
        assert_matches!(
            store
                .import(1, cap1.into_fsandbox_capability(WeakInstanceToken::new_invalid()))
                .await
                .unwrap(),
            Err(fsandbox::CapabilityStoreError::IdAlreadyExists)
        );

        let (token, _) = fidl::EventPair::create();
        let bad_connector = fsandbox::Capability::Connector(fsandbox::Connector { token });
        assert_matches!(
            store.import(2, bad_connector).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::BadCapability)
        );
    }

    #[fuchsia::test]
    async fn export_error() {
        let (store, stream) =
            endpoints::create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let cap1 = Capability::Data(Data::Int64(42));
        store
            .import(1, cap1.clone().into_fsandbox_capability(WeakInstanceToken::new_invalid()))
            .await
            .unwrap()
            .unwrap();

        assert_matches!(
            store.export(2).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdNotFound)
        );
    }

    #[fuchsia::test]
    async fn drop() {
        let (store, stream) =
            endpoints::create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let (ch, _) = fidl::Channel::create();
        let handle = ch.into_handle();
        let handle_koid = handle.as_handle_ref().koid().unwrap();
        let cap1 = Capability::Handle(Handle::new(handle));
        let cap2 = Capability::Data(Data::Int64(42));
        store
            .import(1, cap1.into_fsandbox_capability(WeakInstanceToken::new_invalid()))
            .await
            .unwrap()
            .unwrap();
        store
            .import(2, cap2.into_fsandbox_capability(WeakInstanceToken::new_invalid()))
            .await
            .unwrap()
            .unwrap();

        // Drop capability 2. It's no longer in the store.
        store.drop(2).await.unwrap().unwrap();
        assert_matches!(
            store.export(1).await.unwrap(),
            Ok(fsandbox::Capability::Handle(h)) if h.as_handle_ref().koid().unwrap() == handle_koid
        );
        assert_matches!(
            store.export(2).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdNotFound)
        );

        // Id 2 can be reused.
        let cap2 = Capability::Data(Data::Int64(84));
        store
            .import(2, cap2.into_fsandbox_capability(WeakInstanceToken::new_invalid()))
            .await
            .unwrap()
            .unwrap();
        assert_matches!(
            store.export(2).await.unwrap(),
            Ok(fsandbox::Capability::Data(fsandbox::Data::Int64(i))) if i == 84
        );
    }

    #[fuchsia::test]
    async fn drop_error() {
        let (store, stream) =
            endpoints::create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let cap1 = Capability::Data(Data::Int64(42));
        store
            .import(1, cap1.into_fsandbox_capability(WeakInstanceToken::new_invalid()))
            .await
            .unwrap()
            .unwrap();

        assert_matches!(
            store.drop(2).await.unwrap(),
            Err(fsandbox::CapabilityStoreError::IdNotFound)
        );
    }

    #[fuchsia::test]
    async fn duplicate() {
        let (store, stream) =
            endpoints::create_proxy_and_stream::<fsandbox::CapabilityStoreMarker>();
        let _server = fasync::Task::spawn(async move {
            let receiver_scope = fasync::Scope::new();
            serve_capability_store(stream, &receiver_scope, WeakInstanceToken::new_invalid()).await
        });

        let (event, _) = fidl::EventPair::create();
        let handle = event.into_handle();
        let handle_koid = handle.as_handle_ref().koid().unwrap();
        let cap1 = Capability::Handle(Handle::new(handle));
        store
            .import(1, cap1.into_fsandbox_capability(WeakInstanceToken::new_invalid()))
            .await
            .unwrap()
            .unwrap();
        store.duplicate(1, 2).await.unwrap().unwrap();
        store.drop(1).await.unwrap().unwrap();

        let cap1 = store.export(2).await.unwrap().unwrap();
        assert_matches!(
            cap1,
            fsandbox::Capability::Handle(h) if h.as_handle_ref().koid().unwrap() == handle_koid
        );
    }

    #[derive(Debug)]
    struct TestDirConnector {
        sender:
            UnboundedSender<(ServerEnd<fio::DirectoryMarker>, RelativePath, Option<fio::Flags>)>,
    }

    impl DirConnectable for TestDirConnector {
        fn maximum_flags(&self) -> fio::Flags {
            fio::PERM_READABLE
        }

        fn send(
            &self,
            dir: ServerEnd<fio::DirectoryMarker>,
            subdir: RelativePath,
            flags: Option<fio::Flags>,
        ) -> Result<(), ()> {
            self.sender.unbounded_send((dir, subdir, flags)).unwrap();
            Ok(())
        }
    }

    impl TestDirConnector {
        fn new() -> (
            Arc<DirConnector>,
            UnboundedReceiver<(ServerEnd<fio::DirectoryMarker>, RelativePath, Option<fio::Flags>)>,
        ) {
            let (sender, receiver) = unbounded();
            (DirConnector::new_sendable(Self { sender }), receiver)
        }
    }

    #[fuchsia::test]
    async fn rename_aggregate_with_one_source() {
        let (source_dir_connector, mut source_dir_receiver) = TestDirConnector::new();
        let sources = vec![fsandbox::AggregateSource {
            dir_connector: Some(source_dir_connector.to_fsandbox()),
            renamed_instances: Some(vec![fdecl::NameMapping {
                source_name: "foo".to_string(),
                target_name: "bar".to_string(),
            }]),
            ..Default::default()
        }];
        let fidl_aggregate = create_service_aggregate(WeakInstanceToken::new_invalid(), sources)
            .await
            .expect("failed to create service aggregate");
        let aggregate =
            DirConnector::try_from_fsandbox(fidl_aggregate).expect("invalid dir connector");

        let (client_end, server_end) = create_endpoints::<fio::DirectoryMarker>();
        aggregate.send(server_end, RelativePath::new("bar").unwrap(), None).unwrap();
        let (received_server_end, path, flags) = source_dir_receiver.try_next().unwrap().unwrap();
        assert_eq!(
            client_end.as_handle_ref().basic_info().unwrap().koid,
            received_server_end.as_handle_ref().basic_info().unwrap().related_koid
        );
        assert_eq!(path, RelativePath::new("foo").unwrap());
        assert_eq!(flags, Some(fio::PERM_READABLE));
    }

    #[fuchsia::test]
    async fn rename_aggregate_with_two_sources() {
        let (source_dir_connector_1, source_dir_receiver_1) = TestDirConnector::new();
        let (source_dir_connector_2, source_dir_receiver_2) = TestDirConnector::new();
        let sources = vec![
            fsandbox::AggregateSource {
                dir_connector: Some(source_dir_connector_1.to_fsandbox()),
                renamed_instances: Some(vec![fdecl::NameMapping {
                    source_name: "foo".to_string(),
                    target_name: "bar".to_string(),
                }]),
                ..Default::default()
            },
            fsandbox::AggregateSource {
                dir_connector: Some(source_dir_connector_2.to_fsandbox()),
                renamed_instances: Some(vec![fdecl::NameMapping {
                    source_name: "foo".to_string(),
                    target_name: "baz".to_string(),
                }]),
                ..Default::default()
            },
        ];
        let fidl_aggregate = create_service_aggregate(WeakInstanceToken::new_invalid(), sources)
            .await
            .expect("failed to create service aggregate");
        let aggregate =
            DirConnector::try_from_fsandbox(fidl_aggregate).expect("invalid dir connector");

        for (mut receiver, name) in [(source_dir_receiver_1, "bar"), (source_dir_receiver_2, "baz")]
        {
            let (client_end, server_end) = create_endpoints::<fio::DirectoryMarker>();
            aggregate.send(server_end, RelativePath::new(name).unwrap(), None).unwrap();
            let (received_server_end, path, flags) = receiver.try_next().unwrap().unwrap();
            assert_eq!(
                client_end.as_handle_ref().basic_info().unwrap().koid,
                received_server_end.as_handle_ref().basic_info().unwrap().related_koid
            );
            assert_eq!(path, RelativePath::new("foo").unwrap());
            assert_eq!(flags, Some(fio::PERM_READABLE));
        }
    }

    #[fuchsia::test]
    async fn rename_and_filtering_aggregate() {
        let (source_dir_connector_1, source_dir_receiver_1) = TestDirConnector::new();
        let (source_dir_connector_2, source_dir_receiver_2) = TestDirConnector::new();
        let sources = vec![
            fsandbox::AggregateSource {
                dir_connector: Some(source_dir_connector_1.to_fsandbox()),
                renamed_instances: Some(vec![fdecl::NameMapping {
                    source_name: "foo".to_string(),
                    target_name: "bar".to_string(),
                }]),
                ..Default::default()
            },
            fsandbox::AggregateSource {
                dir_connector: Some(source_dir_connector_2.to_fsandbox()),
                source_instance_filter: Some(vec!["foo".to_string()]),
                ..Default::default()
            },
        ];
        let fidl_aggregate = create_service_aggregate(WeakInstanceToken::new_invalid(), sources)
            .await
            .expect("failed to create service aggregate");
        let aggregate =
            DirConnector::try_from_fsandbox(fidl_aggregate).expect("invalid dir connector");

        for (mut receiver, name) in [(source_dir_receiver_1, "bar"), (source_dir_receiver_2, "foo")]
        {
            let (client_end, server_end) = create_endpoints::<fio::DirectoryMarker>();
            aggregate.send(server_end, RelativePath::new(name).unwrap(), None).unwrap();
            let (received_server_end, path, flags) = receiver.try_next().unwrap().unwrap();
            assert_eq!(
                client_end.as_handle_ref().basic_info().unwrap().koid,
                received_server_end.as_handle_ref().basic_info().unwrap().related_koid
            );
            assert_eq!(path, RelativePath::new("foo").unwrap());
            assert_eq!(flags, Some(fio::PERM_READABLE));
        }
    }
}
