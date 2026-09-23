// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{connect, serde_ext};
use camino::Utf8PathBuf;
use fac::{DEFAULT_DAI_INTERCONNECT_ELEMENT_ID, DEFAULT_RING_BUFFER_ELEMENT_ID};
use fdomain_fuchsia_audio_controller as fac;
use fdomain_fuchsia_audio_device as fadevice;
use fdomain_fuchsia_hardware_audio as fhaudio;
use fdomain_fuchsia_hardware_audio_signalprocessing as fhaudio_sigproc;
use fdomain_fuchsia_io as fio;
use ffx_command_error::{FfxContext as _, Result, bug, user_error};
use fuchsia_audio_fdomain::Registry;
use fuchsia_audio_fdomain::dai::{
    DaiFormatSet, DaiFrameFormat, DaiFrameFormatClocking, DaiFrameFormatJustification,
};
use fuchsia_audio_fdomain::device::{
    ClockDomain, DevfsSelector, GainCapabilities, GainState, Info as DeviceInfo,
    PlugDetectCapabilities, PlugEvent, RegistrySelector, Selector, UniqueInstanceId,
};
use fuchsia_audio_fdomain::format_set::{ChannelAttributes, ChannelSet, PcmFormatSet};
use fuchsia_audio_fdomain::sigproc::{
    DaiInterconnect, DaiInterconnectElementState, Dynamics, DynamicsBand, DynamicsBandState,
    DynamicsElementState, Element, ElementState, Equalizer, EqualizerBand, EqualizerBandState,
    EqualizerElementState, Gain, GainElementState, Topology, TypeSpecificElement,
    TypeSpecificElementState, VendorSpecific, VendorSpecificElementState,
};
use itertools::Itertools;
use prettytable::{Table, format, row};
use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt::{Display, Write};
use std::sync::LazyLock;
use zx_status::Status;

// No padding, no borders.
pub static TABLE_FORMAT_EMPTY: LazyLock<format::TableFormat> =
    LazyLock::new(|| format::FormatBuilder::new().build());

// Left padding, used for the outer table.
pub static TABLE_FORMAT_NORMAL: LazyLock<format::TableFormat> =
    LazyLock::new(|| format::FormatBuilder::new().padding(2, 0).build());

// With borders, used nested tables.
pub static TABLE_FORMAT_NESTED: LazyLock<format::TableFormat> = LazyLock::new(|| {
    format::FormatBuilder::new()
        .borders('│')
        .separators(&[format::LinePosition::Top], format::LineSeparator::new('─', '┬', '┌', '┐'))
        .separators(&[format::LinePosition::Bottom], format::LineSeparator::new('─', '┴', '└', '┘'))
        .indent(2)
        .padding(1, 1)
        .build()
});

// Right padding, no borders.
pub static TABLE_FORMAT_NESTED_COMPACT: LazyLock<format::TableFormat> =
    LazyLock::new(|| format::FormatBuilder::new().column_separator(' ').build());

/// Formatter for a byte slice as hex digits in groups of 16 bits, separated by spaces.
struct HexBytesText<'a>(&'a [u8]);

impl Display for HexBytesText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (idx, b) in self.0.iter().enumerate() {
            if idx != 0 && idx % 2 == 0 {
                f.write_char(' ')?;
            }
            write!(f, "{:02x}", b)?;
        }
        Ok(())
    }
}

/// Formatter for a byte slice as a series of ASCII characters.
///
/// If a byte is not in the ASCII graphic character range, outputs a dot (.) instead.
struct AsciiBytesText<'a>(&'a [u8]);

impl Display for AsciiBytesText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for b in self.0 {
            f.write_char(if b.is_ascii_graphic() { char::from(*b) } else { '.' })?;
        }
        Ok(())
    }
}

/// Formatter for opaque byte data.
struct BytesText<'a>(&'a [u8]);

impl Display for BytesText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (chunk_idx, chunk) in self.0.chunks(0x10).enumerate() {
            if chunk_idx != 0 {
                writeln!(f)?;
            }
            write!(f, "{:04x}: ", chunk_idx)?;
            HexBytesText(chunk).fmt(f)?;
            f.write_str("  ")?;
            AsciiBytesText(chunk).fmt(f)?;
        }
        Ok(())
    }
}

/// Formatter that outputs the [Debug] representation of a value.
pub struct DebugText<T: std::fmt::Debug>(T);

impl<T: std::fmt::Debug> Display for DebugText<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

/// Formatter for a value in decibels.
pub struct DecibelsText<T: Display>(T);

impl<T: Display> Display for DecibelsText<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} dB", self.0)
    }
}

/// Formatter for a percentage value.
pub struct PercentText<T: Display>(T);

impl<T: Display> Display for PercentText<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}%", self.0)
    }
}

/// Formatter for a boolean as a yes/no message.
pub struct YesNoText(bool);

impl Display for YesNoText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = if self.0 { "✅ Yes" } else { "❌ No" };
        f.write_str(s)
    }
}

/// Formatter for an optional boolean and a subject as a can/cannot message.
pub struct CanCannotText<'a>(Option<bool>, &'a str);

impl Display for CanCannotText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Some(value) => {
                let s = if value { "✅ Can" } else { "❌ Cannot" };
                write!(f, "{} {}", s, self.1)
            }
            None => write!(f, "❌ Cannot {} <missing>", self.1),
        }
    }
}

/// Formatter for a Zircon duration in nanoseconds.
pub struct DurationText(zx_types::zx_duration_t);

impl Display for DurationText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ns", self.0)
    }
}

/// Formatter for an integer frequency in hertz.
pub struct HertzText(u32);

impl Display for HertzText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} Hz", self.0)
    }
}

/// Formatter for gain of a particular type.
struct GainValueText(f32, fhaudio_sigproc::GainType);

impl Display for GainValueText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.1 {
            fhaudio_sigproc::GainType::Decibels => DecibelsText(self.0).fmt(f),
            fhaudio_sigproc::GainType::Percent => PercentText(self.0).fmt(f),
        }
    }
}

/// Formatter for [GainState].
struct GainStateText<'a>(&'a GainState);

impl Display for GainStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let muted = match self.0.muted {
            Some(muted) => {
                if muted {
                    "muted"
                } else {
                    "unmuted"
                }
            }
            None => "Muted not available",
        };

        let agc_enabled = match self.0.agc_enabled {
            Some(agc) => {
                if agc {
                    "AGC on"
                } else {
                    "AGC off"
                }
            }
            None => "AGC not available",
        };

        write!(f, "{} ({}, {})", DecibelsText(self.0.gain_db), muted, agc_enabled)
    }
}

/// Formatter for [GainCapabilities].
struct GainCapabilitiesText<'a>(&'a GainCapabilities);

impl Display for GainCapabilitiesText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let gain_range = if self.0.min_gain_db == self.0.max_gain_db {
            format!("Fixed {} gain", DecibelsText(self.0.min_gain_db))
        } else {
            format!("[{}, {}]", DecibelsText(self.0.min_gain_db), DecibelsText(self.0.max_gain_db))
        };

        let gain_step = if self.0.gain_step_db == 0.0f32 {
            "0 dB step (continuous)".to_string()
        } else {
            format!("{} step", DecibelsText(self.0.gain_step_db))
        };

        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);

        table.add_row(row!("Gain range:", gain_range));
        table.add_row(row!("Gain step:", gain_step));
        table.add_row(row!("Mute:", CanCannotText(self.0.can_mute, "Mute")));
        table.add_row(row!("AGC:", CanCannotText(self.0.can_agc, "AGC")));

        table.fmt(f)
    }
}

/// Formatter for [PlugEvent].
struct PlugEventText<'a>(&'a PlugEvent);

impl Display for PlugEventText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}", self.0.state, self.0.time)
    }
}

/// Formatter for a vector of [ChannelSet]s.
struct ChannelSetsText<'a>(&'a Vec<ChannelSet>);

impl Display for ChannelSetsText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        for channel_set in self.0 {
            let key = format!(
                "{} {}:",
                channel_set.attributes.len(),
                if channel_set.attributes.len() == 1 { "channel" } else { "channels" }
            );
            let value = ChannelSetText(channel_set);
            table.add_row(row!(key, value));
        }
        table.fmt(f)
    }
}

/// Formatter for [ChannelSet].
struct ChannelSetText<'a>(&'a ChannelSet);

impl Display for ChannelSetText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        for (idx, attributes) in self.0.attributes.iter().enumerate() {
            let key = format!("Channel {}:", idx + 1);
            let value = ChannelAttributesText(attributes);
            table.add_row(row!(key, value));
        }
        table.fmt(f)
    }
}

/// Formatter for [ChannelAttributes].
struct ChannelAttributesText<'a>(&'a ChannelAttributes);

impl Display for ChannelAttributesText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        table.add_row(row!(
            "Min frequency:",
            or_full_range_none(&self.0.min_frequency.map(HertzText))
        ));
        table.add_row(row!(
            "Max frequency:",
            or_full_range_none(&self.0.max_frequency.map(HertzText))
        ));
        table.fmt(f)
    }
}

/// Formatter for [PcmFormatSet].
struct PcmFormatSetText<'a>(&'a PcmFormatSet);

impl Display for PcmFormatSetText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sample_types = self.0.sample_types.iter().map(ToString::to_string).join(", ");
        let frame_rates = self
            .0
            .frame_rates
            .iter()
            .map(|frame_rate| HertzText(*frame_rate).to_string())
            .join(", ");
        let num_channels = self.0.channel_sets.iter().map(|set| set.channels()).join(", ");

        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED);
        table.add_row(row!("Sample types:", sample_types));
        table.add_row(row!("Frame rates:", frame_rates));
        table.add_row(row!("Number of channels:", num_channels));
        table.add_row(row!("Channel attributes:", ChannelSetsText(&self.0.channel_sets)));
        table.fmt(f)
    }
}

/// Formatter for a map of ring buffer element IDs to their supported formats.
struct SupportedRingBufferFormatsText<'a>(&'a BTreeMap<fadevice::ElementId, Vec<PcmFormatSet>>);

impl Display for SupportedRingBufferFormatsText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_EMPTY);
        if self.0.is_empty() {
            table.add_row(row!["No format sets available."]);
        } else {
            for (element_id, format_sets) in self.0 {
                table.add_row(row![format!(
                    "• Element {} has {} format {}:",
                    element_id,
                    format_sets.len(),
                    if format_sets.len() == 1 { "set" } else { "sets" }
                )]);
                for format_set in format_sets {
                    table.add_row(row![PcmFormatSetText(format_set)]);
                }
            }
        }
        table.fmt(f)
    }
}

/// Formatter for a vector of [DaiFrameFormat]s.
struct DaiFrameFormatsText<'a>(&'a Vec<DaiFrameFormat>);

impl Display for DaiFrameFormatsText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_EMPTY);
        for dai_frame_format in self.0 {
            match dai_frame_format {
                DaiFrameFormat::Standard(_) => {
                    table.add_row(row![dai_frame_format.to_string()]);
                }
                DaiFrameFormat::Custom(custom_format) => {
                    let mut format_table = Table::new();
                    format_table.set_format(*TABLE_FORMAT_NESTED);
                    format_table.set_titles(row![dai_frame_format.to_string()]);
                    format_table.add_empty_row();
                    format_table.add_row(row![
                        "Justification:",
                        match custom_format.justification {
                            DaiFrameFormatJustification::Left => "Left",
                            DaiFrameFormatJustification::Right => "Right",
                        }
                    ]);
                    format_table.add_row(row![
                        "Clocking:",
                        match custom_format.clocking {
                            DaiFrameFormatClocking::RaisingSclk => "Raising sclk",
                            DaiFrameFormatClocking::FallingSclk => "Falling sclk",
                        }
                    ]);
                    format_table.add_row(row![
                        "Frame sync offset (sclks):",
                        custom_format.frame_sync_sclks_offset
                    ]);
                    format_table
                        .add_row(row!["Frame sync size (sclks):", custom_format.frame_sync_size]);
                    table.add_row(row![format_table]);
                }
            }
        }
        table.fmt(f)
    }
}

/// Formatter for [DaiFormatSet].
struct DaiFormatSetText<'a>(&'a DaiFormatSet);

impl Display for DaiFormatSetText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let dai_format_set = self.0;
        let number_of_channels =
            dai_format_set.number_of_channels.iter().map(ToString::to_string).join(", ");
        let sample_formats =
            dai_format_set.sample_formats.iter().map(ToString::to_string).join(", ");
        let frame_formats = DaiFrameFormatsText(&dai_format_set.frame_formats);
        let frame_rates = dai_format_set
            .frame_rates
            .iter()
            .map(|frame_rate| HertzText(*frame_rate).to_string())
            .join(", ");
        let bits_per_slot = dai_format_set.bits_per_slot.iter().map(ToString::to_string).join(", ");
        let bits_per_sample =
            dai_format_set.bits_per_sample.iter().map(ToString::to_string).join(", ");

        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED);
        table.add_row(row!("Number of channels:", number_of_channels));
        table.add_row(row!("Sample formats:", sample_formats));
        table.add_row(row!("Frame formats:", frame_formats));
        table.add_row(row!("Frame rates:", frame_rates));
        table.add_row(row!("Bits per slot:", bits_per_slot));
        table.add_row(row!("Bits per sample:", bits_per_sample));
        table.fmt(f)
    }
}

/// Formatter for a map of DAI interconnect element IDs to their supported formats.
struct SupportedDaiFormatsText<'a>(&'a BTreeMap<fadevice::ElementId, Vec<DaiFormatSet>>);

impl Display for SupportedDaiFormatsText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_EMPTY);
        if self.0.is_empty() {
            table.add_row(row!["No format sets available."]);
        } else {
            for (element_id, dai_format_sets) in self.0 {
                table.add_row(row![format!(
                    "• Element {} has {} DAI format {}:",
                    element_id,
                    dai_format_sets.len(),
                    if dai_format_sets.len() == 1 { "set" } else { "sets" }
                )]);
                for dai_format_set in dai_format_sets {
                    table.add_row(row![DaiFormatSetText(dai_format_set)]);
                }
            }
        }
        table.fmt(f)
    }
}

/// Formatter for a vector of signal processing topologies.
struct TopologiesText<'a>(&'a Vec<Topology>);

impl Display for TopologiesText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_EMPTY);
        if self.0.is_empty() {
            table.add_row(row!["No topologies available."]);
        } else {
            for topology in self.0 {
                table.add_row(row![TopologyText(topology)]);
            }
        }
        table.fmt(f)
    }
}

/// Formatter for a signal processing [Topology].
struct TopologyText<'a>(&'a Topology);

impl Display for TopologyText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED);
        table.add_row(row!("ID:", self.0.id));
        table.add_row(row!("Element edges:", EdgePairsText(&self.0.edge_pairs)));
        table.fmt(f)
    }
}

/// Formatter for a vector of signal processing [EdgePair]s.
struct EdgePairsText<'a>(&'a Vec<fhaudio_sigproc::EdgePair>);

impl Display for EdgePairsText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_EMPTY);
        if self.0.is_empty() {
            table.add_row(row!["No edge pairs available."]);
        } else {
            for edge_pair in self.0 {
                table.add_row(row![format!(
                    "{} -> {}",
                    edge_pair.processing_element_id_from, edge_pair.processing_element_id_to
                )]);
            }
        }
        table.fmt(f)
    }
}

/// Formatter for a vector of [ElementWithState]s.
struct ElementWithStatesText<'a>(&'a Vec<ElementWithState>);

impl Display for ElementWithStatesText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_EMPTY);
        if self.0.is_empty() {
            table.add_row(row!["No elements available."]);
        } else {
            for element_with_state in self.0 {
                table.add_row(row![ElementWithStateText(element_with_state)]);
            }
        }
        table.fmt(f)
    }
}

/// Formatter for an [ElementWithState].
struct ElementWithStateText<'a>(&'a ElementWithState);

impl Display for ElementWithStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let capabilities = ElementCapabilities {
            can_stop: self.0.element.can_stop,
            can_bypass: self.0.element.can_bypass,
        };

        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED);
        table.add_row(row!("ID:", self.0.element.id));
        table.add_row(row!("Type:", DebugText(self.0.element.type_)));
        table.add_row(row!("Description:", or_nothing(&self.0.element.description)));
        if let Some(type_specific) = &self.0.element.type_specific {
            table.add_row(row!("Type specific:", TypeSpecificElementText(type_specific)));
        }
        table.add_row(row!(
            "State:",
            or_unknown(&self.0.state.as_ref().map(|state| ElementStateText(state, capabilities)))
        ));
        table.fmt(f)
    }
}

/// Formatter for a signal processing [TypeSpecificElement].
struct TypeSpecificElementText<'a>(&'a TypeSpecificElement);

impl Display for TypeSpecificElementText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            TypeSpecificElement::VendorSpecific(vendor_specific) => {
                VendorSpecificText(vendor_specific).fmt(f)
            }
            TypeSpecificElement::Gain(gain) => GainText(gain).fmt(f),
            TypeSpecificElement::Equalizer(equalizer) => EqualizerText(equalizer).fmt(f),
            TypeSpecificElement::Dynamics(dynamics) => DynamicsText(dynamics).fmt(f),
            TypeSpecificElement::DaiInterconnect(dai_interconnect) => {
                DaiInterconnectText(dai_interconnect).fmt(f)
            }
        }
    }
}

/// Formatter for signal processing [VendorSpecific] element type-specific properties.
struct VendorSpecificText<'a>(
    // [VendorSpecific] currently does not have any fields, so it's not read.
    #[allow(dead_code)] &'a VendorSpecific,
);

impl Display for VendorSpecificText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<vendor specific>")
    }
}

/// Formatter for signal processing [Gain] element type-specific properties.
struct GainText<'a>(&'a Gain);

impl Display for GainText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let range = if self.0.range.min == self.0.range.max {
            format!("Fixed {}", GainValueText(self.0.range.min, self.0.type_))
        } else {
            format!(
                "[{}, {}]",
                GainValueText(self.0.range.min, self.0.type_),
                GainValueText(self.0.range.max, self.0.type_)
            )
        };

        let min_step = if self.0.range.min_step == 0.0f32 {
            format!("{} step (continuous)", GainValueText(0.0f32, self.0.type_))
        } else {
            format!("{} step", GainValueText(self.0.range.min_step, self.0.type_))
        };

        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        table.add_row(row!("Type:", DebugText(self.0.type_)));
        table.add_row(row!("Domain:", or_unknown(&self.0.domain.as_ref().map(DebugText))));
        table.add_row(row!("Range:", format!("{}; {}", range, min_step)));
        table.fmt(f)
    }
}

/// Formatter for signal processing [Equalizer] element type-specific properties.
struct EqualizerText<'a>(&'a Equalizer);

impl Display for EqualizerText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        table.add_row(row!("Bands:", EqualizerBandsText(&self.0.bands)));
        table.add_row(row!(
            "Supported controls:",
            or_unknown(&self.0.supported_controls.as_ref().map(EqualizerSupportedControlsText))
        ));
        table.add_row(row!(
            "Can disable bands?:",
            // If missing, it means this Equalizer cannot disable individual bands.
            or_no_missing(&self.0.can_disable_bands.map(YesNoText))
        ));
        table.add_row(row!(
            "Frequency range:",
            format!("[{}, {}]", HertzText(self.0.min_frequency), HertzText(self.0.max_frequency))
        ));
        table.add_row(row!(
            "Maximum quality factor:",
            or_unknown(&self.0.max_q.as_ref().map(ToString::to_string))
        ));
        table.add_row(row!(
            "Gain range:",
            format!(
                "[{}, {}]",
                or_unknown(&self.0.min_gain_db.map(DecibelsText)),
                or_unknown(&self.0.max_gain_db.map(DecibelsText))
            ),
        ));
        table.fmt(f)
    }
}

/// Formatter for a vector of signal processing [EqualizerBand]s.
struct EqualizerBandsText<'a>(&'a Vec<EqualizerBand>);

impl Display for EqualizerBandsText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        if self.0.is_empty() {
            table.add_row(row!["No bands available."]);
        } else {
            for element in self.0 {
                table.add_row(row![EqualizerBandText(element)]);
            }
        }
        table.fmt(f)
    }
}

/// Formatter for a signal processing [EqualizerBand].
struct EqualizerBandText<'a>(&'a EqualizerBand);

impl Display for EqualizerBandText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        table.add_row(row!("ID:", self.0.id));
        table.fmt(f)
    }
}

/// Formatter for signal processing [EqualizerSupportedControls].
struct EqualizerSupportedControlsText<'a>(&'a fhaudio_sigproc::EqualizerSupportedControls);

impl Display for EqualizerSupportedControlsText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        for (label, flag) in [
            ("Frequency:", fhaudio_sigproc::EqualizerSupportedControls::CAN_CONTROL_FREQUENCY),
            ("Quality factor (Q):", fhaudio_sigproc::EqualizerSupportedControls::CAN_CONTROL_Q),
            ("Peak bands:", fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_PEAK),
            ("Notch bands:", fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_NOTCH),
            ("Low cut bands:", fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_LOW_CUT),
            (
                "High cut bands:",
                fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_HIGH_CUT,
            ),
            (
                "Low shelf bands:",
                fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_LOW_SHELF,
            ),
            (
                "High shelf bands:",
                fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_HIGH_SHELF,
            ),
        ] {
            table.add_row(row!(label, YesNoText(self.0.contains(flag))));
        }
        table.fmt(f)
    }
}

/// Formatter for signal processing [Dynamics] element type-specific properties.
struct DynamicsText<'a>(&'a Dynamics);

impl Display for DynamicsText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        table.add_row(row!("Bands:", DynamicsBandsText(&self.0.bands)));
        table.add_row(row!(
            "Supported controls:",
            or_unknown(&self.0.supported_controls.as_ref().map(DynamicsSupportedControlsText))
        ));
        table.fmt(f)
    }
}

/// Formatter for a vector of signal processing [DynamicsBand]s.
struct DynamicsBandsText<'a>(&'a Vec<DynamicsBand>);

impl Display for DynamicsBandsText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        if self.0.is_empty() {
            table.add_row(row!["No bands available."]);
        } else {
            for element in self.0 {
                table.add_row(row![DynamicsBandText(element)]);
            }
        }
        table.fmt(f)
    }
}

/// Formatter for a signal processing [DynamicsBand].
struct DynamicsBandText<'a>(&'a DynamicsBand);

impl Display for DynamicsBandText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        table.add_row(row!("ID:", self.0.id));
        table.fmt(f)
    }
}

/// Formatter for signal processing [DynamicsSupportedControls].
struct DynamicsSupportedControlsText<'a>(&'a fhaudio_sigproc::DynamicsSupportedControls);

impl Display for DynamicsSupportedControlsText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        for (label, flag) in [
            ("Knee width:", fhaudio_sigproc::DynamicsSupportedControls::KNEE_WIDTH),
            ("Attack:", fhaudio_sigproc::DynamicsSupportedControls::ATTACK),
            ("Release:", fhaudio_sigproc::DynamicsSupportedControls::RELEASE),
            ("Output gain:", fhaudio_sigproc::DynamicsSupportedControls::OUTPUT_GAIN),
            ("Input gain:", fhaudio_sigproc::DynamicsSupportedControls::INPUT_GAIN),
            ("Lookahead:", fhaudio_sigproc::DynamicsSupportedControls::LOOKAHEAD),
            ("Level type:", fhaudio_sigproc::DynamicsSupportedControls::LEVEL_TYPE),
            ("Linked channels:", fhaudio_sigproc::DynamicsSupportedControls::LINKED_CHANNELS),
            ("Threshold type:", fhaudio_sigproc::DynamicsSupportedControls::THRESHOLD_TYPE),
        ] {
            table.add_row(row!(label, YesNoText(self.0.contains(flag))));
        }
        table.fmt(f)
    }
}

/// Formatter for signal processing [PlugDetectCapabilities].
struct PlugDetectCapabilitiesText<'a>(&'a fhaudio_sigproc::PlugDetectCapabilities);

impl Display for PlugDetectCapabilitiesText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self.0 {
            fhaudio_sigproc::PlugDetectCapabilities::Hardwired => "Hardwired",
            fhaudio_sigproc::PlugDetectCapabilities::CanAsyncNotify => {
                "Pluggable (can async notify)"
            }
            _ => "<unknown>",
        };
        f.write_str(s)
    }
}

/// Formatter for signal processing [DaiInterconnect] element type-specific properties.
struct DaiInterconnectText<'a>(&'a DaiInterconnect);

impl Display for DaiInterconnectText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        table.add_row(row!(
            "Plug detection:",
            PlugDetectCapabilitiesText(&self.0.plug_detect_capabilities)
        ));
        table.fmt(f)
    }
}

/// Portion of `ElementState` that represents its capabilities.
struct ElementCapabilities {
    can_stop: Option<bool>,
    can_bypass: Option<bool>,
}

/// Formatter for a signal processing [ElementState].
///
/// Takes in an `ElementCapabilities` to annotate current state, like
/// started/bypassed with whether that state can change.
struct ElementStateText<'a>(&'a ElementState, ElementCapabilities);

impl Display for ElementStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let started_text =
            format!("{} ({})", YesNoText(self.0.started), CanCannotText(self.1.can_stop, "Stop"),);
        let bypassed_text = format!(
            "{} ({})",
            // If missing, it means this Element is not bypassed.
            or_no_missing(&self.0.bypassed.map(YesNoText)),
            CanCannotText(self.1.can_bypass, "Bypass"),
        );

        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        table.add_row(row!(
            "Vendor specific:",
            or_nothing(
                &self.0.vendor_specific_data.as_ref().map(|data| BytesText(data.as_slice()))
            )
        ));
        table.add_row(row!("Started:", started_text));
        table.add_row(row!("Bypassed:", bypassed_text));
        table.add_row(row!(
            "Turn on delay:",
            or_nothing(&self.0.turn_on_delay_ns.map(DurationText))
        ));
        table.add_row(row!(
            "Turn off delay:",
            or_nothing(&self.0.turn_off_delay_ns.map(DurationText))
        ));
        table.add_row(row!(
            "Processing delay:",
            or_nothing(&self.0.processing_delay_ns.map(DurationText))
        ));
        if let Some(type_specific) = &self.0.type_specific {
            table.add_row(row!("Type specific:", TypeSpecificElementStateText(type_specific)));
        }
        table.fmt(f)
    }
}

/// Formatter for a signal processing [TypeSpecificElementState].
struct TypeSpecificElementStateText<'a>(&'a TypeSpecificElementState);

impl Display for TypeSpecificElementStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            TypeSpecificElementState::VendorSpecific(vendor_specific_element_state) => {
                VendorSpecificElementStateText(vendor_specific_element_state).fmt(f)
            }
            TypeSpecificElementState::Gain(gain_element_state) => {
                GainElementStateText(gain_element_state).fmt(f)
            }
            TypeSpecificElementState::Equalizer(equalizer_element_state) => {
                EqualizerElementStateText(equalizer_element_state).fmt(f)
            }
            TypeSpecificElementState::Dynamics(dynamics_element_state) => {
                DynamicsElementStateText(dynamics_element_state).fmt(f)
            }
            TypeSpecificElementState::DaiInterconnect(dai_interconnect_element_state) => {
                DaiInterconnectElementStateText(dai_interconnect_element_state).fmt(f)
            }
        }
    }
}

/// Formatter for signal processing [VendorSpecificElementState].
struct VendorSpecificElementStateText<'a>(
    // [VendorSpecificState] currently does not have any fields, so it's not read.
    #[allow(dead_code)] &'a VendorSpecificElementState,
);

impl Display for VendorSpecificElementStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<vendor specific element state>")
    }
}

/// Formatter for signal processing [GainElementState].
struct GainElementStateText<'a>(&'a GainElementState);

impl Display for GainElementStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        table.add_row(row!("Gain:", self.0.gain));
        table.fmt(f)
    }
}

/// Formatter for signal processing [EqualizerElementState].
struct EqualizerElementStateText<'a>(&'a EqualizerElementState);

impl Display for EqualizerElementStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // EQ element state only contains band states.
        EqualizerBandStatesText(&self.0.band_states).fmt(f)
    }
}

/// Formatter for a vector of signal processing [EqualizerBandState]s.
struct EqualizerBandStatesText<'a>(&'a Vec<EqualizerBandState>);

impl Display for EqualizerBandStatesText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        if self.0.is_empty() {
            table.add_row(row!["No equalizer band states available."]);
        } else {
            for element in self.0 {
                table.add_row(row![EqualizerBandStateText(element)]);
            }
        }
        table.fmt(f)
    }
}

/// Formatter for a signal processing [EqualizerBandState].
struct EqualizerBandStateText<'a>(&'a EqualizerBandState);

impl Display for EqualizerBandStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED);
        table.add_row(row!("ID:", self.0.id));
        table.add_row(row!("Type:", or_unknown(&self.0.type_.as_ref().map(DebugText))));
        table.add_row(row!("Frequency:", or_unknown(&self.0.frequency.map(HertzText))));
        table.add_row(row!("Quality factor:", or_unknown(&self.0.q)));
        table.add_row(row!("Gain:", or_unknown(&self.0.frequency.map(DecibelsText))));
        // If missing, it means this Equalizer band is enabled.
        table.add_row(row!("Enabled:", or_yes_missing(&self.0.enabled.map(YesNoText))));
        table.fmt(f)
    }
}

/// Formatter for signal processing [DynamicsElementState].
struct DynamicsElementStateText<'a>(&'a DynamicsElementState);

impl Display for DynamicsElementStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Dynamics element state only contains band states.
        DynamicsBandStatesText(&self.0.band_states).fmt(f)
    }
}

/// Formatter for a vector of signal processing [DynamicsBandState]s.
struct DynamicsBandStatesText<'a>(&'a Vec<DynamicsBandState>);

impl Display for DynamicsBandStatesText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        if self.0.is_empty() {
            table.add_row(row!["No dynamics band states available."]);
        } else {
            for element in self.0 {
                table.add_row(row![DynamicsBandStateText(element)]);
            }
        }
        table.fmt(f)
    }
}

/// Formatter for a signal processing [DynamicsBandState].
struct DynamicsBandStateText<'a>(&'a DynamicsBandState);

impl Display for DynamicsBandStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED);
        table.add_row(row!("ID:", self.0.id));
        table.add_row(row!(
            "Frequency range:",
            format!("[{}, {}]", HertzText(self.0.min_frequency), HertzText(self.0.max_frequency))
        ));
        table.add_row(row!(
            "Threshold:",
            format!("{} {}", DebugText(self.0.threshold_type), DecibelsText(self.0.threshold_db))
        ));
        table.add_row(row!("Knee width:", or_unknown(&self.0.knee_width_db.map(DecibelsText))));
        table.add_row(row!("Attack:", or_unknown(&self.0.attack.map(DurationText))));
        table.add_row(row!("Release:", or_unknown(&self.0.release.map(DurationText))));
        table.add_row(row!("Output gain:", or_unknown(&self.0.output_gain_db.map(DecibelsText))));
        table.add_row(row!("Input gain:", or_unknown(&self.0.input_gain_db.map(DecibelsText))));
        table.add_row(row!("Level type:", or_unknown(&self.0.level_type.map(DebugText))));
        table.add_row(row!("Lookahead:", or_unknown(&self.0.lookahead.map(DurationText))));
        table.add_row(row!("Linked channels:", or_unknown(&self.0.linked_channels.map(YesNoText))));
        table.fmt(f)
    }
}

/// Formatter for signal processing [PlugState].
struct PlugStateText<'a>(&'a fuchsia_audio_fdomain::sigproc::PlugState);

impl Display for PlugStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = if self.0.plugged { "Plugged" } else { "Unplugged" };
        write!(f, "{} at {}", s, self.0.plug_state_time)
    }
}

/// Formatter for signal processing [DaiInterconnectElementState].
struct DaiInterconnectElementStateText<'a>(&'a DaiInterconnectElementState);

impl Display for DaiInterconnectElementStateText<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NESTED_COMPACT);
        table.add_row(row!("Plug state:", PlugStateText(&self.0.plug_state)));
        table.add_row(row!(
            "External delay:",
            or_nothing(&self.0.external_delay_ns.map(DurationText))
        ));
        table.fmt(f)
    }
}

/// A combination of a signal processing [Element] and its [ElementState].
#[derive(Debug, Serialize, PartialEq, Clone)]
pub struct ElementWithState {
    #[serde(flatten, with = "serde_ext::ElementDef")]
    element: Element,
    #[serde(serialize_with = "serde_ext::serialize_option_elementstate")]
    state: Option<ElementState>,
}

pub fn serialize_option_vec_elementwithstate<S>(
    element_with_states: &Option<Vec<ElementWithState>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let Some(element_with_states) = element_with_states else { return serializer.serialize_none() };

    let mut seq = serializer.serialize_seq(Some(element_with_states.len()))?;
    for element_with_state in element_with_states {
        seq.serialize_element(&element_with_state)?;
    }
    seq.end()
}

#[derive(Default, Debug, Serialize, PartialEq, Clone)]
pub struct InfoResult {
    pub device_name: Option<String>,

    pub token_id: Option<u64>,

    pub device_path: Option<Utf8PathBuf>,

    #[serde(serialize_with = "serde_ext::serialize_option_tostring")]
    pub unique_id: Option<UniqueInstanceId>,

    pub manufacturer: Option<String>,

    pub product_name: Option<String>,

    #[serde(serialize_with = "serde_ext::serialize_option_gainstate")]
    pub gain_state: Option<GainState>,

    #[serde(serialize_with = "serde_ext::serialize_option_gaincapabilities")]
    pub gain_capabilities: Option<GainCapabilities>,

    #[serde(serialize_with = "serde_ext::serialize_option_plugevent")]
    pub plug_event: Option<PlugEvent>,

    #[serde(serialize_with = "serde_ext::serialize_option_tostring")]
    pub plug_detect_capabilities: Option<PlugDetectCapabilities>,

    #[serde(serialize_with = "serde_ext::serialize_option_clockdomain")]
    pub clock_domain: Option<ClockDomain>,

    #[serde(serialize_with = "serde_ext::serialize_option_map_daiformatset")]
    pub supported_dai_formats: Option<BTreeMap<fadevice::ElementId, Vec<DaiFormatSet>>>,

    #[serde(serialize_with = "serde_ext::serialize_option_map_pcmformatset")]
    pub supported_ring_buffer_formats: Option<BTreeMap<fadevice::ElementId, Vec<PcmFormatSet>>>,

    #[serde(serialize_with = "serde_ext::serialize_option_vec_topology")]
    pub topologies: Option<Vec<Topology>>,

    pub topology_id: Option<fadevice::TopologyId>,

    #[serde(rename = "elements", serialize_with = "serialize_option_vec_elementwithstate")]
    pub element_with_states: Option<Vec<ElementWithState>>,
}

impl Display for InfoResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut table = Table::new();
        table.set_format(*TABLE_FORMAT_NORMAL);
        table.add_row(row!("Name:", or_unknown(&self.device_name)));
        table.add_row(row!("Token ID:", or_unknown(&self.token_id)));
        table.add_row(row!("Path:", or_unknown(&self.device_path)));
        table.add_row(row!("Unique ID:", or_unknown(&self.unique_id)));
        table.add_row(row!("Manufacturer:", or_unknown(&self.manufacturer)));
        table.add_row(row!("Product:", or_unknown(&self.product_name)));
        table.add_row(row!(
            "Current gain:",
            or_unknown(&self.gain_state.as_ref().map(GainStateText)),
        ));
        table.add_row(row!(
            "Gain capabilities:",
            or_unknown(&self.gain_capabilities.as_ref().map(GainCapabilitiesText)),
        ));
        table
            .add_row(
                row!("Plug state:", or_unknown(&self.plug_event.as_ref().map(PlugEventText)),),
            );
        table.add_row(row!("Plug detection:", or_unknown(&self.plug_detect_capabilities)));
        table.add_row(row!("Clock domain:", or_unknown(&self.clock_domain)));
        table.add_row(row!("Signal processing topology:", or_unknown(&self.topology_id)));
        table.fmt(f)?;

        writeln!(f)?;

        let mut dai_formats_table = Table::new();
        dai_formats_table.set_format(*TABLE_FORMAT_NORMAL);
        dai_formats_table.set_titles(row!("DAI formats:"));
        dai_formats_table.add_row(row!(or_unknown(
            &self.supported_dai_formats.as_ref().map(SupportedDaiFormatsText)
        )));
        dai_formats_table.fmt(f)?;

        writeln!(f)?;

        let mut rb_formats_table = Table::new();
        rb_formats_table.set_format(*TABLE_FORMAT_NORMAL);
        rb_formats_table.set_titles(row!("Ring buffer formats:"));
        rb_formats_table.add_row(row!(or_unknown(
            &self.supported_ring_buffer_formats.as_ref().map(SupportedRingBufferFormatsText),
        ),));
        rb_formats_table.fmt(f)?;

        writeln!(f)?;

        let mut topologies_table = Table::new();
        topologies_table.set_format(*TABLE_FORMAT_NORMAL);
        topologies_table.set_titles(row!("Signal processing topologies:"));
        topologies_table.add_row(row!(or_unknown(&self.topologies.as_ref().map(TopologiesText)),));
        topologies_table.fmt(f)?;

        writeln!(f)?;

        let mut elements_table = Table::new();
        elements_table.set_format(*TABLE_FORMAT_NORMAL);
        elements_table.set_titles(row!("Signal processing elements:"));
        elements_table.add_row(row!(or_unknown(
            &self.element_with_states.as_ref().map(ElementWithStatesText)
        ),));
        elements_table.fmt(f)?;

        Ok(())
    }
}

/// Returns the [Display] representation of the option value, if it exists, or a placeholder.
fn or_unknown(value: &Option<impl ToString>) -> String {
    value.as_ref().map_or_else(|| "<unknown>".to_string(), |value| value.to_string())
}
fn or_yes_missing(value: &Option<impl ToString>) -> String {
    value.as_ref().map_or_else(|| "✅ Yes <missing>".to_string(), |value| value.to_string())
}
fn or_no_missing(value: &Option<impl ToString>) -> String {
    value.as_ref().map_or_else(|| "❌ No <missing>".to_string(), |value| value.to_string())
}
fn or_full_range_none(value: &Option<impl ToString>) -> String {
    value.as_ref().map_or_else(|| "Full range <none>".to_string(), |value| value.to_string())
}
fn or_nothing(value: &Option<impl ToString>) -> String {
    value.as_ref().map_or_else(|| "".to_string(), |value| value.to_string())
}
impl From<(Info, Selector)> for InfoResult {
    fn from(value: (Info, Selector)) -> Self {
        let (info, selector) = value;

        let device_path = match selector {
            Selector::Devfs(devfs) => Some(devfs.path()),
            Selector::Registry(_) => None,
        };
        let device_name = device_path.clone().map_or_else(
            // If not Devfs device (i.e. if Registry device): pull Option<String> from Info record
            || info.device_name(),
            // Otherwise: pull file_name from path (and convert Option<&str> to Option<String>)...
            // ... or (if no path/file_name) return None
            |path| path.file_name().map(str::to_string),
        );

        Self {
            device_name,
            token_id: info.token_id(),
            device_path,
            unique_id: info.unique_instance_id(),
            manufacturer: info.manufacturer(),
            product_name: info.product_name(),
            gain_state: info.gain_state(),
            gain_capabilities: info.gain_capabilities(),
            plug_event: info.plug_event(),
            plug_detect_capabilities: info.plug_detect_capabilities(),
            clock_domain: info.clock_domain(),
            supported_ring_buffer_formats: info.supported_ring_buffer_formats(),
            supported_dai_formats: info.supported_dai_formats(),
            topologies: info.topologies(),
            topology_id: info.topology_id(),
            element_with_states: info.element_with_states(),
        }
    }
}

pub struct HardwareCodecInfo {
    properties: fhaudio::CodecProperties,
    dai_formats: Vec<fhaudio::DaiSupportedFormats>,
    plug_state: fhaudio::PlugState,
}

pub struct HardwareCompositeInfo {
    properties: fhaudio::CompositeProperties,
    dai_formats: BTreeMap<fhaudio_sigproc::ElementId, Vec<fhaudio::DaiSupportedFormats>>,
    ring_buffer_formats: BTreeMap<fhaudio_sigproc::ElementId, Vec<fhaudio::SupportedFormats>>,
}

pub struct HardwareDaiInfo {
    properties: fhaudio::DaiProperties,
    dai_formats: Vec<fhaudio::DaiSupportedFormats>,
    ring_buffer_formats: Vec<fhaudio::SupportedFormats>,
}

pub struct HardwareStreamConfigInfo {
    properties: fhaudio::StreamProperties,
    supported_formats: Vec<fhaudio::SupportedFormats>,
    gain_state: fhaudio::GainState,
    plug_state: fhaudio::PlugState,
}

/// Information about a device from its hardware protocol.
pub enum HardwareInfo {
    Codec(HardwareCodecInfo),
    Composite(HardwareCompositeInfo),
    Dai(HardwareDaiInfo),
    StreamConfig(HardwareStreamConfigInfo),
}

impl HardwareInfo {
    pub fn unique_instance_id(&self) -> Option<UniqueInstanceId> {
        let id = match self {
            HardwareInfo::Codec(codec) => codec.properties.unique_id,
            HardwareInfo::Composite(composite) => composite.properties.unique_id,
            HardwareInfo::Dai(dai) => dai.properties.unique_id,
            HardwareInfo::StreamConfig(stream_config) => stream_config.properties.unique_id,
        };
        id.map(UniqueInstanceId)
    }

    pub fn manufacturer(&self) -> Option<String> {
        match self {
            HardwareInfo::Codec(codec) => codec.properties.manufacturer.clone(),
            HardwareInfo::Composite(composite) => composite.properties.manufacturer.clone(),
            HardwareInfo::Dai(dai) => dai.properties.manufacturer.clone(),
            HardwareInfo::StreamConfig(stream_config) => {
                stream_config.properties.manufacturer.clone()
            }
        }
    }

    pub fn product_name(&self) -> Option<String> {
        match self {
            HardwareInfo::Codec(codec) => codec.properties.product.clone(),
            HardwareInfo::Composite(composite) => composite.properties.product.clone(),
            HardwareInfo::Dai(dai) => dai.properties.product_name.clone(),
            HardwareInfo::StreamConfig(stream_config) => stream_config.properties.product.clone(),
        }
    }

    pub fn gain_capabilities(&self) -> Option<GainCapabilities> {
        match self {
            HardwareInfo::StreamConfig(stream_config) => {
                GainCapabilities::try_from(&stream_config.properties).ok()
            }
            // TODO(https://fxbug.dev/333120537): Support gain caps for hardware Composite/Codec/Dai
            _ => None,
        }
    }

    pub fn plug_detect_capabilities(&self) -> Option<PlugDetectCapabilities> {
        match self {
            HardwareInfo::Codec(codec) => {
                codec.properties.plug_detect_capabilities.map(PlugDetectCapabilities::from)
            }
            HardwareInfo::StreamConfig(stream_config) => {
                stream_config.properties.plug_detect_capabilities.map(PlugDetectCapabilities::from)
            }
            // TODO(https://fxbug.dev/333120537): Support plug detect caps for hardware Composite/Dai
            _ => None,
        }
    }

    pub fn clock_domain(&self) -> Option<ClockDomain> {
        match self {
            HardwareInfo::Composite(composite) => {
                composite.properties.clock_domain.map(ClockDomain::from)
            }
            HardwareInfo::StreamConfig(stream_config) => {
                stream_config.properties.clock_domain.map(ClockDomain::from)
            }
            _ => None,
        }
    }

    pub fn supported_ring_buffer_formats(
        &self,
    ) -> Option<BTreeMap<fadevice::ElementId, Vec<PcmFormatSet>>> {
        fn supported_formats_to_pcm_format_sets(
            supported_formats: &[fhaudio::SupportedFormats],
        ) -> Vec<PcmFormatSet> {
            supported_formats
                .iter()
                .filter_map(|supported_formats| {
                    let pcm_supported_formats = supported_formats.pcm_supported_formats.clone()?;
                    PcmFormatSet::try_from(pcm_supported_formats).ok()
                })
                .collect()
        }

        match self {
            HardwareInfo::Codec(_) => None,
            HardwareInfo::Composite(composite) => Some(
                composite
                    .ring_buffer_formats
                    .iter()
                    .map(|(element_id, supported_formats)| {
                        (*element_id, supported_formats_to_pcm_format_sets(supported_formats))
                    })
                    .collect(),
            ),
            HardwareInfo::Dai(dai) => Some({
                let pcm_format_sets =
                    supported_formats_to_pcm_format_sets(&dai.ring_buffer_formats);
                let mut map = BTreeMap::new();
                map.insert(DEFAULT_RING_BUFFER_ELEMENT_ID, pcm_format_sets);
                map
            }),
            HardwareInfo::StreamConfig(stream_config) => Some({
                let pcm_format_sets =
                    supported_formats_to_pcm_format_sets(&stream_config.supported_formats);
                let mut map = BTreeMap::new();
                map.insert(DEFAULT_RING_BUFFER_ELEMENT_ID, pcm_format_sets);
                map
            }),
        }
    }

    pub fn supported_dai_formats(
        &self,
    ) -> Option<BTreeMap<fadevice::ElementId, Vec<DaiFormatSet>>> {
        fn dai_supported_formats_to_dai_format_sets(
            dai_supported_formats: &[fhaudio::DaiSupportedFormats],
        ) -> Vec<DaiFormatSet> {
            dai_supported_formats
                .iter()
                .map(|dai_supported_formats| DaiFormatSet::from(dai_supported_formats.clone()))
                .collect()
        }

        match self {
            HardwareInfo::Codec(codec) => Some({
                let dai_format_sets = dai_supported_formats_to_dai_format_sets(&codec.dai_formats);
                let mut map = BTreeMap::new();
                map.insert(DEFAULT_DAI_INTERCONNECT_ELEMENT_ID, dai_format_sets);
                map
            }),
            HardwareInfo::Composite(composite) => Some(
                composite
                    .dai_formats
                    .iter()
                    .map(|(element_id, dai_supported_formats)| {
                        (
                            *element_id,
                            dai_supported_formats_to_dai_format_sets(dai_supported_formats),
                        )
                    })
                    .collect(),
            ),
            HardwareInfo::Dai(dai) => Some({
                let dai_format_sets = dai_supported_formats_to_dai_format_sets(&dai.dai_formats);
                let mut map = BTreeMap::new();
                map.insert(DEFAULT_DAI_INTERCONNECT_ELEMENT_ID, dai_format_sets);
                map
            }),
            HardwareInfo::StreamConfig(_) => None,
        }
    }

    pub fn gain_state(&self) -> Option<GainState> {
        match self {
            HardwareInfo::Codec(_) | HardwareInfo::Dai(_) => None,
            // TODO(https://fxbug.dev/334981374): Support gain state for hardware Composite
            HardwareInfo::Composite(_) => None,
            HardwareInfo::StreamConfig(stream_config) => {
                stream_config.gain_state.clone().try_into().ok()
            }
        }
    }

    pub fn plug_event(&self) -> Option<PlugEvent> {
        match self {
            HardwareInfo::Codec(codec) => codec.plug_state.clone().try_into().ok(),
            // TODO(https://fxbug.dev/334980316): Support plug state for hardware Composite
            HardwareInfo::Composite(_) => None,
            HardwareInfo::Dai(_) => None,
            HardwareInfo::StreamConfig(stream_config) => {
                stream_config.plug_state.clone().try_into().ok()
            }
        }
    }

    fn topologies(&self) -> Option<Vec<Topology>> {
        // TODO(https://fxbug.dev/334980316): Support signal processing for hardware protocols
        None
    }

    fn topology_id(&self) -> Option<fadevice::TopologyId> {
        // TODO(https://fxbug.dev/334980316): Support signal processing for hardware protocols
        None
    }

    fn element_with_states(&self) -> Option<Vec<ElementWithState>> {
        // TODO(https://fxbug.dev/334980316): Support signal processing for hardware protocols
        None
    }
}

/// Information about signal processing for a device from `fuchsia.audio.device.Registry`.
pub struct RegistrySignalProcessingInfo {
    topologies: Option<Vec<Topology>>,
    topology_id: Option<fadevice::TopologyId>,
    elements: Option<Vec<Element>>,
    element_states: Option<BTreeMap<fadevice::ElementId, ElementState>>,
}

impl RegistrySignalProcessingInfo {
    /// Returns a vector of [ElementWithState]s that join `elements` with their
    /// corresponding `element_states`.
    fn element_with_states(&self) -> Option<Vec<ElementWithState>> {
        self.elements.clone().map(|elements| {
            elements
                .into_iter()
                .map(|element| {
                    let state = self
                        .element_states
                        .as_ref()
                        .and_then(|states| states.get(&element.id).cloned());
                    ElementWithState { element, state }
                })
                .collect()
        })
    }
}

/// Information about a device from `fuchsia.audio.device.Registry`.
pub struct RegistryInfo {
    device_info: DeviceInfo,
    plug_event: Option<PlugEvent>,
    signal_processing: Option<RegistrySignalProcessingInfo>,
}

pub enum Info {
    Hardware(HardwareInfo),
    Registry(RegistryInfo),
}

impl Info {
    pub fn device_name(&self) -> Option<String> {
        match self {
            Info::Hardware(_) => None,
            Info::Registry(registry_info) => {
                Some(registry_info.device_info.device_name().to_string())
            }
        }
    }

    pub fn token_id(&self) -> Option<fadevice::TokenId> {
        match self {
            Info::Hardware(_) => None,
            Info::Registry(registry_info) => Some(registry_info.device_info.token_id()),
        }
    }

    pub fn unique_instance_id(&self) -> Option<UniqueInstanceId> {
        match self {
            Info::Hardware(hw_info) => hw_info.unique_instance_id(),
            Info::Registry(registry_info) => registry_info.device_info.unique_instance_id(),
        }
    }

    pub fn manufacturer(&self) -> Option<String> {
        match self {
            Info::Hardware(hw_info) => hw_info.manufacturer(),
            Info::Registry(registry_info) => registry_info.device_info.0.manufacturer.clone(),
        }
    }

    pub fn product_name(&self) -> Option<String> {
        match self {
            Info::Hardware(hw_info) => hw_info.product_name(),
            Info::Registry(registry_info) => registry_info.device_info.0.product.clone(),
        }
    }

    pub fn gain_state(&self) -> Option<GainState> {
        match self {
            Info::Hardware(hw_info) => hw_info.gain_state(),
            Info::Registry(_) => None,
        }
    }

    pub fn gain_capabilities(&self) -> Option<GainCapabilities> {
        match self {
            Info::Hardware(hw_info) => hw_info.gain_capabilities(),
            Info::Registry(_) => None,
        }
    }

    pub fn plug_event(&self) -> Option<PlugEvent> {
        match self {
            Info::Hardware(hw_info) => hw_info.plug_event(),
            Info::Registry(registry_info) => registry_info.plug_event,
        }
    }

    pub fn plug_detect_capabilities(&self) -> Option<PlugDetectCapabilities> {
        match self {
            Info::Hardware(hw_info) => hw_info.plug_detect_capabilities(),
            Info::Registry(registry_info) => registry_info.device_info.plug_detect_capabilities(),
        }
    }

    pub fn clock_domain(&self) -> Option<ClockDomain> {
        match self {
            Info::Hardware(hw_info) => hw_info.clock_domain(),
            Info::Registry(registry_info) => registry_info.device_info.clock_domain(),
        }
    }

    pub fn supported_ring_buffer_formats(
        &self,
    ) -> Option<BTreeMap<fadevice::ElementId, Vec<PcmFormatSet>>> {
        match self {
            Info::Hardware(hw_info) => hw_info.supported_ring_buffer_formats(),
            Info::Registry(registry_info) => {
                registry_info.device_info.supported_ring_buffer_formats().ok()
            }
        }
    }

    pub fn supported_dai_formats(
        &self,
    ) -> Option<BTreeMap<fadevice::ElementId, Vec<DaiFormatSet>>> {
        match self {
            Info::Hardware(hw_info) => hw_info.supported_dai_formats(),
            Info::Registry(registry_info) => registry_info.device_info.supported_dai_formats().ok(),
        }
    }

    pub fn topologies(&self) -> Option<Vec<Topology>> {
        match self {
            Info::Hardware(hw_info) => hw_info.topologies(),
            Info::Registry(registry_info) => registry_info
                .signal_processing
                .as_ref()
                .and_then(|sigproc| sigproc.topologies.clone()),
        }
    }

    pub fn topology_id(&self) -> Option<fadevice::TopologyId> {
        match self {
            Info::Hardware(hw_info) => hw_info.topology_id(),
            Info::Registry(registry_info) => {
                registry_info.signal_processing.as_ref().and_then(|sigproc| sigproc.topology_id)
            }
        }
    }

    pub fn element_with_states(&self) -> Option<Vec<ElementWithState>> {
        match self {
            Info::Hardware(hw_info) => hw_info.element_with_states(),
            Info::Registry(registry_info) => registry_info
                .signal_processing
                .as_ref()
                .and_then(|sigproc| sigproc.element_with_states()),
        }
    }
}

/// Returns information about a Codec device from its hardware protocol.
async fn get_hw_codec_info(codec: &fhaudio::CodecProxy) -> Result<HardwareCodecInfo> {
    let properties =
        codec.get_properties().await.bug_context("Failed to call Codec.GetProperties")?;

    let dai_formats = codec
        .get_dai_formats()
        .await
        .bug_context("Failed to call Codec.GetDaiFormats")?
        .map_err(Status::err_from_raw)
        .bug_context("Failed to get DAI formats")?;

    let plug_state =
        codec.watch_plug_state().await.bug_context("Failed to call Codec.WatchPlugState")?;

    Ok(HardwareCodecInfo { properties, dai_formats, plug_state })
}

/// Returns information about a Composite device from its hardware protocol.
async fn get_hw_composite_info(
    composite: &fhaudio::CompositeProxy,
) -> Result<HardwareCompositeInfo> {
    let properties =
        composite.get_properties().await.bug_context("Failed to call Composite.GetProperties")?;

    // TODO(https://fxbug.dev/333120537): Support fetching DAI formats for hardware Composite
    let dai_formats = BTreeMap::new();

    // TODO(https://fxbug.dev/333120537): Support fetching ring buffer formats for hardware Composite
    let ring_buffer_formats = BTreeMap::new();

    Ok(HardwareCompositeInfo { properties, dai_formats, ring_buffer_formats })
}

/// Returns information about a Dai device from its hardware protocol.
async fn get_hw_dai_info(dai: &fhaudio::DaiProxy) -> Result<HardwareDaiInfo> {
    let properties = dai.get_properties().await.bug_context("Failed to call Dai.GetProperties")?;

    let dai_formats = dai
        .get_dai_formats()
        .await
        .bug_context("Failed to call Dai.GetDaiFormats")?
        .map_err(Status::err_from_raw)
        .bug_context("Failed to get DAI formats")?;

    let ring_buffer_formats = dai
        .get_ring_buffer_formats()
        .await
        .bug_context("Failed to call Dai.GetRingBufferFormats")?
        .map_err(Status::err_from_raw)
        .bug_context("Failed to get ring buffer formats")?;

    Ok(HardwareDaiInfo { properties, dai_formats, ring_buffer_formats })
}

/// Returns information about a StreamConfig device from its hardware protocol.
async fn get_hw_stream_config_info(
    stream_config: &fhaudio::StreamConfigProxy,
) -> Result<HardwareStreamConfigInfo> {
    let properties = stream_config
        .get_properties()
        .await
        .bug_context("Failed to call StreamConfig.GetProperties")?;

    let supported_formats = stream_config
        .get_supported_formats()
        .await
        .bug_context("Failed to call StreamConfig.GetSupportedFormats")?;

    let gain_state = stream_config
        .watch_gain_state()
        .await
        .bug_context("Failed to call StreamConfig.WatchGainState")?;

    let plug_state = stream_config
        .watch_plug_state()
        .await
        .bug_context("Failed to call StreamConfig.WatchPlugState")?;

    Ok(HardwareStreamConfigInfo { properties, supported_formats, gain_state, plug_state })
}

/// Returns information about a device from its hardware protocol in devfs.
async fn get_hardware_info(
    dev_class: &fio::DirectoryProxy,
    selector: DevfsSelector,
) -> Result<HardwareInfo> {
    let protocol_path = selector.relative_path();

    match fac::DeviceType::from(selector.device_type()) {
        fac::DeviceType::Codec => {
            let codec = connect::connect_hw_codec(dev_class, protocol_path.as_str())?;
            let codec_info = get_hw_codec_info(&codec).await?;
            Ok(HardwareInfo::Codec(codec_info))
        }
        fac::DeviceType::Composite => {
            let composite = connect::connect_hw_composite(dev_class, protocol_path.as_str())?;
            let composite_info = get_hw_composite_info(&composite).await?;
            Ok(HardwareInfo::Composite(composite_info))
        }
        fac::DeviceType::Dai => {
            let dai = connect::connect_hw_dai(dev_class, protocol_path.as_str())?;
            let dai_info = get_hw_dai_info(&dai).await?;
            Ok(HardwareInfo::Dai(dai_info))
        }
        fac::DeviceType::Input | fac::DeviceType::Output => {
            let stream_config =
                connect::connect_hw_streamconfig(dev_class, protocol_path.as_str())?;
            let stream_config_info = get_hw_stream_config_info(&stream_config).await?;
            Ok(HardwareInfo::StreamConfig(stream_config_info))
        }
        _ => Err(bug!("Unsupported device type")),
    }
}

/// Returns information about a device in the device registry.
async fn get_registry_info(
    registry: &Registry,
    selector: RegistrySelector,
) -> Result<RegistryInfo> {
    let token_id = selector.token_id();

    let device_info = registry
        .device_info(token_id)
        .await
        .ok_or_else(|| user_error!("No device with given ID exists in registry"))?;

    let registry_device =
        registry.observe(token_id).await.bug_context("Failed to observe registry device")?;

    let signal_processing = if let Some(ref sigproc) = registry_device.signal_processing {
        Some(RegistrySignalProcessingInfo {
            topologies: sigproc.topologies().await?,
            topology_id: sigproc.topology_id().await,
            elements: sigproc.elements().await?,
            element_states: sigproc.element_states().await,
        })
    } else {
        None
    };

    let registry_info = RegistryInfo {
        device_info,
        // TODO(https://fxbug.dev/329150383): Support plug_state/plug_time for ADR devices
        plug_event: None,
        signal_processing,
    };
    Ok(registry_info)
}

/// Returns a device info from the target.
///
/// If the selector is a [RegistrySelector] for a registry device, `registry` must be Some.
pub async fn get_info(
    dev_class: &fio::DirectoryProxy,
    registry: Option<&Registry>,
    selector: Selector,
) -> Result<Info> {
    match selector {
        Selector::Devfs(devfs_selector) => {
            let hw_info = get_hardware_info(dev_class, devfs_selector).await?;
            Ok(Info::Hardware(hw_info))
        }
        Selector::Registry(registry_selector) => {
            let registry = registry.ok_or_else(|| bug!("Registry not available"))?;
            let registry_info = get_registry_info(registry, registry_selector).await?;
            Ok(Info::Registry(registry_info))
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use fdomain_fuchsia_audio_device as fadevice;
    use fdomain_fuchsia_hardware_audio as fhaudio;
    use fuchsia_audio_fdomain::format::SampleType;
    use fuchsia_audio_fdomain::sigproc::GainRange;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    const TOKEN_ID: fadevice::TokenId = 68;
    const SOURCE_DAI_ELEMENT_ID: fhaudio_sigproc::ElementId = 0;
    const DEST_DAI_ELEMENT_ID: fhaudio_sigproc::ElementId = 1;
    const DEST_RB_ELEMENT_ID: fhaudio_sigproc::ElementId = 2;
    const SOURCE_RB_ELEMENT_ID: fhaudio_sigproc::ElementId = 3;
    const MUTE_ELEMENT_ID: fhaudio_sigproc::ElementId = 4;
    // The elements below are not part of any topology.
    // They're included to test info output for all element types.
    const VENDOR_SPECIFIC_ELEMENT_ID: fhaudio_sigproc::ElementId = 5;
    const CONNECTION_POINT_ELEMENT_ID: fhaudio_sigproc::ElementId = 6;
    const GAIN_ELEMENT_ID: fhaudio_sigproc::ElementId = 7;
    const AGC_ELEMENT_ID: fhaudio_sigproc::ElementId = 8;
    const AGL_ELEMENT_ID: fhaudio_sigproc::ElementId = 9;
    const DYNAMICS_ELEMENT_ID: fhaudio_sigproc::ElementId = 10;
    const DELAY_ELEMENT_ID: fhaudio_sigproc::ElementId = 11;
    const EQUALIZER_ELEMENT_ID: fhaudio_sigproc::ElementId = 12;
    const SRC_ELEMENT_ID: fhaudio_sigproc::ElementId = 13;

    const INPUT_ONLY_TOPOLOGY_ID: fhaudio_sigproc::TopologyId = 0;
    const FULL_DUPLEX_TOPOLOGY_ID: fhaudio_sigproc::TopologyId = 1;
    const OUTPUT_ONLY_TOPOLOGY_ID: fhaudio_sigproc::TopologyId = 2;
    const OUTPUT_WITH_MUTE_TOPOLOGY_ID: fhaudio_sigproc::TopologyId = 3;

    const INPUT_EDGE_PAIR: fhaudio_sigproc::EdgePair = fhaudio_sigproc::EdgePair {
        processing_element_id_from: SOURCE_DAI_ELEMENT_ID,
        processing_element_id_to: DEST_RB_ELEMENT_ID,
    };

    const OUTPUT_EDGE_PAIR: fhaudio_sigproc::EdgePair = fhaudio_sigproc::EdgePair {
        processing_element_id_from: SOURCE_RB_ELEMENT_ID,
        processing_element_id_to: DEST_DAI_ELEMENT_ID,
    };

    const RB_TO_MUTE_EDGE_PAIR: fhaudio_sigproc::EdgePair = fhaudio_sigproc::EdgePair {
        processing_element_id_from: SOURCE_RB_ELEMENT_ID,
        processing_element_id_to: MUTE_ELEMENT_ID,
    };

    const MUTE_TO_DAI_EDGE_PAIR: fhaudio_sigproc::EdgePair = fhaudio_sigproc::EdgePair {
        processing_element_id_from: MUTE_ELEMENT_ID,
        processing_element_id_to: DEST_DAI_ELEMENT_ID,
    };

    const VENDOR_SPECIFIC_DATA: &[u8] = b"Hello\x01world.";

    pub static TEST_INFO_RESULT: LazyLock<InfoResult> = LazyLock::new(|| {
        InfoResult {
        device_name: Some("0c8301e0".to_string()),
        token_id: Some(TOKEN_ID),
        device_path: Some(Utf8PathBuf::from("/dev/class/audio-input/0c8301e0")),
        unique_id: Some(UniqueInstanceId([
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ])),
        manufacturer: Some("Test manufacturer".to_string()),
        product_name: Some("Test product".to_string()),
        gain_state: Some(GainState {
            gain_db: -3.0_f32,
            muted: Some(false),
            agc_enabled: Some(false),
        }),
        gain_capabilities: Some(GainCapabilities {
            min_gain_db: -100.0_f32,
            max_gain_db: 0.0_f32,
            gain_step_db: 0.0_f32,
            can_mute: Some(true),
            can_agc: Some(false),
        }),
        plug_event: Some(PlugEvent {
            state: fadevice::PlugState::Plugged.into(),
            time: 123456789,
        }),
        plug_detect_capabilities: Some(fadevice::PlugDetectCapabilities::Hardwired.into()),
        clock_domain: Some(ClockDomain(fhaudio::CLOCK_DOMAIN_MONOTONIC)),
        supported_dai_formats: Some({
            let mut map = BTreeMap::new();
            map.insert(
                DEFAULT_DAI_INTERCONNECT_ELEMENT_ID,
                vec![fhaudio::DaiSupportedFormats {
                    number_of_channels: vec![1, 2],
                    sample_formats: vec![
                        fhaudio::DaiSampleFormat::PcmSigned,
                        fhaudio::DaiSampleFormat::PcmUnsigned,
                    ],
                    frame_formats: vec![
                        fhaudio::DaiFrameFormat::FrameFormatStandard(
                            fhaudio::DaiFrameFormatStandard::StereoLeft,
                        ),
                        fhaudio::DaiFrameFormat::FrameFormatStandard(
                            fhaudio::DaiFrameFormatStandard::StereoRight,
                        ),
                    ],
                    frame_rates: vec![16000, 22050, 32000, 44100, 48000, 88200, 96000],
                    bits_per_slot: vec![32],
                    bits_per_sample: vec![8, 16],
                }
                .into()],
            );
            map.insert(
                123,
                vec![fhaudio::DaiSupportedFormats {
                    number_of_channels: vec![1],
                    sample_formats: vec![fhaudio::DaiSampleFormat::PcmFloat],
                    frame_formats: vec![fhaudio::DaiFrameFormat::FrameFormatCustom(
                        fhaudio::DaiFrameFormatCustom {
                            left_justified: false,
                            sclk_on_raising: true,
                            frame_sync_sclks_offset: 1,
                            frame_sync_size: 2,
                        },
                    )],
                    frame_rates: vec![16000, 22050, 32000, 44100, 48000, 88200, 96000],
                    bits_per_slot: vec![16, 32],
                    bits_per_sample: vec![16],
                }
                .into()],
            );
            map
        }),
        supported_ring_buffer_formats: Some({
            let mut map = BTreeMap::new();
            map.insert(
                DEFAULT_RING_BUFFER_ELEMENT_ID,
                vec![PcmFormatSet {
                    channel_sets: vec![
                        ChannelSet::try_from(vec![ChannelAttributes::default()]).unwrap(),
                        ChannelSet::try_from(vec![
                            ChannelAttributes::default(),
                            ChannelAttributes::default(),
                        ])
                        .unwrap(),
                    ],
                    sample_types: vec![SampleType::Int16],
                    frame_rates: vec![16000, 22050, 32000, 44100, 48000, 88200, 96000],
                }],
            );
            map.insert(
                123,
                vec![
                    PcmFormatSet {
                        channel_sets: vec![ChannelSet::try_from(vec![
                            ChannelAttributes::default(),
                        ])
                        .unwrap()],
                        sample_types: vec![SampleType::Uint8, SampleType::Int16],
                        frame_rates: vec![16000, 22050, 32000],
                    },
                    PcmFormatSet {
                        channel_sets: vec![
                            ChannelSet::try_from(vec![ChannelAttributes::default()]).unwrap(),
                            ChannelSet::try_from(vec![
                                ChannelAttributes::default(),
                                ChannelAttributes::default(),
                            ])
                            .unwrap(),
                        ],
                        sample_types: vec![SampleType::Float32],
                        frame_rates: vec![44100, 48000, 88200, 96000],
                    },
                ],
            );
            map
        }),
        topologies: Some(vec![
            Topology { id: INPUT_ONLY_TOPOLOGY_ID, edge_pairs: vec![INPUT_EDGE_PAIR] },
            Topology {
                id: FULL_DUPLEX_TOPOLOGY_ID,
                edge_pairs: vec![INPUT_EDGE_PAIR, OUTPUT_EDGE_PAIR,]
            },
            Topology { id: OUTPUT_ONLY_TOPOLOGY_ID, edge_pairs: vec![OUTPUT_EDGE_PAIR,] },
            Topology {
                id: OUTPUT_WITH_MUTE_TOPOLOGY_ID,
                edge_pairs: vec![RB_TO_MUTE_EDGE_PAIR, MUTE_TO_DAI_EDGE_PAIR]
            },
        ]),
        topology_id: Some(FULL_DUPLEX_TOPOLOGY_ID),
        element_with_states: Some(vec![
            ElementWithState {
                element: Element {
                    id: SOURCE_DAI_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::DaiInterconnect,
                    type_specific: Some(TypeSpecificElement::DaiInterconnect(DaiInterconnect {
                        plug_detect_capabilities:
                            fhaudio_sigproc::PlugDetectCapabilities::CanAsyncNotify
                    })),
                    description: Some("Source DAI interconnect element".to_string()),
                    can_stop: Some(true),
                    can_bypass: Some(false)
                },
                state: Some(ElementState {
                    type_specific: Some(TypeSpecificElementState::DaiInterconnect(DaiInterconnectElementState {
                        plug_state: fuchsia_audio_fdomain::sigproc::PlugState {
                            plugged: true,
                            plug_state_time: 123456789,
                        },
                        external_delay_ns: Some(5000),
                    })),
                    vendor_specific_data: None,
                    started: true,
                    bypassed: None,
                    turn_on_delay_ns: Some(0),
                    turn_off_delay_ns: Some(0),
                    processing_delay_ns: Some(0),
                }),
            },
            ElementWithState {
                element: Element {
                    id: DEST_DAI_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::DaiInterconnect,
                    type_specific: Some(TypeSpecificElement::DaiInterconnect(DaiInterconnect {
                        plug_detect_capabilities:
                            fhaudio_sigproc::PlugDetectCapabilities::CanAsyncNotify
                    })),
                    description: Some("Destination DAI interconnect element".to_string()),
                    can_stop: Some(true),
                    can_bypass: Some(false)
                },
                state: None,
            },
            ElementWithState {
                element: Element {
                    id: SOURCE_RB_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::RingBuffer,
                    type_specific: None,
                    description: Some("Source ring buffer element".to_string()),
                    can_stop: Some(false),
                    can_bypass: Some(false)
                },
                state: None,
            },
            ElementWithState {
                element: Element {
                    id: DEST_RB_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::RingBuffer,
                    type_specific: None,
                    description: Some("Destination ring buffer element".to_string()),
                    can_stop: Some(false),
                    can_bypass: None
                },
                state: None,
            },
            ElementWithState {
                element: Element {
                    id: MUTE_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::Mute,
                    type_specific: None,
                    description: Some("Mute element".to_string()),
                    can_stop: Some(false),
                    can_bypass: Some(true)
                },
                state: None,
            },
            // The elements below are not part of any topology.
            // They're included to test info output for all element types.
            ElementWithState {
                element: Element {
                    id: VENDOR_SPECIFIC_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::VendorSpecific,
                    type_specific: Some(TypeSpecificElement::VendorSpecific(VendorSpecific{})),
                    description: Some("Vendor specific element".to_string()),
                    can_stop: Some(false),
                    can_bypass: Some(true)
                },
                state: Some(ElementState {
                    type_specific: Some(TypeSpecificElementState::VendorSpecific(VendorSpecificElementState {})),
                    vendor_specific_data: Some(VENDOR_SPECIFIC_DATA.to_vec()),
                    started: true,
                    bypassed: Some(false),
                    turn_on_delay_ns: Some(12),
                    turn_off_delay_ns: Some(34),
                    processing_delay_ns: Some(56),
                }),
            },
            ElementWithState {
                element: Element {
                    id: CONNECTION_POINT_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::ConnectionPoint,
                    type_specific: None,
                    description: Some("Connection point element".to_string()),
                    can_stop: Some(false),
                    can_bypass: Some(false)
                },
                state: None,
            },
            ElementWithState {
                element: Element {
                    id: GAIN_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::Gain,
                    type_specific: Some(TypeSpecificElement::Gain(Gain {
                        type_: fhaudio_sigproc::GainType::Decibels,
                        domain: Some(fhaudio_sigproc::GainDomain::Digital),
                        range: GainRange { min: -100.0, max: 0.0, min_step: 1.0 },
                    })),
                    description: Some("Gain element".to_string()),
                    can_stop: None,
                    can_bypass: Some(true)
                },
                state: Some(ElementState {
                    type_specific: Some(TypeSpecificElementState::Gain(GainElementState { gain: -3.0 })),
                    vendor_specific_data: None,
                    started: true,
                    bypassed: Some(false),
                    turn_on_delay_ns: Some(0),
                    turn_off_delay_ns: Some(0),
                    processing_delay_ns: Some(0),
                }),
            },
            ElementWithState {
                element: Element {
                    id: AGC_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::AutomaticGainControl,
                    type_specific: None,
                    description: Some("Automatic gain control element".to_string()),
                    can_stop: Some(false),
                    can_bypass: Some(true)
                },
                state: None,
            },
            ElementWithState {
                element: Element {
                    id: AGL_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::AutomaticGainLimiter,
                    type_specific: None,
                    description: Some("Automatic gain limiter element".to_string()),
                    can_stop: Some(false),
                    can_bypass: Some(true)
                },
                state: None,
            },
            ElementWithState {
                element: Element {
                    id: DYNAMICS_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::Dynamics,
                    type_specific: Some(TypeSpecificElement::Dynamics(Dynamics {
                        bands: vec![DynamicsBand { id: 1 }, DynamicsBand { id: 2 }],
                        supported_controls: Some(
                            fhaudio_sigproc::DynamicsSupportedControls::KNEE_WIDTH
                                | fhaudio_sigproc::DynamicsSupportedControls::ATTACK
                                | fhaudio_sigproc::DynamicsSupportedControls::RELEASE
                                | fhaudio_sigproc::DynamicsSupportedControls::OUTPUT_GAIN
                                | fhaudio_sigproc::DynamicsSupportedControls::INPUT_GAIN
                                | fhaudio_sigproc::DynamicsSupportedControls::LOOKAHEAD
                                | fhaudio_sigproc::DynamicsSupportedControls::LEVEL_TYPE
                                | fhaudio_sigproc::DynamicsSupportedControls::LINKED_CHANNELS
                                | fhaudio_sigproc::DynamicsSupportedControls::THRESHOLD_TYPE,
                        ),
                    })),
                    description: Some("Dynamics element".to_string()),
                    can_stop: Some(false),
                    can_bypass: Some(true)
                },
                state: Some(ElementState {
                    type_specific: Some(TypeSpecificElementState::Dynamics(DynamicsElementState {
                        band_states: vec![
                            DynamicsBandState {
                                id: 1,
                                min_frequency: 0,
                                max_frequency: 24_000,
                                threshold_db: -6.0,
                                threshold_type: fhaudio_sigproc::ThresholdType::Above,
                                ratio: 3.0,
                                knee_width_db: Some(1.0),
                                attack: Some(10),
                                release: Some(300),
                                output_gain_db: Some(1.0),
                                input_gain_db: Some(2.0),
                                level_type: Some(fhaudio_sigproc::LevelType::Peak),
                                lookahead: Some(15),
                                linked_channels: Some(true),
                            },
                            DynamicsBandState {
                                id: 2,
                                min_frequency: 100,
                                max_frequency: 20000,
                                threshold_db: 3.0,
                                threshold_type: fhaudio_sigproc::ThresholdType::Above,
                                ratio: 10.0,
                                knee_width_db: Some(6.0),
                                attack: Some(1),
                                release: Some(100),
                                output_gain_db: Some(0.0),
                                input_gain_db: Some(0.0),
                                level_type: Some(fhaudio_sigproc::LevelType::Rms),
                                lookahead: Some(5),
                                linked_channels: Some(false),
                            },
                        ],
                    })),
                    vendor_specific_data: None,
                    started: true,
                    bypassed: Some(false),
                    turn_on_delay_ns: Some(0),
                    turn_off_delay_ns: Some(0),
                    processing_delay_ns: Some(0),
                }),
            },
            ElementWithState {
                element: Element {
                    id: DELAY_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::Delay,
                    type_specific: None,
                    description: Some("Delay element".to_string()),
                    can_stop: Some(false),
                    can_bypass: Some(true)
                },
                state: None,
            },
            ElementWithState {
                element: Element {
                    id: EQUALIZER_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::Equalizer,
                    type_specific: Some(TypeSpecificElement::Equalizer(
                        Equalizer {
                            bands: vec![
                                EqualizerBand { id: 1 },
                                EqualizerBand { id: 2 },
                            ],
                            supported_controls: Some(
                                fhaudio_sigproc::EqualizerSupportedControls::CAN_CONTROL_FREQUENCY
                                    | fhaudio_sigproc::EqualizerSupportedControls::CAN_CONTROL_Q
                                    | fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_PEAK
                                    | fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_NOTCH
                                    | fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_LOW_CUT
                                    | fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_HIGH_CUT
                                    | fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_LOW_SHELF
                                    | fhaudio_sigproc::EqualizerSupportedControls::SUPPORTS_TYPE_HIGH_SHELF
                            ),
                            can_disable_bands: Some(false),
                            min_frequency: 10,
                            max_frequency: 20_000,
                            max_q: Some(2.5),
                            min_gain_db: Some(-24.0),
                            max_gain_db: Some(24.0),
                        }
                    )),
                    description: Some("Equalizer element".to_string()),
                    can_stop: Some(false),
                    can_bypass: Some(true)
                },
                state: Some(ElementState {
                    type_specific: Some(TypeSpecificElementState::Equalizer(EqualizerElementState {
                        band_states: vec![
                            EqualizerBandState {
                                id: 1,
                                type_: Some(fhaudio_sigproc::EqualizerBandType::Peak),
                                frequency: Some(300),
                                q: Some(2.0),
                                gain_db: Some(3.0),
                                enabled: Some(true),
                            },
                            EqualizerBandState {
                                id: 2,
                                type_: Some(fhaudio_sigproc::EqualizerBandType::HighCut),
                                frequency: Some(10_000),
                                q: Some(10.0),
                                gain_db: None,
                                enabled: None,
                            },
                        ],
                    })),
                    vendor_specific_data: None,
                    started: true,
                    bypassed: Some(false),
                    turn_on_delay_ns: Some(0),
                    turn_off_delay_ns: Some(0),
                    processing_delay_ns: Some(0),
                }),
            },
            ElementWithState {
                element: Element {
                    id: SRC_ELEMENT_ID,
                    type_: fhaudio_sigproc::ElementType::SampleRateConversion,
                    type_specific: None,
                    description: Some("Sample rate conversion element".to_string()),
                    can_stop: Some(false),
                    can_bypass: Some(true)
                },
                state: None,
            },
        ]),
    }
    });

    #[test]
    fn test_info_result_table() {
        let output = TEST_INFO_RESULT.to_string();

        let expected = r#"
  Name:                        0c8301e0
  Token ID:                    68
  Path:                        /dev/class/audio-input/0c8301e0
  Unique ID:                   000102030405060708090a0b0c0d0e0f
  Manufacturer:                Test manufacturer
  Product:                     Test product
  Current gain:                -3 dB (unmuted, AGC off)
  Gain capabilities:           Gain range: [-100 dB, 0 dB]
                               Gain step:  0 dB step (continuous)
                               Mute:       ✅ Can Mute
                               AGC:        ❌ Cannot AGC
  Plug state:                  Plugged at 123456789
  Plug detection:              Hardwired
  Clock domain:                0 (monotonic)
  Signal processing topology:  1

  DAI formats:
  • Element 1 has 1 DAI format set:
    ┌───────────────────────────────────────────────────────────────────────────────────────────┐
    │ Number of channels:  1, 2                                                                 │
    │ Sample formats:      pcm_signed, pcm_unsigned                                             │
    │ Frame formats:       stereo_left                                                          │
    │                      stereo_right                                                         │
    │ Frame rates:         16000 Hz, 22050 Hz, 32000 Hz, 44100 Hz, 48000 Hz, 88200 Hz, 96000 Hz │
    │ Bits per slot:       32                                                                   │
    │ Bits per sample:     8, 16                                                                │
    └───────────────────────────────────────────────────────────────────────────────────────────┘
  • Element 123 has 1 DAI format set:
    ┌───────────────────────────────────────────────────────────────────────────────────────────┐
    │ Number of channels:  1                                                                    │
    │ Sample formats:      pcm_float                                                            │
    │ Frame formats:         ┌───────────────────────────────────────────────────────┐          │
    │                        │ custom:right_justified;raising_sclk;1;2               │          │
    │                        │                                                       │          │
    │                        │ Justification:                           Right        │          │
    │                        │ Clocking:                                Raising sclk │          │
    │                        │ Frame sync offset (sclks):               1            │          │
    │                        │ Frame sync size (sclks):                 2            │          │
    │                        └───────────────────────────────────────────────────────┘          │
    │ Frame rates:         16000 Hz, 22050 Hz, 32000 Hz, 44100 Hz, 48000 Hz, 88200 Hz, 96000 Hz │
    │ Bits per slot:       16, 32                                                               │
    │ Bits per sample:     16                                                                   │
    └───────────────────────────────────────────────────────────────────────────────────────────┘

  Ring buffer formats:
  • Element 0 has 1 format set:
    ┌───────────────────────────────────────────────────────────────────────────────────────────┐
    │ Sample types:        int16                                                                │
    │ Frame rates:         16000 Hz, 22050 Hz, 32000 Hz, 44100 Hz, 48000 Hz, 88200 Hz, 96000 Hz │
    │ Number of channels:  1, 2                                                                 │
    │ Channel attributes:  1 channel:  Channel 1: Min frequency: Full range <none>              │
    │                                             Max frequency: Full range <none>              │
    │                      2 channels: Channel 1: Min frequency: Full range <none>              │
    │                                             Max frequency: Full range <none>              │
    │                                  Channel 2: Min frequency: Full range <none>              │
    │                                             Max frequency: Full range <none>              │
    └───────────────────────────────────────────────────────────────────────────────────────────┘
  • Element 123 has 2 format sets:
    ┌─────────────────────────────────────────────────────────────────────────────┐
    │ Sample types:        uint8, int16                                           │
    │ Frame rates:         16000 Hz, 22050 Hz, 32000 Hz                           │
    │ Number of channels:  1                                                      │
    │ Channel attributes:  1 channel: Channel 1: Min frequency: Full range <none> │
    │                                            Max frequency: Full range <none> │
    └─────────────────────────────────────────────────────────────────────────────┘
    ┌──────────────────────────────────────────────────────────────────────────────┐
    │ Sample types:        float32                                                 │
    │ Frame rates:         44100 Hz, 48000 Hz, 88200 Hz, 96000 Hz                  │
    │ Number of channels:  1, 2                                                    │
    │ Channel attributes:  1 channel:  Channel 1: Min frequency: Full range <none> │
    │                                             Max frequency: Full range <none> │
    │                      2 channels: Channel 1: Min frequency: Full range <none> │
    │                                             Max frequency: Full range <none> │
    │                                  Channel 2: Min frequency: Full range <none> │
    │                                             Max frequency: Full range <none> │
    └──────────────────────────────────────────────────────────────────────────────┘

  Signal processing topologies:
    ┌────────────────────────┐
    │ ID:             0      │
    │ Element edges:  0 -> 2 │
    └────────────────────────┘
    ┌────────────────────────┐
    │ ID:             1      │
    │ Element edges:  0 -> 2 │
    │                 3 -> 1 │
    └────────────────────────┘
    ┌────────────────────────┐
    │ ID:             2      │
    │ Element edges:  3 -> 1 │
    └────────────────────────┘
    ┌────────────────────────┐
    │ ID:             3      │
    │ Element edges:  3 -> 4 │
    │                 4 -> 1 │
    └────────────────────────┘

  Signal processing elements:
    ┌────────────────────────────────────────────────────────────────────────┐
    │ ID:             0                                                      │
    │ Type:           DaiInterconnect                                        │
    │ Description:    Source DAI interconnect element                        │
    │ Type specific:  Plug detection: Pluggable (can async notify)           │
    │ State:          Vendor specific:                                       │
    │                 Started:          ✅ Yes (✅ Can Stop)                 │
    │                 Bypassed:         ❌ No <missing> (❌ Cannot Bypass)   │
    │                 Turn on delay:    0 ns                                 │
    │                 Turn off delay:   0 ns                                 │
    │                 Processing delay: 0 ns                                 │
    │                 Type specific:    Plug state:     Plugged at 123456789 │
    │                                   External delay: 5000 ns              │
    └────────────────────────────────────────────────────────────────────────┘
    ┌──────────────────────────────────────────────────────────────┐
    │ ID:             1                                            │
    │ Type:           DaiInterconnect                              │
    │ Description:    Destination DAI interconnect element         │
    │ Type specific:  Plug detection: Pluggable (can async notify) │
    │ State:          <unknown>                                    │
    └──────────────────────────────────────────────────────────────┘
    ┌──────────────────────────────────────────┐
    │ ID:           3                          │
    │ Type:         RingBuffer                 │
    │ Description:  Source ring buffer element │
    │ State:        <unknown>                  │
    └──────────────────────────────────────────┘
    ┌───────────────────────────────────────────────┐
    │ ID:           2                               │
    │ Type:         RingBuffer                      │
    │ Description:  Destination ring buffer element │
    │ State:        <unknown>                       │
    └───────────────────────────────────────────────┘
    ┌────────────────────────────┐
    │ ID:           4            │
    │ Type:         Mute         │
    │ Description:  Mute element │
    │ State:        <unknown>    │
    └────────────────────────────┘
    ┌─────────────────────────────────────────────────────────────────────────────────────┐
    │ ID:             5                                                                   │
    │ Type:           VendorSpecific                                                      │
    │ Description:    Vendor specific element                                             │
    │ Type specific:  <vendor specific>                                                   │
    │ State:          Vendor specific:  0000: 4865 6c6c 6f01 776f 726c 642e  Hello.world. │
    │                 Started:          ✅ Yes (❌ Cannot Stop)                           │
    │                 Bypassed:         ❌ No (✅ Can Bypass)                             │
    │                 Turn on delay:    12 ns                                             │
    │                 Turn off delay:   34 ns                                             │
    │                 Processing delay: 56 ns                                             │
    │                 Type specific:    <vendor specific element state>                   │
    └─────────────────────────────────────────────────────────────────────────────────────┘
    ┌────────────────────────────────────────┐
    │ ID:           6                        │
    │ Type:         ConnectionPoint          │
    │ Description:  Connection point element │
    │ State:        <unknown>                │
    └────────────────────────────────────────┘
    ┌─────────────────────────────────────────────────────────────────────┐
    │ ID:             7                                                   │
    │ Type:           Gain                                                │
    │ Description:    Gain element                                        │
    │ Type specific:  Type:   Decibels                                    │
    │                 Domain: Digital                                     │
    │                 Range:  [-100 dB, 0 dB]; 1 dB step                  │
    │ State:          Vendor specific:                                    │
    │                 Started:          ✅ Yes (❌ Cannot Stop <missing>) │
    │                 Bypassed:         ❌ No (✅ Can Bypass)             │
    │                 Turn on delay:    0 ns                              │
    │                 Turn off delay:   0 ns                              │
    │                 Processing delay: 0 ns                              │
    │                 Type specific:    Gain: -3                          │
    └─────────────────────────────────────────────────────────────────────┘
    ┌──────────────────────────────────────────────┐
    │ ID:           8                              │
    │ Type:         AutomaticGainControl           │
    │ Description:  Automatic gain control element │
    │ State:        <unknown>                      │
    └──────────────────────────────────────────────┘
    ┌──────────────────────────────────────────────┐
    │ ID:           9                              │
    │ Type:         AutomaticGainLimiter           │
    │ Description:  Automatic gain limiter element │
    │ State:        <unknown>                      │
    └──────────────────────────────────────────────┘
    ┌──────────────────────────────────────────────────────────────────────────────┐
    │ ID:             10                                                           │
    │ Type:           Dynamics                                                     │
    │ Description:    Dynamics element                                             │
    │ Type specific:  Bands:              ID: 1                                    │
    │                                     ID: 2                                    │
    │                 Supported controls: Knee width:      ✅ Yes                  │
    │                                     Attack:          ✅ Yes                  │
    │                                     Release:         ✅ Yes                  │
    │                                     Output gain:     ✅ Yes                  │
    │                                     Input gain:      ✅ Yes                  │
    │                                     Lookahead:       ✅ Yes                  │
    │                                     Level type:      ✅ Yes                  │
    │                                     Linked channels: ✅ Yes                  │
    │                                     Threshold type:  ✅ Yes                  │
    │ State:          Vendor specific:                                             │
    │                 Started:          ✅ Yes (❌ Cannot Stop)                    │
    │                 Bypassed:         ❌ No (✅ Can Bypass)                      │
    │                 Turn on delay:    0 ns                                       │
    │                 Turn off delay:   0 ns                                       │
    │                 Processing delay: 0 ns                                       │
    │                 Type specific:      ┌────────────────────────────────────┐   │
    │                                     │ ID:               1                │   │
    │                                     │ Frequency range:  [0 Hz, 24000 Hz] │   │
    │                                     │ Threshold:        Above -6 dB      │   │
    │                                     │ Knee width:       1 dB             │   │
    │                                     │ Attack:           10 ns            │   │
    │                                     │ Release:          300 ns           │   │
    │                                     │ Output gain:      1 dB             │   │
    │                                     │ Input gain:       2 dB             │   │
    │                                     │ Level type:       Peak             │   │
    │                                     │ Lookahead:        15 ns            │   │
    │                                     │ Linked channels:  ✅ Yes           │   │
    │                                     └────────────────────────────────────┘   │
    │                                     ┌──────────────────────────────────────┐ │
    │                                     │ ID:               2                  │ │
    │                                     │ Frequency range:  [100 Hz, 20000 Hz] │ │
    │                                     │ Threshold:        Above 3 dB         │ │
    │                                     │ Knee width:       6 dB               │ │
    │                                     │ Attack:           1 ns               │ │
    │                                     │ Release:          100 ns             │ │
    │                                     │ Output gain:      0 dB               │ │
    │                                     │ Input gain:       0 dB               │ │
    │                                     │ Level type:       Rms                │ │
    │                                     │ Lookahead:        5 ns               │ │
    │                                     │ Linked channels:  ❌ No              │ │
    │                                     └──────────────────────────────────────┘ │
    └──────────────────────────────────────────────────────────────────────────────┘
    ┌─────────────────────────────┐
    │ ID:           11            │
    │ Type:         Delay         │
    │ Description:  Delay element │
    │ State:        <unknown>     │
    └─────────────────────────────┘
    ┌───────────────────────────────────────────────────────────────────────────┐
    │ ID:             12                                                        │
    │ Type:           Equalizer                                                 │
    │ Description:    Equalizer element                                         │
    │ Type specific:  Bands:                  ID: 1                             │
    │                                         ID: 2                             │
    │                 Supported controls:     Frequency:          ✅ Yes        │
    │                                         Quality factor (Q): ✅ Yes        │
    │                                         Peak bands:         ✅ Yes        │
    │                                         Notch bands:        ✅ Yes        │
    │                                         Low cut bands:      ✅ Yes        │
    │                                         High cut bands:     ✅ Yes        │
    │                                         Low shelf bands:    ✅ Yes        │
    │                                         High shelf bands:   ✅ Yes        │
    │                 Can disable bands?:     ❌ No                             │
    │                 Frequency range:        [10 Hz, 20000 Hz]                 │
    │                 Maximum quality factor: 2.5                               │
    │                 Gain range:             [-24 dB, 24 dB]                   │
    │ State:          Vendor specific:                                          │
    │                 Started:          ✅ Yes (❌ Cannot Stop)                 │
    │                 Bypassed:         ❌ No (✅ Can Bypass)                   │
    │                 Turn on delay:    0 ns                                    │
    │                 Turn off delay:   0 ns                                    │
    │                 Processing delay: 0 ns                                    │
    │                 Type specific:      ┌─────────────────────────┐           │
    │                                     │ ID:              1      │           │
    │                                     │ Type:            Peak   │           │
    │                                     │ Frequency:       300 Hz │           │
    │                                     │ Quality factor:  2      │           │
    │                                     │ Gain:            300 dB │           │
    │                                     │ Enabled:         ✅ Yes │           │
    │                                     └─────────────────────────┘           │
    │                                     ┌───────────────────────────────────┐ │
    │                                     │ ID:              2                │ │
    │                                     │ Type:            HighCut          │ │
    │                                     │ Frequency:       10000 Hz         │ │
    │                                     │ Quality factor:  10               │ │
    │                                     │ Gain:            10000 dB         │ │
    │                                     │ Enabled:         ✅ Yes <missing> │ │
    │                                     └───────────────────────────────────┘ │
    └───────────────────────────────────────────────────────────────────────────┘
    ┌──────────────────────────────────────────────┐
    │ ID:           13                             │
    │ Type:         SampleRateConversion           │
    │ Description:  Sample rate conversion element │
    │ State:        <unknown>                      │
    └──────────────────────────────────────────────┘
"#
        .trim_start_matches('\n');

        assert_eq!(expected, output);
    }

    #[test]
    pub fn test_info_result_json() {
        let output = serde_json::to_value(&*TEST_INFO_RESULT).unwrap();

        let expected = json!({
            "device_name": "0c8301e0",
            "token_id": 68,
            "device_path": "/dev/class/audio-input/0c8301e0",
            "unique_id": "000102030405060708090a0b0c0d0e0f",
            "manufacturer": "Test manufacturer",
            "product_name": "Test product",
            "gain_state": {
                "gain_db": -3.0,
                "muted": false,
                "agc_enabled": false
            },
            "gain_capabilities": {
                "min_gain_db": -100.0,
                "max_gain_db": 0.0,
                "gain_step_db": 0.0,
                "can_mute": true,
                "can_agc": false
            },
            "plug_event": {
                "state": "Plugged",
                "time": 123456789,
            },
            "plug_detect_capabilities": "Hardwired",
            "clock_domain": 0,
            "supported_dai_formats": {
                "1": [
                    {
                        "number_of_channels": [1, 2],
                        "sample_formats": ["pcm_signed", "pcm_unsigned"],
                        "frame_formats": ["stereo_left", "stereo_right"],
                        "frame_rates": [16000, 22050, 32000, 44100, 48000, 88200, 96000],
                        "bits_per_slot": [32],
                        "bits_per_sample": [8, 16]
                    }
                ],
                "123": [
                    {
                        "number_of_channels": [1],
                        "sample_formats": ["pcm_float"],
                        "frame_formats": ["custom:right_justified;raising_sclk;1;2"],
                        "frame_rates": [16000, 22050, 32000, 44100, 48000, 88200, 96000],
                        "bits_per_slot": [16, 32],
                        "bits_per_sample": [16]
                    }
                ]
            },
            "supported_ring_buffer_formats": {
                "0": [
                    {
                        "channel_sets": [
                            {
                                "attributes": [
                                    { "min_frequency": null, "max_frequency": null }
                                ]
                            },
                            {
                                "attributes": [
                                    { "min_frequency": null, "max_frequency": null },
                                    { "min_frequency": null, "max_frequency": null }
                                ]
                            }
                        ],
                        "sample_types": ["int16"],
                        "frame_rates": [16000, 22050, 32000, 44100, 48000, 88200, 96000]
                    }
                ],
                "123": [
                    {
                        "channel_sets": [
                            {
                                "attributes": [
                                    { "min_frequency": null, "max_frequency": null }
                                ]
                            }
                        ],
                        "sample_types": ["uint8", "int16"],
                        "frame_rates": [16000, 22050, 32000]
                    },
                    {
                        "channel_sets": [
                            {
                                "attributes": [
                                    { "min_frequency": null, "max_frequency": null }
                                ]
                            },
                            {
                                "attributes": [
                                    { "min_frequency": null, "max_frequency": null },
                                    { "min_frequency": null, "max_frequency": null }
                                ]
                            }
                        ],
                        "sample_types": ["float32"],
                        "frame_rates": [44100, 48000, 88200, 96000]
                    }
                ]
            },
            "topologies": [
                {
                    "id": INPUT_ONLY_TOPOLOGY_ID,
                    "edge_pairs": [
                        {
                            "processing_element_id_from": SOURCE_DAI_ELEMENT_ID,
                            "processing_element_id_to": DEST_RB_ELEMENT_ID
                        }
                    ],
                },
                {
                    "id": FULL_DUPLEX_TOPOLOGY_ID,
                    "edge_pairs": [
                        {
                            "processing_element_id_from": SOURCE_DAI_ELEMENT_ID,
                            "processing_element_id_to": DEST_RB_ELEMENT_ID
                        },
                        {
                            "processing_element_id_from": SOURCE_RB_ELEMENT_ID,
                            "processing_element_id_to": DEST_DAI_ELEMENT_ID
                        }
                    ]
                },
                {
                    "id": OUTPUT_ONLY_TOPOLOGY_ID,
                    "edge_pairs": [
                        {
                            "processing_element_id_from": SOURCE_RB_ELEMENT_ID,
                            "processing_element_id_to": DEST_DAI_ELEMENT_ID
                        }
                    ]
                },
                {
                    "id": OUTPUT_WITH_MUTE_TOPOLOGY_ID,
                    "edge_pairs": [
                        {
                            "processing_element_id_from": SOURCE_RB_ELEMENT_ID,
                            "processing_element_id_to": MUTE_ELEMENT_ID
                        },
                        {
                            "processing_element_id_from": MUTE_ELEMENT_ID,
                            "processing_element_id_to": DEST_DAI_ELEMENT_ID
                        }
                    ]
                },
            ],
            "topology_id": FULL_DUPLEX_TOPOLOGY_ID,
            "elements": [
                {
                    "id": SOURCE_DAI_ELEMENT_ID,
                    "type": "dai_interconnect",
                    "type_specific": {
                        "dai_interconnect": {
                            "plug_detect_capabilities": "can_async_notify"
                        }
                    },
                    "description": "Source DAI interconnect element",
                    "can_stop": true,
                    "can_bypass": false,
                    "state": {
                        "bypassed": null,
                        "started": true,
                        "turn_off_delay_ns": 0,
                        "turn_on_delay_ns": 0,
                        "processing_delay_ns": 0,
                        "type_specific": {
                            "dai_interconnect": {
                                "external_delay_ns": 5000,
                                "plug_state": {
                                    "plug_state_time": 123456789,
                                    "plugged": true
                                }
                            }
                        },
                        "vendor_specific_data": null
                    }
                },
                {
                    "id": DEST_DAI_ELEMENT_ID,
                    "type": "dai_interconnect",
                    "type_specific": {
                        "dai_interconnect": {
                            "plug_detect_capabilities": "can_async_notify"
                        }
                    },
                    "description": "Destination DAI interconnect element",
                    "can_stop": true,
                    "can_bypass": false,
                    "state": null
                },
                {
                    "id": SOURCE_RB_ELEMENT_ID,
                    "type": "ring_buffer",
                    "type_specific": null,
                    "description": "Source ring buffer element",
                    "can_stop": false,
                    "can_bypass": false,
                    "state": null
                },
                {
                    "id": DEST_RB_ELEMENT_ID,
                    "type": "ring_buffer",
                    "type_specific": null,
                    "description": "Destination ring buffer element",
                    "can_bypass": null,
                    "can_stop": false,
                    "state": null
                },
                {
                    "id": MUTE_ELEMENT_ID,
                    "type": "mute",
                    "type_specific": null,
                    "description": "Mute element",
                    "can_bypass": true,
                    "can_stop": false,
                    "state": null
                },
                {
                    "id": VENDOR_SPECIFIC_ELEMENT_ID,
                    "type": "vendor_specific",
                    "type_specific": {
                        "vendor_specific": {}
                    },
                    "description": "Vendor specific element",
                    "can_bypass": true,
                    "can_stop": false,
                    "state": {
                        "bypassed": false,
                        "started": true,
                        "turn_off_delay_ns": 34,
                        "turn_on_delay_ns": 12,
                        "processing_delay_ns": 56,
                        "type_specific": {
                            "vendor_specific": {}
                        },
                        "vendor_specific_data": "SGVsbG8Bd29ybGQu"
                    }
                },
                {
                    "id": CONNECTION_POINT_ELEMENT_ID,
                    "type": "connection_point",
                    "type_specific": null,
                    "description": "Connection point element",
                    "can_bypass": false,
                    "can_stop": false,
                    "state": null
                },
                {
                    "id": GAIN_ELEMENT_ID,
                    "type": "gain",
                    "type_specific": {
                        "gain": {
                            "domain": "digital",
                            "range": {
                                "max": 0.0,
                                "min": -100.0,
                                "min_step": 1.0
                            },
                            "type": "decibels"
                        }
                    },
                    "description": "Gain element",
                    "can_bypass": true,
                    "can_stop": null,
                    "state": {
                        "bypassed": false,
                        "started": true,
                        "turn_off_delay_ns": 0,
                        "turn_on_delay_ns": 0,
                        "processing_delay_ns": 0,
                        "type_specific": {
                            "gain": {
                                "gain": -3.0
                            }
                        },
                        "vendor_specific_data": null
                    }
                },
                {
                    "id": AGC_ELEMENT_ID,
                    "type": "automatic_gain_control",
                    "type_specific": null,
                    "description": "Automatic gain control element",
                    "can_bypass": true,
                    "can_stop": false,
                    "state": null
                },
                {
                    "id": AGL_ELEMENT_ID,
                    "type": "automatic_gain_limiter",
                    "type_specific": null,
                    "description": "Automatic gain limiter element",
                    "can_bypass": true,
                    "can_stop": false,
                    "state": null
                },
                {
                    "id": DYNAMICS_ELEMENT_ID,
                    "type": "dynamics",
                    "type_specific": {
                        "dynamics": {
                            "bands": [
                                {
                                    "id": 1
                                },
                                {
                                    "id": 2
                                }
                            ],
                            "supported_controls": 511
                        }
                    },
                    "description": "Dynamics element",
                    "can_bypass": true,
                    "can_stop": false,
                    "state": {
                        "bypassed": false,
                        "started": true,
                        "turn_off_delay_ns": 0,
                        "turn_on_delay_ns": 0,
                        "processing_delay_ns": 0,
                        "type_specific": {
                            "dynamics": {
                                "band_states": [
                                    {
                                        "attack": 10,
                                        "id": 1,
                                        "input_gain_db": 2.0,
                                        "knee_width_db": 1.0,
                                        "level_type": "peak",
                                        "linked_channels": true,
                                        "lookahead": 15,
                                        "max_frequency": 24000,
                                        "min_frequency": 0,
                                        "output_gain_db": 1.0,
                                        "ratio": 3.0,
                                        "release": 300,
                                        "threshold_db": -6.0,
                                        "threshold_type": "above"
                                    },
                                    {
                                        "attack": 1,
                                        "id": 2,
                                        "input_gain_db": 0.0,
                                        "knee_width_db": 6.0,
                                        "level_type": "rms",
                                        "linked_channels": false,
                                        "lookahead": 5,
                                        "max_frequency": 20000,
                                        "min_frequency": 100,
                                        "output_gain_db": 0.0,
                                        "ratio": 10.0,
                                        "release": 100,
                                        "threshold_db": 3.0,
                                        "threshold_type": "above"
                                    }
                                ]
                            }
                        },
                        "vendor_specific_data": null
                    }
                },
                {
                    "id": DELAY_ELEMENT_ID,
                    "type": "delay",
                    "type_specific": null,
                    "description": "Delay element",
                    "can_bypass": true,
                    "can_stop": false,
                    "state": null
                },
                {
                    "id": EQUALIZER_ELEMENT_ID,
                    "type": "equalizer",
                    "type_specific": {
                        "equalizer": {
                            "bands": [
                                {
                                    "id": 1
                                },
                                {
                                    "id": 2
                                }
                            ],
                            "can_disable_bands": false,
                            "max_frequency": 20000,
                            "max_gain_db": 24.0,
                            "max_q": 2.5,
                            "min_frequency": 10,
                            "min_gain_db": -24.0,
                            "supported_controls": 255
                        }
                    },
                    "description": "Equalizer element",
                    "can_bypass": true,
                    "can_stop": false,
                    "state": {
                        "bypassed": false,
                        "started": true,
                        "turn_off_delay_ns": 0,
                        "turn_on_delay_ns": 0,
                        "processing_delay_ns": 0,
                        "type_specific": {
                            "equalizer": {
                                "band_states": [
                                    {
                                        "enabled": true,
                                        "frequency": 300,
                                        "gain_db": 3.0,
                                        "id": 1,
                                        "q": 2.0,
                                        "type": "peak"
                                    },
                                    {
                                        "enabled": null,
                                        "frequency": 10000,
                                        "gain_db": null,
                                        "id": 2,
                                        "q": 10.0,
                                        "type": "high_cut"
                                    }
                                ]
                            }
                        },
                        "vendor_specific_data": null
                    }
                },
                {
                    "id": SRC_ELEMENT_ID,
                    "type": "sample_rate_conversion",
                    "type_specific": null,
                    "description": "Sample rate conversion element",
                    "can_bypass": true,
                    "can_stop": false,
                    "state": null
                }
            ],
        });

        assert_eq!(expected, output);
    }
}
