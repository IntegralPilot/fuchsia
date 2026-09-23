// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Helpers to serialize fuchsia_audio types with serde.

use base64::engine::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use fdomain_fuchsia_audio_device as fadevice;
use fdomain_fuchsia_hardware_audio_signalprocessing as fhaudio_sigproc;
use fuchsia_audio_fdomain::dai::{DaiFormatSet, DaiFrameFormat, DaiSampleFormat};
use fuchsia_audio_fdomain::device::{
    ClockDomain, GainCapabilities, GainState, PlugEvent, PlugState,
};
use fuchsia_audio_fdomain::format::SampleType;
use fuchsia_audio_fdomain::format_set::{ChannelAttributes, ChannelSet, PcmFormatSet};
use fuchsia_audio_fdomain::sigproc::{
    DaiInterconnect, DaiInterconnectElementState, Dynamics, DynamicsBand, DynamicsBandState,
    DynamicsElementState, Element, ElementState, Equalizer, EqualizerBand, EqualizerBandState,
    EqualizerElementState, Gain, GainElementState, GainRange, Topology, TypeSpecificElement,
    TypeSpecificElementState, VendorSpecific, VendorSpecificElementState,
};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;

/// Serialize an value that can be converted to a string to the string.
pub fn serialize_tostring<S>(value: &impl ToString, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_string())
}

/// Serialize an optional value that can be converted to a string to either the string, or none.
pub fn serialize_option_tostring<S>(
    value: &Option<impl ToString>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(value) = value else { return serializer.serialize_none() };
    serializer.serialize_str(&value.to_string())
}

/// Serialize a vector of values that can be converted to a string to either a sequence
/// of strings, or none.
pub fn serialize_vec_tostring<S>(
    value: &Vec<impl ToString>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut seq = serializer.serialize_seq(Some(value.len()))?;
    for item in value {
        seq.serialize_element(&item.to_string())?;
    }
    seq.end()
}

/// Serialize an optional [ClockDomain] to its number value, or none.
pub fn serialize_option_clockdomain<S>(
    clock_domain: &Option<ClockDomain>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(clock_domain) = clock_domain else { return serializer.serialize_none() };
    serializer.serialize_u32(clock_domain.0)
}

/// Serialize a byte slice as a base64 string.
pub fn serialize_base64<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&BASE64_STANDARD.encode(bytes))
}

/// Serialize an optional [Vec<u8>] as base64, or none.
pub fn serialize_option_vecu8_base64<S>(
    bytes: &Option<Vec<u8>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(bytes) = bytes else { return serializer.serialize_none() };
    serialize_base64(bytes.as_slice(), serializer)
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "GainState")]
pub struct GainStateDef {
    pub gain_db: f32,
    pub muted: Option<bool>,
    pub agc_enabled: Option<bool>,
}

pub fn serialize_option_gainstate<S>(
    gain_state: &Option<GainState>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(gain_state) = gain_state else { return serializer.serialize_none() };
    GainStateDef::serialize(gain_state, serializer)
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "GainCapabilities")]
pub struct GainCapabilitiesDef {
    pub min_gain_db: f32,
    pub max_gain_db: f32,
    pub gain_step_db: f32,
    pub can_mute: Option<bool>,
    pub can_agc: Option<bool>,
}

pub fn serialize_option_gaincapabilities<S>(
    gain_caps: &Option<GainCapabilities>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(gain_caps) = gain_caps else { return serializer.serialize_none() };
    GainCapabilitiesDef::serialize(gain_caps, serializer)
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "PcmFormatSet")]
pub struct PcmFormatSetDef {
    #[serde(serialize_with = "serialize_vec_channelset")]
    pub channel_sets: Vec<ChannelSet>,
    #[serde(serialize_with = "serialize_vec_tostring")]
    pub sample_types: Vec<SampleType>,
    pub frame_rates: Vec<u32>,
}

pub fn serialize_vec_pcmformatset<S>(
    pcm_format_sets: &Vec<PcmFormatSet>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[derive(Serialize)]
    struct Wrapper<'a>(#[serde(with = "PcmFormatSetDef")] &'a PcmFormatSet);

    let mut seq = serializer.serialize_seq(Some(pcm_format_sets.len()))?;
    for pcm_format_set in pcm_format_sets {
        seq.serialize_element(&Wrapper(pcm_format_set))?;
    }
    seq.end()
}

pub fn serialize_option_map_pcmformatset<S>(
    format_set_map: &Option<BTreeMap<fadevice::ElementId, Vec<PcmFormatSet>>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(format_set_map) = format_set_map else { return serializer.serialize_none() };

    #[derive(Serialize)]
    struct Wrapper<'a>(
        #[serde(serialize_with = "serialize_vec_pcmformatset")] &'a Vec<PcmFormatSet>,
    );

    let mut map = serializer.serialize_map(Some(format_set_map.len()))?;
    for (key, value) in format_set_map {
        map.serialize_entry(key, &Wrapper(value))?;
    }
    map.end()
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "ChannelSet")]
pub struct ChannelSetDef {
    #[serde(serialize_with = "serialize_vec_channelattributes")]
    pub attributes: Vec<ChannelAttributes>,
}

pub fn serialize_vec_channelset<S>(
    channel_sets: &Vec<ChannelSet>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[derive(Serialize)]
    struct Wrapper<'a>(#[serde(with = "ChannelSetDef")] &'a ChannelSet);

    let mut seq = serializer.serialize_seq(Some(channel_sets.len()))?;
    for channel_set in channel_sets {
        seq.serialize_element(&Wrapper(channel_set))?;
    }
    seq.end()
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "ChannelAttributes")]
pub struct ChannelAttributesDef {
    pub min_frequency: Option<u32>,
    pub max_frequency: Option<u32>,
}

pub fn serialize_vec_channelattributes<S>(
    channel_attributes: &Vec<ChannelAttributes>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[derive(Serialize)]
    struct Wrapper<'a>(#[serde(with = "ChannelAttributesDef")] &'a ChannelAttributes);

    let mut seq = serializer.serialize_seq(Some(channel_attributes.len()))?;
    for attribute in channel_attributes {
        seq.serialize_element(&Wrapper(attribute))?;
    }
    seq.end()
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "DaiFormatSet")]
pub struct DaiFormatSetDef {
    pub number_of_channels: Vec<u32>,
    #[serde(serialize_with = "serialize_vec_tostring")]
    pub sample_formats: Vec<DaiSampleFormat>,
    #[serde(serialize_with = "serialize_vec_tostring")]
    pub frame_formats: Vec<DaiFrameFormat>,
    pub frame_rates: Vec<u32>,
    pub bits_per_slot: Vec<u8>,
    pub bits_per_sample: Vec<u8>,
}

pub fn serialize_vec_daiformatset<S>(
    dai_format_sets: &Vec<DaiFormatSet>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[derive(Serialize)]
    struct Wrapper<'a>(#[serde(with = "DaiFormatSetDef")] &'a DaiFormatSet);

    let mut seq = serializer.serialize_seq(Some(dai_format_sets.len()))?;
    for dai_format_set in dai_format_sets {
        seq.serialize_element(&Wrapper(dai_format_set))?;
    }
    seq.end()
}

pub fn serialize_option_map_daiformatset<S>(
    format_set_map: &Option<BTreeMap<fadevice::ElementId, Vec<DaiFormatSet>>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(format_set_map) = format_set_map else { return serializer.serialize_none() };

    #[derive(Serialize)]
    struct Wrapper<'a>(
        #[serde(serialize_with = "serialize_vec_daiformatset")] &'a Vec<DaiFormatSet>,
    );

    let mut map = serializer.serialize_map(Some(format_set_map.len()))?;
    for (key, value) in format_set_map {
        map.serialize_entry(key, &Wrapper(value))?;
    }
    map.end()
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "PlugEvent")]
pub struct PlugEventDef {
    #[serde(serialize_with = "serialize_tostring")]
    pub state: PlugState,
    pub time: zx_types::zx_time_t,
}

pub fn serialize_option_plugevent<S>(
    plug_event: &Option<PlugEvent>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(plug_event) = plug_event else { return serializer.serialize_none() };
    PlugEventDef::serialize(plug_event, serializer)
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "Topology")]
pub struct TopologyDef {
    pub id: fhaudio_sigproc::TopologyId,
    #[serde(serialize_with = "serialize_vec_edgepair")]
    pub edge_pairs: Vec<fhaudio_sigproc::EdgePair>,
}

pub fn serialize_option_vec_topology<S>(
    topologies: &Option<Vec<Topology>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(topologies) = topologies else { return serializer.serialize_none() };

    #[derive(Serialize)]
    struct Wrapper<'a>(#[serde(with = "TopologyDef")] &'a Topology);

    let mut seq = serializer.serialize_seq(Some(topologies.len()))?;
    for topology in topologies {
        seq.serialize_element(&Wrapper(topology))?;
    }
    seq.end()
}

// Mirror type for serialization.
#[derive(Serialize)]
#[serde(remote = "fhaudio_sigproc::EdgePair")]
pub struct EdgePairDef {
    pub processing_element_id_from: fhaudio_sigproc::ElementId,
    pub processing_element_id_to: fhaudio_sigproc::ElementId,
}

pub fn serialize_vec_edgepair<S>(
    edge_pairs: &Vec<fhaudio_sigproc::EdgePair>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[derive(Serialize)]
    struct Wrapper<'a>(#[serde(with = "EdgePairDef")] &'a fhaudio_sigproc::EdgePair);

    let mut seq = serializer.serialize_seq(Some(edge_pairs.len()))?;
    for edge_pair in edge_pairs {
        seq.serialize_element(&Wrapper(edge_pair))?;
    }
    seq.end()
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "Element")]
pub struct ElementDef {
    pub id: fhaudio_sigproc::ElementId,
    #[serde(rename = "type", with = "ElementTypeDef")]
    pub type_: fhaudio_sigproc::ElementType,
    #[serde(serialize_with = "serialize_option_typespecificelement")]
    pub type_specific: Option<TypeSpecificElement>,
    #[serde(serialize_with = "serialize_option_tostring")]
    pub description: Option<String>,
    pub can_stop: Option<bool>,
    pub can_bypass: Option<bool>,
}

// Mirror type for serialization.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", remote = "fhaudio_sigproc::ElementType")]
pub enum ElementTypeDef {
    VendorSpecific,
    ConnectionPoint,
    Gain,
    AutomaticGainControl,
    AutomaticGainLimiter,
    Dynamics,
    Mute,
    Delay,
    Equalizer,
    SampleRateConversion,
    RingBuffer,
    DaiInterconnect,
    PacketStream,
    #[serde(skip)]
    __SourceBreaking {
        unknown_ordinal: u32,
    },
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", remote = "TypeSpecificElement")]
pub enum TypeSpecificElementDef {
    VendorSpecific(#[serde(with = "VendorSpecificDef")] VendorSpecific),
    Gain(
        // rustc incorrectly considers this field as never read
        #[allow(dead_code)]
        #[serde(with = "GainDef")]
        Gain,
    ),
    Equalizer(
        // rustc incorrectly considers this field as never read
        #[allow(dead_code)]
        #[serde(with = "EqualizerDef")]
        Equalizer,
    ),
    Dynamics(
        // rustc incorrectly considers this field as never read
        #[allow(dead_code)]
        #[serde(with = "DynamicsDef")]
        Dynamics,
    ),
    DaiInterconnect(
        // rustc incorrectly considers this field as never read
        #[allow(dead_code)]
        #[serde(with = "DaiInterconnectDef")]
        DaiInterconnect,
    ),
}

pub fn serialize_option_typespecificelement<S>(
    type_specific_element: &Option<TypeSpecificElement>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(type_specific_element) = type_specific_element else {
        return serializer.serialize_none();
    };
    TypeSpecificElementDef::serialize(type_specific_element, serializer)
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "VendorSpecific")]
pub struct VendorSpecificDef {}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "Gain")]
pub struct GainDef {
    #[serde(rename = "type", with = "GainTypeDef")]
    pub type_: fhaudio_sigproc::GainType,
    #[serde(serialize_with = "serialize_option_gaindomain")]
    pub domain: Option<fhaudio_sigproc::GainDomain>,
    #[serde(with = "GainRangeDef")]
    pub range: GainRange,
}

pub fn serialize_option_gaindomain<S>(
    gain_domain: &Option<fhaudio_sigproc::GainDomain>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(gain_domain) = gain_domain else {
        return serializer.serialize_none();
    };
    GainDomainDef::serialize(gain_domain, serializer)
}

// Mirror type for serialization.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", remote = "fhaudio_sigproc::GainDomain")]
pub enum GainDomainDef {
    Digital,
    Analog,
    Mixed,
    #[serde(skip)]
    __SourceBreaking {
        unknown_ordinal: u32,
    },
}

// Mirror type for serialization.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", remote = "fhaudio_sigproc::GainType")]
pub enum GainTypeDef {
    Decibels,
    Percent,
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "GainRange")]
pub struct GainRangeDef {
    pub min: f32,
    pub max: f32,
    pub min_step: f32,
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "Equalizer")]
pub struct EqualizerDef {
    #[serde(serialize_with = "serialize_vec_equalizerband")]
    pub bands: Vec<EqualizerBand>,
    #[serde(serialize_with = "serialize_option_equalizersupportedcontrols")]
    pub supported_controls: Option<fhaudio_sigproc::EqualizerSupportedControls>,
    pub can_disable_bands: Option<bool>,
    pub min_frequency: u32,
    pub max_frequency: u32,
    pub max_q: Option<f32>,
    pub min_gain_db: Option<f32>,
    pub max_gain_db: Option<f32>,
}

pub fn serialize_vec_equalizerband<S>(
    equalizer_bands: &Vec<EqualizerBand>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[derive(Serialize)]
    struct Wrapper<'a>(#[serde(with = "EqualizerBandDef")] &'a EqualizerBand);

    let mut seq = serializer.serialize_seq(Some(equalizer_bands.len()))?;
    for equalizer_band in equalizer_bands {
        seq.serialize_element(&Wrapper(equalizer_band))?;
    }
    seq.end()
}

pub fn serialize_option_equalizersupportedcontrols<S>(
    equalizer_supported_controls: &Option<fhaudio_sigproc::EqualizerSupportedControls>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(equalizer_supported_controls) = equalizer_supported_controls else {
        return serializer.serialize_none();
    };
    serializer.serialize_u64(equalizer_supported_controls.bits())
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "EqualizerBand")]
pub struct EqualizerBandDef {
    pub id: u64,
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "Dynamics")]
pub struct DynamicsDef {
    #[serde(serialize_with = "serialize_vec_dynamicsband")]
    pub bands: Vec<DynamicsBand>,
    #[serde(serialize_with = "serialize_option_dynamicssupportedcontrols")]
    pub supported_controls: Option<fhaudio_sigproc::DynamicsSupportedControls>,
}

pub fn serialize_vec_dynamicsband<S>(
    dynamics_bands: &Vec<DynamicsBand>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[derive(Serialize)]
    struct Wrapper<'a>(#[serde(with = "DynamicsBandDef")] &'a DynamicsBand);

    let mut seq = serializer.serialize_seq(Some(dynamics_bands.len()))?;
    for dynamics_band in dynamics_bands {
        seq.serialize_element(&Wrapper(dynamics_band))?;
    }
    seq.end()
}

pub fn serialize_option_dynamicssupportedcontrols<S>(
    dynamics_supported_controls: &Option<fhaudio_sigproc::DynamicsSupportedControls>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(dynamics_supported_controls) = dynamics_supported_controls else {
        return serializer.serialize_none();
    };
    serializer.serialize_u64(dynamics_supported_controls.bits())
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "DynamicsBand")]
pub struct DynamicsBandDef {
    pub id: u64,
}

// Mirror type for serialization.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", remote = "fhaudio_sigproc::PlugDetectCapabilities")]
pub enum PlugDetectCapabilitiesDef {
    Hardwired,
    CanAsyncNotify,
    #[serde(skip)]
    __SourceBreaking {
        unknown_ordinal: u32,
    },
}
// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "DaiInterconnect")]
pub struct DaiInterconnectDef {
    #[serde(with = "PlugDetectCapabilitiesDef")]
    pub plug_detect_capabilities: fhaudio_sigproc::PlugDetectCapabilities,
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "ElementState")]
pub struct ElementStateDef {
    #[serde(serialize_with = "serialize_option_typespecificelementstate")]
    type_specific: Option<TypeSpecificElementState>,
    #[serde(serialize_with = "serialize_option_vecu8_base64")]
    vendor_specific_data: Option<Vec<u8>>,
    started: bool,
    bypassed: Option<bool>,
    turn_on_delay_ns: Option<zx_types::zx_duration_t>,
    turn_off_delay_ns: Option<zx_types::zx_duration_t>,
    processing_delay_ns: Option<zx_types::zx_duration_t>,
}

pub fn serialize_option_elementstate<S>(
    element_state: &Option<ElementState>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(element_state) = element_state else {
        return serializer.serialize_none();
    };
    ElementStateDef::serialize(element_state, serializer)
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", remote = "TypeSpecificElementState")]
pub enum TypeSpecificElementStateDef {
    VendorSpecific(#[serde(with = "VendorSpecificElementStateDef")] VendorSpecificElementState),
    Gain(
        // rustc incorrectly considers this field as never read
        #[allow(dead_code)]
        #[serde(with = "GainElementStateDef")]
        GainElementState,
    ),
    Equalizer(
        // rustc incorrectly considers this field as never read
        #[allow(dead_code)]
        #[serde(with = "EqualizerElementStateDef")]
        EqualizerElementState,
    ),
    Dynamics(
        // rustc incorrectly considers this field as never read
        #[allow(dead_code)]
        #[serde(with = "DynamicsElementStateDef")]
        DynamicsElementState,
    ),
    DaiInterconnect(
        // rustc incorrectly considers this field as never read
        #[allow(dead_code)]
        #[serde(with = "DaiInterconnectElementStateDef")]
        DaiInterconnectElementState,
    ),
}

pub fn serialize_option_typespecificelementstate<S>(
    type_specific_element_state: &Option<TypeSpecificElementState>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(type_specific_element_state) = type_specific_element_state else {
        return serializer.serialize_none();
    };
    TypeSpecificElementStateDef::serialize(type_specific_element_state, serializer)
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "VendorSpecificElementState")]
pub struct VendorSpecificElementStateDef {}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "GainElementState")]
pub struct GainElementStateDef {
    pub gain: f32,
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "EqualizerElementState")]
pub struct EqualizerElementStateDef {
    #[serde(serialize_with = "serialize_vec_equalizerbandstate")]
    pub band_states: Vec<EqualizerBandState>,
}

pub fn serialize_vec_equalizerbandstate<S>(
    band_states: &Vec<EqualizerBandState>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[derive(Serialize)]
    struct Wrapper<'a>(#[serde(with = "EqualizerBandStateDef")] &'a EqualizerBandState);

    let mut seq = serializer.serialize_seq(Some(band_states.len()))?;
    for band_state in band_states {
        seq.serialize_element(&Wrapper(band_state))?;
    }
    seq.end()
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "EqualizerBandState")]
pub struct EqualizerBandStateDef {
    pub id: u64,
    #[serde(rename = "type", serialize_with = "serialize_option_equalizerbandtype")]
    pub type_: Option<fhaudio_sigproc::EqualizerBandType>,
    pub frequency: Option<u32>,
    pub q: Option<f32>,
    pub gain_db: Option<f32>,
    pub enabled: Option<bool>,
}

// Mirror type for serialization.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", remote = "fhaudio_sigproc::EqualizerBandType")]
pub enum EqualizerBandTypeDef {
    Peak,
    Notch,
    LowCut,
    HighCut,
    LowShelf,
    HighShelf,
    #[serde(skip)]
    __SourceBreaking {
        unknown_ordinal: u32,
    },
}

pub fn serialize_option_equalizerbandtype<S>(
    equalizer_band_type: &Option<fhaudio_sigproc::EqualizerBandType>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(equalizer_band_type) = equalizer_band_type else {
        return serializer.serialize_none();
    };
    EqualizerBandTypeDef::serialize(equalizer_band_type, serializer)
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "DynamicsElementState")]
pub struct DynamicsElementStateDef {
    #[serde(serialize_with = "serialize_vec_dynamicsbandstate")]
    pub band_states: Vec<DynamicsBandState>,
}

pub fn serialize_vec_dynamicsbandstate<S>(
    band_states: &Vec<DynamicsBandState>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[derive(Serialize)]
    struct Wrapper<'a>(#[serde(with = "DynamicsBandStateDef")] &'a DynamicsBandState);

    let mut seq = serializer.serialize_seq(Some(band_states.len()))?;
    for band_state in band_states {
        seq.serialize_element(&Wrapper(band_state))?;
    }
    seq.end()
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "DynamicsBandState")]
pub struct DynamicsBandStateDef {
    pub id: u64,
    pub min_frequency: u32,
    pub max_frequency: u32,
    pub threshold_db: f32,
    #[serde(with = "ThresholdTypeDef")]
    pub threshold_type: fhaudio_sigproc::ThresholdType,
    pub ratio: f32,
    pub knee_width_db: Option<f32>,
    pub attack: Option<zx_types::zx_duration_t>,
    pub release: Option<zx_types::zx_duration_t>,
    pub output_gain_db: Option<f32>,
    pub input_gain_db: Option<f32>,
    #[serde(serialize_with = "serialize_option_leveltype")]
    pub level_type: Option<fhaudio_sigproc::LevelType>,
    pub lookahead: Option<zx_types::zx_duration_t>,
    pub linked_channels: Option<bool>,
}

// Mirror type for serialization.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", remote = "fhaudio_sigproc::ThresholdType")]
pub enum ThresholdTypeDef {
    Above,
    Below,
}

// Mirror type for serialization.
#[derive(Serialize)]
#[serde(rename_all = "snake_case", remote = "fhaudio_sigproc::LevelType")]
pub enum LevelTypeDef {
    Peak,
    Rms,
}

pub fn serialize_option_leveltype<S>(
    level_type: &Option<fhaudio_sigproc::LevelType>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(level_type) = level_type else {
        return serializer.serialize_none();
    };
    LevelTypeDef::serialize(level_type, serializer)
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "DaiInterconnectElementState")]
pub struct DaiInterconnectElementStateDef {
    #[serde(with = "SigprocPlugStateDef")]
    pub plug_state: fuchsia_audio_fdomain::sigproc::PlugState,
    pub external_delay_ns: Option<zx_types::zx_duration_t>,
}

// Mirror type for serialization.
// This exists so that fuchsia_audio (and fuchsia_audio_fdomain) need not take a
// dependency on serde.
#[derive(Serialize)]
#[serde(remote = "fuchsia_audio_fdomain::sigproc::PlugState")]
pub struct SigprocPlugStateDef {
    plugged: bool,
    plug_state_time: zx_types::zx_time_t,
}
