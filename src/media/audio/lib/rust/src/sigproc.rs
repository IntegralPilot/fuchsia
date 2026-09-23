// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use flex_fuchsia_hardware_audio_signalprocessing as fhaudio_sigproc;
use zx_types;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Topology {
    pub id: fhaudio_sigproc::TopologyId,
    pub edge_pairs: Vec<fhaudio_sigproc::EdgePair>,
}

impl TryFrom<fhaudio_sigproc::Topology> for Topology {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::Topology) -> Result<Self, Self::Error> {
        let id = value.id.ok_or_else(|| "missing 'id'".to_string())?;
        let edge_pairs = value
            .processing_elements_edge_pairs
            .ok_or_else(|| "missing 'processing_elements_edge_pairs'".to_string())?;
        Ok(Self { id, edge_pairs })
    }
}

impl From<Topology> for fhaudio_sigproc::Topology {
    fn from(value: Topology) -> Self {
        Self {
            id: Some(value.id),
            processing_elements_edge_pairs: Some(value.edge_pairs),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    pub id: fhaudio_sigproc::ElementId,
    pub type_: fhaudio_sigproc::ElementType,
    pub type_specific: Option<TypeSpecificElement>,
    pub description: Option<String>,
    pub can_stop: Option<bool>,
    pub can_bypass: Option<bool>,
}

impl TryFrom<fhaudio_sigproc::Element> for Element {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::Element) -> Result<Self, Self::Error> {
        let id = value.id.ok_or_else(|| "missing 'id'".to_string())?;
        let type_ = value.type_.ok_or_else(|| "missing 'type'".to_string())?;
        let type_specific: Option<TypeSpecificElement> =
            value.type_specific.map(TryInto::try_into).transpose()?;
        Ok(Self {
            id,
            type_,
            type_specific,
            description: value.description,
            can_stop: value.can_stop,
            can_bypass: value.can_bypass,
        })
    }
}

impl From<Element> for fhaudio_sigproc::Element {
    fn from(value: Element) -> Self {
        Self {
            id: Some(value.id),
            type_: Some(value.type_),
            type_specific: value.type_specific.map(Into::into),
            description: value.description,
            can_stop: value.can_stop,
            can_bypass: value.can_bypass,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeSpecificElement {
    VendorSpecific(VendorSpecific),
    Gain(Gain),
    Equalizer(Equalizer),
    Dynamics(Dynamics),
    DaiInterconnect(DaiInterconnect),
}

impl TryFrom<fhaudio_sigproc::TypeSpecificElement> for TypeSpecificElement {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::TypeSpecificElement) -> Result<Self, Self::Error> {
        match value {
            fhaudio_sigproc::TypeSpecificElement::VendorSpecific(vendor_specific) => {
                Ok(Self::VendorSpecific(vendor_specific.try_into()?))
            }
            fhaudio_sigproc::TypeSpecificElement::Gain(gain) => Ok(Self::Gain(gain.try_into()?)),
            fhaudio_sigproc::TypeSpecificElement::Equalizer(equalizer) => {
                Ok(Self::Equalizer(equalizer.try_into()?))
            }
            fhaudio_sigproc::TypeSpecificElement::Dynamics(dynamics) => {
                Ok(Self::Dynamics(dynamics.try_into()?))
            }
            fhaudio_sigproc::TypeSpecificElement::DaiInterconnect(dai_interconnect) => {
                Ok(Self::DaiInterconnect(dai_interconnect.try_into()?))
            }
            _ => Err("unknown TypeSpecificElement variant".to_string()),
        }
    }
}

impl From<TypeSpecificElement> for fhaudio_sigproc::TypeSpecificElement {
    fn from(value: TypeSpecificElement) -> Self {
        match value {
            TypeSpecificElement::VendorSpecific(vendor_specific) => {
                Self::VendorSpecific(vendor_specific.into())
            }
            TypeSpecificElement::Gain(gain) => Self::Gain(gain.into()),
            TypeSpecificElement::Equalizer(equalizer) => Self::Equalizer(equalizer.into()),
            TypeSpecificElement::Dynamics(dynamics) => Self::Dynamics(dynamics.into()),
            TypeSpecificElement::DaiInterconnect(dai_interconnect) => {
                Self::DaiInterconnect(dai_interconnect.into())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorSpecific {}

impl TryFrom<fhaudio_sigproc::VendorSpecific> for VendorSpecific {
    type Error = String;

    fn try_from(_value: fhaudio_sigproc::VendorSpecific) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}

impl From<VendorSpecific> for fhaudio_sigproc::VendorSpecific {
    fn from(_value: VendorSpecific) -> Self {
        Default::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gain {
    pub type_: fhaudio_sigproc::GainType,
    pub domain: Option<fhaudio_sigproc::GainDomain>,
    pub range: GainRange,
}

impl TryFrom<fhaudio_sigproc::Gain> for Gain {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::Gain) -> Result<Self, Self::Error> {
        let type_ = value.type_.ok_or_else(|| "missing 'type'".to_string())?;
        let min_gain = value.min_gain.ok_or_else(|| "missing 'min_gain'".to_string())?;
        let max_gain = value.max_gain.ok_or_else(|| "missing 'max_gain'".to_string())?;
        let min_gain_step =
            value.min_gain_step.ok_or_else(|| "missing 'min_gain_step'".to_string())?;
        let range = GainRange { min: min_gain, max: max_gain, min_step: min_gain_step };
        Ok(Self { type_, domain: value.domain, range })
    }
}

impl From<Gain> for fhaudio_sigproc::Gain {
    fn from(value: Gain) -> Self {
        Self {
            type_: Some(value.type_),
            domain: value.domain,
            min_gain: Some(value.range.min),
            max_gain: Some(value.range.max),
            min_gain_step: Some(value.range.min_step),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainRange {
    pub min: f32,
    pub max: f32,
    pub min_step: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Equalizer {
    pub bands: Vec<EqualizerBand>,
    pub supported_controls: Option<fhaudio_sigproc::EqualizerSupportedControls>,
    pub can_disable_bands: Option<bool>,
    pub min_frequency: u32,
    pub max_frequency: u32,
    pub max_q: Option<f32>,
    pub min_gain_db: Option<f32>,
    pub max_gain_db: Option<f32>,
}

impl TryFrom<fhaudio_sigproc::Equalizer> for Equalizer {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::Equalizer) -> Result<Self, Self::Error> {
        let bands: Vec<EqualizerBand> = value
            .bands
            .ok_or_else(|| "missing 'bands'".to_string())?
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
        let min_frequency =
            value.min_frequency.ok_or_else(|| "missing 'min_frequency'".to_string())?;
        let max_frequency =
            value.max_frequency.ok_or_else(|| "missing 'max_frequency'".to_string())?;
        Ok(Self {
            bands,
            supported_controls: value.supported_controls,
            can_disable_bands: value.can_disable_bands,
            min_frequency,
            max_frequency,
            max_q: value.max_q,
            min_gain_db: value.min_gain_db,
            max_gain_db: value.max_gain_db,
        })
    }
}

impl From<Equalizer> for fhaudio_sigproc::Equalizer {
    fn from(value: Equalizer) -> Self {
        let bands: Vec<fhaudio_sigproc::EqualizerBand> =
            value.bands.into_iter().map(Into::into).collect();
        Self {
            bands: Some(bands),
            supported_controls: value.supported_controls,
            can_disable_bands: value.can_disable_bands,
            min_frequency: Some(value.min_frequency),
            max_frequency: Some(value.max_frequency),
            max_q: value.max_q,
            min_gain_db: value.min_gain_db,
            max_gain_db: value.max_gain_db,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EqualizerBand {
    pub id: u64,
}

impl TryFrom<fhaudio_sigproc::EqualizerBand> for EqualizerBand {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::EqualizerBand) -> Result<Self, Self::Error> {
        let id = value.id.ok_or_else(|| "missing 'id'".to_string())?;
        Ok(Self { id })
    }
}

impl From<EqualizerBand> for fhaudio_sigproc::EqualizerBand {
    fn from(value: EqualizerBand) -> Self {
        Self { id: Some(value.id), ..Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dynamics {
    pub bands: Vec<DynamicsBand>,
    pub supported_controls: Option<fhaudio_sigproc::DynamicsSupportedControls>,
}

impl TryFrom<fhaudio_sigproc::Dynamics> for Dynamics {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::Dynamics) -> Result<Self, Self::Error> {
        let bands: Vec<DynamicsBand> = value
            .bands
            .ok_or_else(|| "missing 'bands'".to_string())?
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { bands, supported_controls: value.supported_controls })
    }
}

impl From<Dynamics> for fhaudio_sigproc::Dynamics {
    fn from(value: Dynamics) -> Self {
        let bands: Vec<fhaudio_sigproc::DynamicsBand> =
            value.bands.into_iter().map(Into::into).collect();
        Self {
            bands: Some(bands),
            supported_controls: value.supported_controls,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DynamicsBand {
    pub id: u64,
}

impl TryFrom<fhaudio_sigproc::DynamicsBand> for DynamicsBand {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::DynamicsBand) -> Result<Self, Self::Error> {
        let id = value.id.ok_or_else(|| "missing 'id'".to_string())?;
        Ok(Self { id })
    }
}

impl From<DynamicsBand> for fhaudio_sigproc::DynamicsBand {
    fn from(value: DynamicsBand) -> Self {
        Self { id: Some(value.id), ..Default::default() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DaiInterconnect {
    pub plug_detect_capabilities: fhaudio_sigproc::PlugDetectCapabilities,
}

impl TryFrom<fhaudio_sigproc::DaiInterconnect> for DaiInterconnect {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::DaiInterconnect) -> Result<Self, Self::Error> {
        let plug_detect_capabilities = value
            .plug_detect_capabilities
            .ok_or_else(|| "missing 'plug_detect_capabilities'".to_string())?;
        Ok(Self { plug_detect_capabilities })
    }
}

impl From<DaiInterconnect> for fhaudio_sigproc::DaiInterconnect {
    fn from(value: DaiInterconnect) -> Self {
        Self {
            plug_detect_capabilities: Some(value.plug_detect_capabilities),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ElementState {
    pub type_specific: Option<TypeSpecificElementState>,
    pub vendor_specific_data: Option<Vec<u8>>,
    pub started: bool,
    pub bypassed: Option<bool>,
    pub turn_on_delay_ns: Option<zx_types::zx_duration_t>,
    pub turn_off_delay_ns: Option<zx_types::zx_duration_t>,
    pub processing_delay_ns: Option<zx_types::zx_duration_t>,
}

impl TryFrom<fhaudio_sigproc::ElementState> for ElementState {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::ElementState) -> Result<Self, Self::Error> {
        let type_specific = value.type_specific.map(TryInto::try_into).transpose()?;
        let started = value.started.ok_or_else(|| "missing 'started'".to_string())?;
        Ok(Self {
            type_specific,
            vendor_specific_data: value.vendor_specific_data,
            started,
            bypassed: value.bypassed,
            turn_on_delay_ns: value.turn_on_delay,
            turn_off_delay_ns: value.turn_off_delay,
            processing_delay_ns: value.processing_delay,
        })
    }
}

impl From<ElementState> for fhaudio_sigproc::ElementState {
    fn from(value: ElementState) -> Self {
        Self {
            type_specific: value.type_specific.map(Into::into),
            vendor_specific_data: value.vendor_specific_data,
            started: Some(value.started),
            bypassed: value.bypassed,
            turn_on_delay: value.turn_on_delay_ns,
            turn_off_delay: value.turn_off_delay_ns,
            processing_delay: value.processing_delay_ns,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypeSpecificElementState {
    VendorSpecific(VendorSpecificElementState),
    Gain(GainElementState),
    Equalizer(EqualizerElementState),
    Dynamics(DynamicsElementState),
    DaiInterconnect(DaiInterconnectElementState),
}

impl TryFrom<fhaudio_sigproc::TypeSpecificElementState> for TypeSpecificElementState {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::TypeSpecificElementState) -> Result<Self, Self::Error> {
        match value {
            fhaudio_sigproc::TypeSpecificElementState::VendorSpecific(vendor_specific_state) => {
                Ok(Self::VendorSpecific(vendor_specific_state.try_into()?))
            }
            fhaudio_sigproc::TypeSpecificElementState::Gain(gain_state) => {
                Ok(Self::Gain(gain_state.try_into()?))
            }
            fhaudio_sigproc::TypeSpecificElementState::Equalizer(equalizer_state) => {
                Ok(Self::Equalizer(equalizer_state.try_into()?))
            }
            fhaudio_sigproc::TypeSpecificElementState::Dynamics(dynamics_state) => {
                Ok(Self::Dynamics(dynamics_state.try_into()?))
            }
            fhaudio_sigproc::TypeSpecificElementState::DaiInterconnect(dai_interconnect_state) => {
                Ok(Self::DaiInterconnect(dai_interconnect_state.try_into()?))
            }
            _ => Err("unknown TypeSpecificElementStateState variant".to_string()),
        }
    }
}

impl From<TypeSpecificElementState> for fhaudio_sigproc::TypeSpecificElementState {
    fn from(value: TypeSpecificElementState) -> Self {
        match value {
            TypeSpecificElementState::VendorSpecific(vendor_specific_state) => {
                Self::VendorSpecific(vendor_specific_state.into())
            }
            TypeSpecificElementState::Gain(gain_state) => Self::Gain(gain_state.into()),
            TypeSpecificElementState::Equalizer(equalizer_state) => {
                Self::Equalizer(equalizer_state.into())
            }
            TypeSpecificElementState::Dynamics(dynamics_state) => {
                Self::Dynamics(dynamics_state.into())
            }
            TypeSpecificElementState::DaiInterconnect(dai_interconnect_state) => {
                Self::DaiInterconnect(dai_interconnect_state.into())
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VendorSpecificElementState {}

impl TryFrom<fhaudio_sigproc::VendorSpecificState> for VendorSpecificElementState {
    type Error = String;

    fn try_from(_value: fhaudio_sigproc::VendorSpecificState) -> Result<Self, Self::Error> {
        Ok(Self {})
    }
}

impl From<VendorSpecificElementState> for fhaudio_sigproc::VendorSpecificState {
    fn from(_value: VendorSpecificElementState) -> Self {
        Default::default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GainElementState {
    pub gain: f32,
}

impl TryFrom<fhaudio_sigproc::GainElementState> for GainElementState {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::GainElementState) -> Result<Self, Self::Error> {
        let gain = value.gain.ok_or_else(|| "missing 'gain'".to_string())?;
        Ok(Self { gain })
    }
}

impl From<GainElementState> for fhaudio_sigproc::GainElementState {
    fn from(value: GainElementState) -> Self {
        Self { gain: Some(value.gain), ..Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EqualizerElementState {
    pub band_states: Vec<EqualizerBandState>,
}

impl TryFrom<fhaudio_sigproc::EqualizerElementState> for EqualizerElementState {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::EqualizerElementState) -> Result<Self, Self::Error> {
        let band_states = value
            .band_states
            .ok_or_else(|| "missing 'band_states'".to_string())?
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { band_states })
    }
}

impl From<EqualizerElementState> for fhaudio_sigproc::EqualizerElementState {
    fn from(value: EqualizerElementState) -> Self {
        let band_states: Vec<fhaudio_sigproc::EqualizerBandState> =
            value.band_states.into_iter().map(Into::into).collect();
        Self { band_states: Some(band_states), ..Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EqualizerBandState {
    pub id: u64,
    pub type_: Option<fhaudio_sigproc::EqualizerBandType>,
    pub frequency: Option<u32>,
    pub q: Option<f32>,
    pub gain_db: Option<f32>,
    pub enabled: Option<bool>,
}

impl TryFrom<fhaudio_sigproc::EqualizerBandState> for EqualizerBandState {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::EqualizerBandState) -> Result<Self, Self::Error> {
        let id = value.id.ok_or_else(|| "missing 'id'".to_string())?;
        Ok(Self {
            id,
            type_: value.type_,
            frequency: value.frequency,
            q: value.q,
            gain_db: value.gain_db,
            enabled: value.enabled,
        })
    }
}

impl From<EqualizerBandState> for fhaudio_sigproc::EqualizerBandState {
    fn from(value: EqualizerBandState) -> Self {
        Self {
            id: Some(value.id),
            type_: value.type_,
            frequency: value.frequency,
            q: value.q,
            gain_db: value.gain_db,
            enabled: value.enabled,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DynamicsElementState {
    pub band_states: Vec<DynamicsBandState>,
}

impl TryFrom<fhaudio_sigproc::DynamicsElementState> for DynamicsElementState {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::DynamicsElementState) -> Result<Self, Self::Error> {
        let band_states = value
            .band_states
            .ok_or_else(|| "missing 'band_states'".to_string())?
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { band_states })
    }
}

impl From<DynamicsElementState> for fhaudio_sigproc::DynamicsElementState {
    fn from(value: DynamicsElementState) -> Self {
        let band_states: Vec<fhaudio_sigproc::DynamicsBandState> =
            value.band_states.into_iter().map(Into::into).collect();
        Self { band_states: Some(band_states), ..Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DynamicsBandState {
    pub id: u64,
    pub min_frequency: u32,
    pub max_frequency: u32,
    pub threshold_db: f32,
    pub threshold_type: fhaudio_sigproc::ThresholdType,
    pub ratio: f32,
    pub knee_width_db: Option<f32>,
    pub attack: Option<zx_types::zx_duration_t>,
    pub release: Option<zx_types::zx_duration_t>,
    pub output_gain_db: Option<f32>,
    pub input_gain_db: Option<f32>,
    pub level_type: Option<fhaudio_sigproc::LevelType>,
    pub lookahead: Option<zx_types::zx_duration_t>,
    pub linked_channels: Option<bool>,
}

impl TryFrom<fhaudio_sigproc::DynamicsBandState> for DynamicsBandState {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::DynamicsBandState) -> Result<Self, Self::Error> {
        let id = value.id.ok_or_else(|| "missing 'id'".to_string())?;
        let min_frequency =
            value.min_frequency.ok_or_else(|| "missing 'min_frequency'".to_string())?;
        let max_frequency =
            value.max_frequency.ok_or_else(|| "missing 'max_frequency'".to_string())?;
        let threshold_db =
            value.threshold_db.ok_or_else(|| "missing 'threshold_db'".to_string())?;
        let threshold_type =
            value.threshold_type.ok_or_else(|| "missing 'threshold_type'".to_string())?;
        let ratio = value.ratio.ok_or_else(|| "missing 'ratio'".to_string())?;
        Ok(Self {
            id,
            min_frequency,
            max_frequency,
            threshold_db,
            threshold_type,
            ratio,
            knee_width_db: value.knee_width_db,
            attack: value.attack,
            release: value.release,
            output_gain_db: value.output_gain_db,
            input_gain_db: value.input_gain_db,
            level_type: value.level_type,
            lookahead: value.lookahead,
            linked_channels: value.linked_channels,
        })
    }
}

impl From<DynamicsBandState> for fhaudio_sigproc::DynamicsBandState {
    fn from(value: DynamicsBandState) -> Self {
        Self {
            id: Some(value.id),
            min_frequency: Some(value.min_frequency),
            max_frequency: Some(value.max_frequency),
            threshold_db: Some(value.threshold_db),
            threshold_type: Some(value.threshold_type),
            ratio: Some(value.ratio),
            knee_width_db: value.knee_width_db,
            attack: value.attack,
            release: value.release,
            output_gain_db: value.output_gain_db,
            input_gain_db: value.input_gain_db,
            level_type: value.level_type,
            lookahead: value.lookahead,
            linked_channels: value.linked_channels,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlugState {
    pub plugged: bool,
    pub plug_state_time: zx_types::zx_time_t,
}

impl TryFrom<fhaudio_sigproc::PlugState> for PlugState {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::PlugState) -> Result<Self, Self::Error> {
        let plugged = value.plugged.ok_or_else(|| "missing 'plugged'".to_string())?;
        let plug_state_time =
            value.plug_state_time.ok_or_else(|| "missing 'plug_state_time'".to_string())?;
        Ok(Self { plugged, plug_state_time })
    }
}

impl From<PlugState> for fhaudio_sigproc::PlugState {
    fn from(value: PlugState) -> Self {
        Self {
            plugged: Some(value.plugged),
            plug_state_time: Some(value.plug_state_time),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DaiInterconnectElementState {
    pub plug_state: PlugState,
    pub external_delay_ns: Option<zx_types::zx_duration_t>,
}

impl TryFrom<fhaudio_sigproc::DaiInterconnectElementState> for DaiInterconnectElementState {
    type Error = String;

    fn try_from(value: fhaudio_sigproc::DaiInterconnectElementState) -> Result<Self, Self::Error> {
        let plug_state: PlugState =
            value.plug_state.ok_or_else(|| "missing 'plug_state'".to_string())?.try_into()?;
        Ok(Self { plug_state, external_delay_ns: value.external_delay })
    }
}

impl From<DaiInterconnectElementState> for fhaudio_sigproc::DaiInterconnectElementState {
    fn from(value: DaiInterconnectElementState) -> Self {
        Self {
            plug_state: Some(value.plug_state.into()),
            external_delay: value.external_delay_ns,
            ..Default::default()
        }
    }
}
