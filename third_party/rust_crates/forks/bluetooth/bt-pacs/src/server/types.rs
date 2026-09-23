// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Define types and trait implementations specific to the PACS server.

use bt_gatt::client::FromCharacteristic;
use bt_gatt::types::{
    AttributePermissions, CharacteristicProperties, CharacteristicProperty, Handle, SecurityLevels,
};
use bt_gatt::Characteristic;

use std::collections::HashSet;

use crate::*;

pub(crate) const SUPPORTED_AUDIO_CONTEXTS_HANDLE: Handle = Handle(1);
pub(crate) const AVAILABLE_AUDIO_CONTEXTS_HANDLE: Handle = Handle(2);
pub(crate) const HANDLE_OFFSET: u64 = 3;

impl From<&SupportedAudioContexts> for Characteristic {
    fn from(_value: &SupportedAudioContexts) -> Self {
        // TODO(b/309015071): implement optional properties.
        let properties: CharacteristicProperties = CharacteristicProperty::Read.into();

        Characteristic {
            handle: SUPPORTED_AUDIO_CONTEXTS_HANDLE,
            uuid: <SupportedAudioContexts as FromCharacteristic>::UUID,
            properties: properties.clone(),
            permissions: AttributePermissions::with_levels(
                &properties,
                &SecurityLevels::encryption_required(),
            ),
            descriptors: Vec::new(),
        }
    }
}

impl From<&AvailableAudioContexts> for Characteristic {
    fn from(_value: &AvailableAudioContexts) -> Self {
        let properties = CharacteristicProperty::Read | CharacteristicProperty::Notify;

        Characteristic {
            handle: AVAILABLE_AUDIO_CONTEXTS_HANDLE,
            uuid: <AvailableAudioContexts as FromCharacteristic>::UUID,
            properties: properties.clone(),
            permissions: AttributePermissions::with_levels(
                &properties,
                &SecurityLevels::encryption_required(),
            ),
            descriptors: Vec::new(),
        }
    }
}

impl From<&SourcePac> for Characteristic {
    fn from(value: &SourcePac) -> Self {
        // TODO(b/309015071): implement optional properties.
        let properties: CharacteristicProperties = CharacteristicProperty::Read.into();

        Characteristic {
            handle: value.handle,
            uuid: <SourcePac as FromCharacteristic>::UUID,
            properties: properties.clone(),
            permissions: AttributePermissions::with_levels(
                &properties,
                &SecurityLevels::encryption_required(),
            ),
            descriptors: Vec::new(),
        }
    }
}

impl From<&SinkPac> for Characteristic {
    fn from(value: &SinkPac) -> Self {
        // TODO(b/309015071): implement optional properties.
        let properties: CharacteristicProperties = CharacteristicProperty::Read.into();

        Characteristic {
            handle: value.handle,
            uuid: <SinkPac as FromCharacteristic>::UUID,
            properties: properties.clone(),
            permissions: AttributePermissions::with_levels(
                &properties,
                &SecurityLevels::encryption_required(),
            ),
            descriptors: Vec::new(),
        }
    }
}

impl From<&SourceAudioLocations> for Characteristic {
    fn from(value: &SourceAudioLocations) -> Self {
        // TODO(b/309015071): implement optional properties.
        let properties: CharacteristicProperties = CharacteristicProperty::Read.into();

        Characteristic {
            handle: value.handle,
            uuid: <SourceAudioLocations as FromCharacteristic>::UUID,
            properties: properties.clone(),
            permissions: AttributePermissions::with_levels(
                &properties,
                &SecurityLevels::encryption_required(),
            ),
            descriptors: Vec::new(),
        }
    }
}

impl From<&SinkAudioLocations> for Characteristic {
    fn from(value: &SinkAudioLocations) -> Self {
        // TODO(b/309015071): implement optional properties.
        let properties: CharacteristicProperties = CharacteristicProperty::Read.into();

        Characteristic {
            handle: value.handle,
            uuid: <SinkAudioLocations as FromCharacteristic>::UUID,
            properties: properties.clone(),
            permissions: AttributePermissions::with_levels(
                &properties,
                &SecurityLevels::encryption_required(),
            ),
            descriptors: Vec::new(),
        }
    }
}

#[derive(Default)]
pub struct AudioContexts {
    pub(crate) sink: HashSet<ContextType>,
    pub(crate) source: HashSet<ContextType>,
}

impl AudioContexts {
    pub fn new(sink: HashSet<ContextType>, source: HashSet<ContextType>) -> Self {
        AudioContexts { sink, source }
    }
}

/// A single PAC characteristic consists of 1 or more PAC records.
pub type PacRecords = Vec<PacRecord>;

#[derive(Debug, PartialEq)]
pub(crate) enum PublishedAudioCapability {
    Sink(SinkPac),
    Source(SourcePac),
}

impl PublishedAudioCapability {
    pub fn new_sink(handle: Handle, records: PacRecords) -> Self {
        Self::Sink(SinkPac { handle: handle, capabilities: records })
    }

    pub fn new_source(handle: Handle, records: PacRecords) -> Self {
        Self::Source(SourcePac { handle: handle, capabilities: records })
    }

    #[cfg(test)]
    pub fn is_sink(&self) -> bool {
        match self {
            PublishedAudioCapability::Sink(_) => true,
            PublishedAudioCapability::Source(_) => false,
        }
    }

    #[cfg(test)]
    pub fn is_source(&self) -> bool {
        match self {
            PublishedAudioCapability::Sink(_) => false,
            PublishedAudioCapability::Source(_) => true,
        }
    }

    #[cfg(test)]
    pub fn pac_records(&self) -> &Vec<PacRecord> {
        match self {
            PublishedAudioCapability::Sink(pac) => &pac.capabilities,
            PublishedAudioCapability::Source(pac) => &pac.capabilities,
        }
    }

    /// Encode into PAC characteristic format as defined in PACS v1.0.1
    /// Table 3.2/3.4.
    pub(crate) fn encode(&self) -> Vec<u8> {
        match self {
            PublishedAudioCapability::Sink(pac) => pac_records_into_char_value(&pac.capabilities),
            PublishedAudioCapability::Source(pac) => pac_records_into_char_value(&pac.capabilities),
        }
    }
}

impl From<&PublishedAudioCapability> for Characteristic {
    fn from(value: &PublishedAudioCapability) -> Self {
        match value {
            PublishedAudioCapability::Sink(pac) => pac.into(),
            PublishedAudioCapability::Source(pac) => pac.into(),
        }
    }
}
