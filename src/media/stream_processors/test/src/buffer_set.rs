// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Handles negotiating buffer sets with the codec server and sysmem.

use crate::Result;
use crate::buffer_collection_constraints::*;
use anyhow::Context as _;
use fidl::endpoints::{ClientEnd, Proxy, create_endpoints};
use fidl_fuchsia_media::*;
use fidl_fuchsia_sysmem2::*;
use fuchsia_component::client;
use fuchsia_stream_processors::*;
use log::debug;
use std::fmt;
use std::iter::StepBy;
use std::ops::RangeFrom;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    ReclaimClientTokenChannel,
    ServerOmittedBufferVmo,
    PacketReferencesInvalidBuffer,
    VmoReadFail(zx::Status),
}

impl fmt::Display for Error {
    fn fmt(&self, w: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self, w)
    }
}

/// The pattern to use when advancing ordinals.
#[derive(Debug, Clone, Copy)]
pub enum OrdinalPattern {
    /// Odd ordinal pattern starts at 1 and moves in increments of 2: [1,3,5..]
    Odd,
    /// All ordinal pattern starts at 1 and moves in increments of 1: [1,2,3..]
    All,
}

impl IntoIterator for OrdinalPattern {
    type Item = u64;
    type IntoIter = StepBy<RangeFrom<Self::Item>>;
    fn into_iter(self) -> Self::IntoIter {
        let (start, step) = match self {
            OrdinalPattern::Odd => (1, 2),
            OrdinalPattern::All => (1, 1),
        };
        (start..).step_by(step)
    }
}

pub fn get_ordinal(pattern: &mut <OrdinalPattern as IntoIterator>::IntoIter) -> u64 {
    pattern.next().expect("Getting next item in infinite pattern")
}

pub enum BufferSetType {
    Input,
    Output,
}

pub struct BufferSetFactory;

fn set_allocator_name(sysmem_client: &fidl_fuchsia_sysmem2::AllocatorProxy) -> Result<()> {
    let name = fuchsia_runtime::process_self().get_name()?;
    let koid = fuchsia_runtime::process_self().koid()?;
    Ok(sysmem_client.set_debug_client_info(&AllocatorSetDebugClientInfoRequest {
        name: Some(name.to_string()),
        id: Some(koid.raw_koid()),
        ..Default::default()
    })?)
}

// This client only intends to be filling one input buffer or hashing one output buffer at any given
// time.
const MIN_BUFFER_COUNT_FOR_CAMPING: u32 = 1;

impl BufferSetFactory {
    pub async fn buffer_set(
        buffer_lifetime_ordinal: u64,
        constraints: ValidStreamBufferConstraints,
        codec: &mut StreamProcessorProxy,
        buffer_set_type: BufferSetType,
        buffer_collection_constraints: Option<BufferCollectionConstraints>,
    ) -> Result<BufferSet> {
        let (collection_client, settings) =
            Self::settings(buffer_lifetime_ordinal, constraints, buffer_collection_constraints)
                .await?;

        debug!("Got settings; waiting for buffers. {:?}", settings);

        match buffer_set_type {
            BufferSetType::Input => codec
                .set_input_buffer_partial_settings(settings)
                .context("Sending input partial settings to codec")?,
            BufferSetType::Output => codec
                .set_output_buffer_partial_settings(settings)
                .context("Sending output partial settings to codec")?,
        };

        let wait_result = collection_client
            .wait_for_all_buffers_allocated()
            .await
            .context("Waiting for buffers")?;
        debug!("Sysmem responded (None is success): {:?}", wait_result.as_ref().err());
        let collection_info = wait_result
            .map_err(|err| anyhow::format_err!("sysmem allocation error: {:?}", err))?
            .buffer_collection_info
            .unwrap();

        if let BufferSetType::Output = buffer_set_type {
            debug!("Completing settings for output.");
            codec.complete_output_buffer_partial_settings(buffer_lifetime_ordinal)?;
        }

        debug!(
            "Got {} buffers of size {:?}",
            collection_info.buffers.as_ref().unwrap().len(),
            collection_info
                .settings
                .as_ref()
                .unwrap()
                .buffer_settings
                .as_ref()
                .unwrap()
                .size_bytes
                .as_ref()
                .unwrap()
        );
        debug!("Buffer collection is: {:#?}", collection_info.settings.as_ref().unwrap());
        for (i, buffer) in collection_info.buffers.as_ref().unwrap().iter().enumerate() {
            debug!("Buffer {} is : {:#?}", i, buffer);
        }

        Ok(BufferSet::try_from(BufferSetSpec {
            proxy: collection_client,
            buffer_lifetime_ordinal,
            collection_info,
        })?)
    }

    async fn settings(
        buffer_lifetime_ordinal: u64,
        constraints: ValidStreamBufferConstraints,
        buffer_collection_constraints: Option<BufferCollectionConstraints>,
    ) -> Result<(BufferCollectionProxy, StreamBufferPartialSettings)> {
        let (client_token, client_token_request) =
            create_endpoints::<BufferCollectionTokenMarker>();
        let (codec_token, codec_token_request) = create_endpoints::<BufferCollectionTokenMarker>();
        let client_token = client_token.into_proxy();

        let sysmem_client =
            client::connect_to_protocol::<AllocatorMarker>().context("Connecting to sysmem")?;

        set_allocator_name(&sysmem_client).context("Setting sysmem allocator name")?;

        sysmem_client
            .allocate_shared_collection(AllocatorAllocateSharedCollectionRequest {
                token_request: Some(client_token_request),
                ..Default::default()
            })
            .context("Allocating shared collection")?;
        client_token.duplicate(BufferCollectionTokenDuplicateRequest {
            rights_attenuation_mask: Some(fidl::Rights::SAME_RIGHTS),
            token_request: Some(codec_token_request),
            ..Default::default()
        })?;

        let (collection_client, collection_request) = create_endpoints::<BufferCollectionMarker>();
        sysmem_client.bind_shared_collection(AllocatorBindSharedCollectionRequest {
            token: Some(
                client_token.into_client_end().map_err(|_| Error::ReclaimClientTokenChannel)?,
            ),
            buffer_collection_request: Some(collection_request),
            ..Default::default()
        })?;
        let collection_client = collection_client.into_proxy();
        collection_client.sync().await.context("Syncing codec_token_request with sysmem")?;

        let mut collection_constraints = buffer_collection_constraints
            .unwrap_or_else(|| buffer_collection_constraints_default());
        assert!(
            collection_constraints.min_buffer_count_for_camping.is_none(),
            "min_buffer_count_for_camping should be un-set before we've set it"
        );
        collection_constraints.min_buffer_count_for_camping = Some(MIN_BUFFER_COUNT_FOR_CAMPING);

        debug!("Our buffer collection constraints are: {:#?}", collection_constraints);

        collection_client
            .set_constraints(BufferCollectionSetConstraintsRequest {
                constraints: Some(collection_constraints),
                ..Default::default()
            })
            .context("Sending buffer constraints to sysmem")?;

        Ok((
            collection_client,
            StreamBufferPartialSettings {
                buffer_lifetime_ordinal: Some(buffer_lifetime_ordinal),
                buffer_constraints_version_ordinal: Some(
                    constraints.buffer_constraints_version_ordinal,
                ),
                // A sysmem token channel serves both sysmem(1) and sysmem2 token protocols, so we
                // can convert here until StreamBufferPartialSettings has a sysmem2 token field.
                sysmem_token: Some(
                    ClientEnd::<fidl_fuchsia_sysmem::BufferCollectionTokenMarker>::new(
                        codec_token.into_channel(),
                    ),
                ),
                ..Default::default()
            },
        ))
    }
}

struct BufferSetSpec {
    proxy: BufferCollectionProxy,
    buffer_lifetime_ordinal: u64,
    collection_info: BufferCollectionInfo,
}

#[derive(Debug, PartialEq)]
pub struct Buffer {
    pub data: zx::Vmo,
    pub start: u64,
    pub size: u64,
}

#[derive(Debug)]
pub struct BufferSet {
    pub proxy: BufferCollectionProxy,
    pub buffers: Vec<Buffer>,
    pub buffer_lifetime_ordinal: u64,
    pub buffer_size: usize,
}

impl TryFrom<BufferSetSpec> for BufferSet {
    type Error = anyhow::Error;
    fn try_from(mut src: BufferSetSpec) -> std::result::Result<Self, Self::Error> {
        let buffer_size = *src
            .collection_info
            .settings
            .as_ref()
            .unwrap()
            .buffer_settings
            .as_ref()
            .unwrap()
            .size_bytes
            .as_ref()
            .unwrap();
        let buffer_count = src.collection_info.buffers.as_ref().unwrap().len();

        let mut buffers = vec![];
        for (i, buffer) in src.collection_info.buffers.as_mut().unwrap().iter_mut().enumerate() {
            buffers.push(Buffer {
                data: buffer.vmo.take().ok_or(Error::ServerOmittedBufferVmo).with_context(
                    || {
                        format!(
                            "Trying to ingest {}th buffer of {}: {:#?}",
                            i, buffer_count, buffer
                        )
                    },
                )?,
                start: *buffer.vmo_usable_start.as_ref().unwrap(),
                size: buffer_size,
            });
        }

        Ok(Self {
            proxy: src.proxy,
            buffers,
            buffer_lifetime_ordinal: src.buffer_lifetime_ordinal,
            buffer_size: buffer_size as usize,
        })
    }
}

impl BufferSet {
    pub fn read_packet(&self, packet: &ValidPacket) -> Result<Vec<u8>> {
        let buffer = self
            .buffers
            .get(packet.buffer_index as usize)
            .ok_or(Error::PacketReferencesInvalidBuffer)?;
        let mut dest = vec![0; packet.valid_length_bytes as usize];
        buffer.data.read(&mut dest, packet.start_offset as u64).map_err(Error::VmoReadFail)?;
        Ok(dest)
    }
}
