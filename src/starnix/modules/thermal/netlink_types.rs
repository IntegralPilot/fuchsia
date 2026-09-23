// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use linux_uapi::{
    THERMAL_GENL_FAMILY_NAME, THERMAL_GENL_VERSION,
    thermal_genl_attr_THERMAL_GENL_ATTR_TZ_ID as THERMAL_GENL_ATTR_TZ_ID,
    thermal_genl_attr_THERMAL_GENL_ATTR_TZ_TEMP as THERMAL_GENL_ATTR_TZ_TEMP,
    thermal_genl_sampling_THERMAL_GENL_SAMPLING_TEMP as THERMAL_GENL_SAMPLING_TEMP,
};
use netlink_packet_generic::{GenlFamily, GenlHeader};
use netlink_packet_utils::byteorder::{ByteOrder, NativeEndian};
use netlink_packet_utils::nla::{Nla, NlaBuffer, NlasIterator};
use netlink_packet_utils::parsers::parse_u32;
use netlink_packet_utils::{DecodeError, Emitable, Parseable, ParseableParametrized};
use std::mem::size_of_val;

pub fn celsius_to_millicelsius(temp: f32) -> f32 {
    temp * 1000.0
}

pub fn millicelsius_to_celsius(temp: f32) -> f32 {
    temp / 1000.0
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThermalAttr {
    ThermalZoneId(u32),
    ThermalZoneTemp(u32),
}

impl Nla for ThermalAttr {
    fn value_len(&self) -> usize {
        use ThermalAttr::*;
        match self {
            ThermalZoneId(v) => size_of_val(v),
            ThermalZoneTemp(v) => size_of_val(v),
        }
    }

    fn kind(&self) -> u16 {
        use ThermalAttr::*;
        match self {
            ThermalZoneId(_) => THERMAL_GENL_ATTR_TZ_ID as u16,
            ThermalZoneTemp(_) => THERMAL_GENL_ATTR_TZ_TEMP as u16,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use ThermalAttr::*;
        match self {
            ThermalZoneId(id) => NativeEndian::write_u32(buffer, *id),
            ThermalZoneTemp(temp) => NativeEndian::write_u32(buffer, *temp),
        };
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> Parseable<NlaBuffer<&'a T>> for ThermalAttr {
    type Error = DecodeError;
    fn parse(buf: &NlaBuffer<&'a T>) -> Result<Self, DecodeError> {
        let payload = buf.value();
        let kind = buf.kind() as u32;

        Ok(match kind {
            THERMAL_GENL_ATTR_TZ_ID => Self::ThermalZoneId(parse_u32(payload)?),
            THERMAL_GENL_ATTR_TZ_TEMP => Self::ThermalZoneTemp(parse_u32(payload)?),
            kind => return Err(DecodeError::from(format!("Unknown NLA type: {kind}"))),
        })
    }
}

/// Command code definition of Netlink controller (thermal) family
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenlThermalCmd {
    /// Notify from sample.
    ThermalGenlSamplingTemp,
}

impl From<GenlThermalCmd> for u8 {
    fn from(cmd: GenlThermalCmd) -> u8 {
        use GenlThermalCmd::*;
        match cmd {
            ThermalGenlSamplingTemp => THERMAL_GENL_SAMPLING_TEMP as u8,
        }
    }
}

impl TryFrom<u8> for GenlThermalCmd {
    type Error = DecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        use GenlThermalCmd::*;
        let cmd_code = value as u32;

        Ok(match cmd_code {
            THERMAL_GENL_SAMPLING_TEMP => ThermalGenlSamplingTemp,
            cmd => return Err(DecodeError::from(format!("Unknown thermal  command: {cmd}"))),
        })
    }
}

/// Payload of thermal netlink server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenlThermalPayload {
    /// Command code of this message
    pub cmd: GenlThermalCmd,
    /// Netlink attributes in this message
    pub nlas: Vec<ThermalAttr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenlThermal {
    pub payload: GenlThermalPayload,
    /// The family ID of the message.
    /// Assigned by the GenericNetlink server.
    pub family_id: u16,
}

impl GenlFamily for GenlThermal {
    fn family_name() -> &'static str {
        THERMAL_GENL_FAMILY_NAME.to_str().expect("family name has invalid UTF-8 data")
    }

    fn family_id(&self) -> u16 {
        self.family_id
    }

    fn command(&self) -> u8 {
        self.payload.cmd.into()
    }

    fn version(&self) -> u8 {
        THERMAL_GENL_VERSION as u8
    }
}

impl Emitable for GenlThermal {
    fn emit(&self, buffer: &mut [u8]) {
        self.payload.nlas.as_slice().emit(buffer)
    }

    fn buffer_len(&self) -> usize {
        self.payload.nlas.as_slice().buffer_len()
    }
}

impl ParseableParametrized<[u8], GenlHeader> for GenlThermalPayload {
    type Error = DecodeError;
    fn parse_with_param(buf: &[u8], header: GenlHeader) -> Result<Self, DecodeError> {
        let nlas = NlasIterator::new(buf)
            .map(|nla| {
                nla.map_err(|err| DecodeError::Nla(err)).and_then(|nla| ThermalAttr::parse(&nla))
            })
            .collect::<Result<Vec<_>, _>>()
            .context("failed to parse thermal message attributes")?;

        Ok(Self { cmd: header.cmd.try_into()?, nlas })
    }
}
