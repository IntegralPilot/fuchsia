// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::composite_node_spec_manager::CompositeNodeSpecManager;
use crate::indexer::*;
use crate::resolved_driver::ResolvedDriver;
use anyhow::anyhow;
use fidl_fuchsia_component_sandbox as fsandbox;
use fuchsia_component::escrow::EscrowOperation;
use std::collections::HashMap;
use std::rc::Rc;

const STATE_DRIVER_NOTIFIER: &str = "notifier";
const STATE_SAVED_STATE: &str = "saved_state";

pub struct ResumedState {
    pub notifier: Option<zx::NullableHandle>,
    pub composite_specs: CompositeNodeSpecManager,
    pub boot_repo: Option<Vec<ResolvedDriver>>,
    pub base_repo: Option<BaseRepo>,
    pub ephemeral_repo: HashMap<cm_types::Url, ResolvedDriver>,
}

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
struct SavedState {
    composite_specs: CompositeNodeSpecManager,
    boot_repo: Vec<ResolvedDriver>,
    base_repo: Option<Vec<ResolvedDriver>>,
    ephemeral_repo: Vec<ResolvedDriver>,
}

// Returns whether there were any base drivers that loaded from the resume state.
pub fn apply_state(resume_state: ResumedState, index: Rc<Indexer>) -> bool {
    match resume_state.notifier {
        Some(handle) => {
            log::debug!("loading driver notifier from escrow.");
            index.set_notifier(handle.into());
        }
        None => {}
    };

    log::debug!("loading composite node specs from escrow.");
    index.composite_node_spec_manager.replace(resume_state.composite_specs);

    let base_loaded = if let Some(base_repo) = resume_state.base_repo {
        index.base_repo.replace(base_repo);
        log::debug!("loading base drivers from escrow.");
        true
    } else {
        false
    };

    log::debug!("loading ephemeral drivers from escrow.");
    index.ephemeral_drivers.replace(resume_state.ephemeral_repo);

    base_loaded
}

// This wraps the logic for storing our state to a `fuchsia.component.sandbox.Dictionary`, and
// sending an OnEscrow event to the `fuchsia.process.lifecycle.Lifecycle` with the dictionary
// we saved our state to, as well as our outgoing directory server end.
pub async fn handle_stall(
    index: Rc<Indexer>,
    escrow_operation: &EscrowOperation,
    outgoing_directory: zx::Channel,
    capability_store: &fsandbox::CapabilityStoreProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    dict_id: u64,
) -> Result<(), anyhow::Error> {
    // Create a new dictionary, save our state to it.
    capability_store
        .dictionary_create(dict_id)
        .await
        .map_err(|e| anyhow!("Could not call dictionary_create {:?}", e))?
        .map_err(|e| anyhow!("Received error from dictionary_create {:?}.", e))?;

    save_state(index.clone(), &capability_store, &id_gen, dict_id).await?;

    // Send the dictionary away.
    let fsandbox::Capability::Dictionary(dictionary_ref) = capability_store
        .export(dict_id)
        .await
        .map_err(|e| anyhow!("Could not call export {:?}", e))?
        .map_err(|e| anyhow!("Received error from export {:?}.", e))?
    else {
        panic!("capability is not Dictionary?");
    };

    escrow_operation.with_fsandbox_dictionary(dictionary_ref);
    escrow_operation
        .run(outgoing_directory.into())
        .map_err(|e| anyhow!("Could not perform escrow {:?}", e))
}

pub async fn resume_state(
    capability_store: &fsandbox::CapabilityStoreProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    dict_id: u64,
) -> Result<ResumedState, anyhow::Error> {
    let notifier_result =
        get_handle_from_dictionary(capability_store, id_gen, dict_id, STATE_DRIVER_NOTIFIER).await;
    let notifier = match notifier_result {
        Ok(notifier) => Some(notifier),
        Err(e) => {
            log::warn!("Failed to get notifier {:?}", e);
            None
        }
    };

    let now = std::time::Instant::now();

    let saved_state_bytes = read_vmo_and_decompress_from_dictionary(
        capability_store,
        id_gen,
        dict_id,
        STATE_SAVED_STATE,
    )
    .await?;
    let read_and_decompress_time = now.elapsed();
    let saved_state = rkyv::from_bytes::<SavedState, rkyv::rancor::Failure>(&saved_state_bytes)?;
    let deserialize_time = now.elapsed();

    let composite_specs = saved_state.composite_specs;
    let boot_repo = Some(saved_state.boot_repo);
    let base_repo = match saved_state.base_repo {
        Some(base) => Some(BaseRepo::Resolved(base)),
        None => None,
    };
    let ephemeral_repo = HashMap::from_iter(
        saved_state.ephemeral_repo.into_iter().map(|d| (d.component_url.clone(), d)),
    );

    log::debug!(
        "read and decompress duration {:?}, deserialize duration {:?}",
        read_and_decompress_time,
        deserialize_time
    );

    Ok(ResumedState { notifier, composite_specs, boot_repo, base_repo, ephemeral_repo })
}

async fn save_state(
    index: Rc<Indexer>,
    capability_store: &fsandbox::CapabilityStoreProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    dict_id: u64,
) -> Result<(), anyhow::Error> {
    {
        let id = id_gen.next();
        match index.take_notifier() {
            Some(notifier) => {
                capability_store
                    .import(id, fsandbox::Capability::Handle(notifier.into()))
                    .await
                    .map_err(|e| anyhow!("Failed to call import: {:?}", e))?
                    .map_err(|e| anyhow!("Failed to import with store error: {:?}", e))?;

                capability_store
                    .dictionary_insert(
                        dict_id,
                        &fsandbox::DictionaryItem { key: STATE_DRIVER_NOTIFIER.into(), value: id },
                    )
                    .await
                    .map_err(|e| anyhow!("Failed to call dictionary_insert: {:?}", e))?
                    .map_err(|e| {
                        anyhow!("Failed to dictionary_insert with store error: {:?}", e)
                    })?;
            }
            None => {
                log::debug!("No notifier to store.");
            }
        }
    }

    let composite_specs = index.composite_node_spec_manager.take();
    let boot_repo = index.boot_repo.take();
    let base_repo = index.base_repo.take();
    let base_repo = match base_repo {
        BaseRepo::Resolved(base_repo) => Some(base_repo),
        BaseRepo::NotResolved => None,
    };

    let ephemeral_repo = index.ephemeral_drivers.take();
    let ephemeral_repo = ephemeral_repo.into_iter().map(|e| e.1).collect::<Vec<_>>();

    let saved_state = SavedState { composite_specs, boot_repo, base_repo, ephemeral_repo };

    let now = std::time::Instant::now();

    let data = rkyv::api::high::to_bytes_in::<_, rkyv::rancor::Error>(&saved_state, Vec::new())
        .map_err(|e| anyhow!("Failed to serialize saved state with rkyv: {:?}", e))?;

    let serialize_time = now.elapsed();
    compress_and_save_to_dictionary_vmo(data, capability_store, id_gen, dict_id, STATE_SAVED_STATE)
        .await?;
    let compress_and_save_time = now.elapsed();

    log::debug!(
        "serialize duration {:?}, compress and save duration {:?}",
        serialize_time,
        compress_and_save_time
    );
    Ok(())
}

async fn save_to_dictionary_vmo(
    data: Vec<u8>,
    capability_store: &fsandbox::CapabilityStoreProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    dict_id: u64,
    key: &str,
) -> Result<(), anyhow::Error> {
    let size = data.len() as u64;
    let vmo = zx::Vmo::create(size).map_err(|e| anyhow!("Failed to create vmo: {:?}", e))?;
    vmo.write(data.as_slice(), 0).map_err(|e| anyhow!("Failed to write to vmo: {:?}", e))?;
    vmo.set_content_size(&size)
        .map_err(|e| anyhow!("Failed to set_content_size on vmo: {:?}", e))?;

    log::debug!("{} vmo size: {}", key, size);

    let id = id_gen.next();
    capability_store
        .import(id, fsandbox::Capability::Handle(vmo.into()))
        .await
        .map_err(|e| anyhow!("Failed to call import: {:?}", e))?
        .map_err(|e| anyhow!("Failed to import with store error: {:?}", e))?;

    capability_store
        .dictionary_insert(dict_id, &fsandbox::DictionaryItem { key: key.into(), value: id })
        .await
        .map_err(|e| anyhow!("Failed to call dictionary_insert: {:?}", e))?
        .map_err(|e| anyhow!("Failed to dictionary_insert with store error: {:?}", e))
}

async fn compress_and_save_to_dictionary_vmo(
    data: Vec<u8>,
    capability_store: &fsandbox::CapabilityStoreProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    dict_id: u64,
    key: &str,
) -> Result<(), anyhow::Error> {
    log::debug!("{} uncompressed size: {}", key, data.len());
    write_size_to_dictionary(
        data.len(),
        capability_store,
        id_gen.next(),
        dict_id,
        format!("{}_uncompressed_len", key).as_str(),
    )
    .await?;

    let compressed = lz4::compress(data.as_slice());
    save_to_dictionary_vmo(compressed, capability_store, id_gen, dict_id, key).await
}

async fn write_size_to_dictionary(
    size: usize,
    capability_store: &fsandbox::CapabilityStoreProxy,
    id: u64,
    dict_id: u64,
    key: &str,
) -> Result<(), anyhow::Error> {
    capability_store
        .import(id, fsandbox::Capability::Data(fsandbox::Data::Uint64(size as u64)))
        .await
        .map_err(|e| anyhow!("Failed to call import: {:?}", e))?
        .map_err(|e| anyhow!("Failed to import with store error: {:?}", e))?;

    capability_store
        .dictionary_insert(dict_id, &fsandbox::DictionaryItem { key: key.into(), value: id })
        .await
        .map_err(|e| anyhow!("Failed to call dictionary_insert: {:?}", e))?
        .map_err(|e| anyhow!("Failed to dictionary_insert with store error: {:?}", e))
}

async fn get_handle_from_dictionary(
    capability_store: &fsandbox::CapabilityStoreProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    dict_id: u64,
    key: &str,
) -> Result<zx::NullableHandle, anyhow::Error> {
    let dest_id = id_gen.next();
    capability_store
        .dictionary_remove(dict_id, key, Some(&fsandbox::WrappedNewCapabilityId { id: dest_id }))
        .await
        .map_err(|e| anyhow!("Failed to call dictionary_remove: {:?}", e))?
        .map_err(|e| anyhow!("dictionary_remove failed with store error: {:?}", e))?;

    let capability = capability_store
        .export(dest_id)
        .await
        .map_err(|e| anyhow!("Failed to call export: {:?}", e))?
        .map_err(|e| anyhow!("Failed to export with store error: {:?}", e))?;

    match capability {
        fsandbox::Capability::Handle(handle) => Ok(handle),
        _ => Err(anyhow!("unexpected capability type from dictionary for {key}: {capability:?}")),
    }
}

async fn read_vmo_from_dictionary(
    capability_store: &fsandbox::CapabilityStoreProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    dict_id: u64,
    key: &str,
) -> Result<Vec<u8>, anyhow::Error> {
    let handle = get_handle_from_dictionary(capability_store, id_gen, dict_id, key).await?;

    let vmo = zx::Vmo::from(handle);
    let size =
        vmo.get_content_size().map_err(|e| anyhow!("Failed to get vmo content size: {:?}", e))?;
    vmo.read_to_vec(0, size).map_err(|e| anyhow!("Failed to read vmo to vector: {:?}", e))
}

async fn read_vmo_and_decompress_from_dictionary(
    capability_store: &fsandbox::CapabilityStoreProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    dict_id: u64,
    key: &str,
) -> Result<Vec<u8>, anyhow::Error> {
    let bytes = read_vmo_from_dictionary(capability_store, id_gen, dict_id, key).await?;
    let size = read_size_from_dictionary(
        capability_store,
        id_gen,
        dict_id,
        format!("{}_uncompressed_len", key).as_str(),
    )
    .await?;

    lz4::decompress(bytes.as_slice(), size)
        .map_err(|e| anyhow!("Failed to decompress {} {:?}", key, e))
}

async fn read_size_from_dictionary(
    capability_store: &fsandbox::CapabilityStoreProxy,
    id_gen: &sandbox::CapabilityIdGenerator,
    dict_id: u64,
    key: &str,
) -> Result<usize, anyhow::Error> {
    let dest_id = id_gen.next();
    capability_store
        .dictionary_remove(dict_id, key, Some(&fsandbox::WrappedNewCapabilityId { id: dest_id }))
        .await
        .map_err(|e| anyhow!("Failed to call dictionary_remove: {:?}", e))?
        .map_err(|e| anyhow!("dictionary_remove failed with store error: {:?}", e))?;

    let capability = capability_store
        .export(dest_id)
        .await
        .map_err(|e| anyhow!("Failed to call export: {:?}", e))?
        .map_err(|e| anyhow!("Failed to export with store error: {:?}", e))?;

    match capability {
        fsandbox::Capability::Data(data) => match data {
            fsandbox::Data::Uint64(result) => Ok(result as usize),
            _ => Err(anyhow!("unexpected data type {data:?}")),
        },
        _ => Err(anyhow!("unexpected capability type from dictionary for {key}: {capability:?}")),
    }
}
