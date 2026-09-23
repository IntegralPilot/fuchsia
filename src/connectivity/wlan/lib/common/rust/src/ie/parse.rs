// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::*;
use crate::buffer_reader::BufferReader;
use crate::error::{FrameParseError, FrameParseResult};
use crate::organization::Oui;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use paste::paste;
use zerocopy::{Ref, SplitByteSlice};

macro_rules! validate {
    ( $condition:expr, $message:expr ) => {
        if !$condition {
            return Err($crate::error::FrameParseError(format!($message)));
        }
    };
}

macro_rules! simple_parse_func {
    ( $ie_snake_case:ident ) => {
        paste! {
            pub fn [<parse_ $ie_snake_case>]<B: SplitByteSlice>(
                raw_body: B,
            ) -> FrameParseResult<Ref<B, [<$ie_snake_case:camel>]>> {
                Ref::from_bytes(raw_body)
                    .map_err(|_| FrameParseError(
                        format!(concat!(
                            "Invalid length or alignment for ",
                            stringify!([<$ie_snake_case:camel>])))))
            }
        }
    };
}

// Each of the following creates a `parse_some_ie()` function associated with a `SomeIe` type.
simple_parse_func!(dsss_param_set);
simple_parse_func!(ht_capabilities);
simple_parse_func!(ht_operation);
simple_parse_func!(rm_enabled_capabilities);
simple_parse_func!(vht_capabilities);
simple_parse_func!(vht_operation);
simple_parse_func!(wmm_info);
simple_parse_func!(wmm_param);
simple_parse_func!(channel_switch_announcement);
simple_parse_func!(extended_channel_switch_announcement);
simple_parse_func!(wide_bandwidth_channel_switch);
simple_parse_func!(sec_chan_offset);

pub fn parse_ssid<B: SplitByteSlice>(raw_body: B) -> FrameParseResult<B> {
    validate!(raw_body.len() <= (fidl_ieee80211::MAX_SSID_BYTE_LEN as usize), "SSID is too long");
    Ok(raw_body)
}

pub fn parse_supported_rates<B: SplitByteSlice>(
    raw_body: B,
) -> FrameParseResult<Ref<B, [SupportedRate]>> {
    // IEEE Std 802.11-2016, 9.2.4.3 specifies that the Supported Rates IE may contain at most
    // eight rates. However, in practice some devices transmit more (rather than using Extended
    // Supported Rates). As the rates are encoded in a standard IE, this function does not validate
    // the number of rates to improve interoperability.
    validate!(!raw_body.is_empty(), "Empty Supported Rates IE");
    // unwrap() is OK because sizeof(SupportedRate) is 1, and any slice length is a multiple of 1
    Ok(Ref::from_bytes(raw_body).unwrap())
}

pub fn parse_extended_supported_rates<B: SplitByteSlice>(
    raw_body: B,
) -> FrameParseResult<Ref<B, [SupportedRate]>> {
    validate!(!raw_body.is_empty(), "Empty Extended Supported Rates IE");
    // The maximum number of extended supported rates (each a single u8) is the same as the
    // maximum number of bytes in an IE. Therefore, there is no need to check the max length
    // of the extended supported rates IE body.
    // unwrap() is OK because sizeof(SupportedRate) is 1, and any slice length is a multiple of 1
    Ok(Ref::from_bytes(raw_body).unwrap())
}

pub fn parse_tim<B: SplitByteSlice>(raw_body: B) -> FrameParseResult<TimView<B>> {
    let (header, bitmap) = Ref::<B, TimHeader>::from_prefix(raw_body).map_err(Into::into).map_err(
        |_: zerocopy::SizeError<_, _>| {
            FrameParseError(format!("Element body is too short to include a TIM header"))
        },
    )?;
    validate!(!bitmap.is_empty(), "Bitmap in TIM is empty");
    validate!(bitmap.len() <= TIM_MAX_BITMAP_LEN, "Bitmap in TIM is too long");
    Ok(TimView { header: *header, bitmap })
}

pub fn parse_country<B: SplitByteSlice>(raw_body: B) -> FrameParseResult<CountryView<B>> {
    let mut reader = BufferReader::new(raw_body);
    let country_code = reader.read::<[u8; 2]>().ok_or_else(|| {
        FrameParseError(format!("Element body is too short to include a country code"))
    })?;
    let environment = reader.read_byte().ok_or_else(|| {
        FrameParseError(format!("Element body is too short to include the whole country string"))
    })?;
    Ok(CountryView {
        country_code: *country_code,
        environment: CountryEnvironment(environment),
        subbands: reader.into_remaining(),
    })
}

pub fn parse_ext_capabilities<B: SplitByteSlice>(raw_body: B) -> ExtCapabilitiesView<B> {
    let mut reader = BufferReader::new(raw_body);
    let ext_caps_octet_1 = reader.read();
    let ext_caps_octet_2 = reader.read();
    let ext_caps_octet_3 = reader.read();
    ExtCapabilitiesView {
        ext_caps_octet_1,
        ext_caps_octet_2,
        ext_caps_octet_3,
        remaining: reader.into_remaining(),
    }
}

pub fn parse_wpa_ie<B: SplitByteSlice>(raw_body: B) -> FrameParseResult<wpa::WpaIe> {
    wpa::from_bytes(&raw_body[..])
        .map(|(_, r)| r)
        .map_err(|_| FrameParseError(format!("Failed to parse WPA IE")))
}

pub fn parse_transmit_power_envelope<B: SplitByteSlice>(
    raw_body: B,
) -> FrameParseResult<TransmitPowerEnvelopeView<B>> {
    let mut reader = BufferReader::new(raw_body);
    let transmit_power_info = reader
        .read::<TransmitPowerInfo>()
        .ok_or_else(|| FrameParseError(format!("Transmit Power Envelope element too short")))?;
    if transmit_power_info.max_transmit_power_count() > 3 {
        return FrameParseResult::Err(FrameParseError(format!(
            "Invalid transmit power count for Transmit Power Envelope element"
        )));
    }
    let expected_bytes_remaining = transmit_power_info.max_transmit_power_count() as usize + 1;
    if reader.bytes_remaining() < expected_bytes_remaining {
        return FrameParseResult::Err(FrameParseError(format!(
            "Transmit Power Envelope element too short"
        )));
    } else if reader.bytes_remaining() > expected_bytes_remaining {
        return FrameParseResult::Err(FrameParseError(format!(
            "Transmit Power Envelope element too long"
        )));
    }
    // Unwrap safe due to checks above.
    let max_transmit_power_20 = reader.read().unwrap();
    let max_transmit_power_40 = reader.read();
    let max_transmit_power_80 = reader.read();
    let max_transmit_power_160 = reader.read();
    FrameParseResult::Ok(TransmitPowerEnvelopeView {
        transmit_power_info,
        max_transmit_power_20,
        max_transmit_power_40,
        max_transmit_power_80,
        max_transmit_power_160,
    })
}

pub fn parse_channel_switch_wrapper<B: SplitByteSlice>(
    raw_body: B,
) -> FrameParseResult<ChannelSwitchWrapperView<B>> {
    let mut result = ChannelSwitchWrapperView {
        new_country: None,
        wide_bandwidth_channel_switch: None,
        new_transmit_power_envelope: None,
    };
    let ie_reader = crate::ie::Reader::new(raw_body);
    for (ie_id, ie_body) in ie_reader {
        match ie_id {
            Id::COUNTRY => {
                result.new_country.replace(parse_country(ie_body)?);
            }
            Id::WIDE_BANDWIDTH_CHANNEL_SWITCH => {
                result
                    .wide_bandwidth_channel_switch
                    .replace(parse_wide_bandwidth_channel_switch(ie_body)?);
            }
            Id::TRANSMIT_POWER_ENVELOPE => {
                result.new_transmit_power_envelope.replace(parse_transmit_power_envelope(ie_body)?);
            }
            _ => {
                return Err(FrameParseError(format!(
                    "Unexpected sub-element Id in Channel Switch Wrapper"
                )));
            }
        }
    }
    FrameParseResult::Ok(result)
}

pub fn parse_vendor_ie<B: SplitByteSlice>(raw_body: B) -> FrameParseResult<VendorIe<B>> {
    let mut reader = BufferReader::new(raw_body);
    let oui = *reader
        .read::<Oui>()
        .ok_or_else(|| FrameParseError(format!("Failed to read vendor OUI")))?;
    let vendor_ie = match oui {
        Oui::MSFT => {
            let ie_type = reader.peek_byte();
            match ie_type {
                Some(wpa::VENDOR_SPECIFIC_TYPE) => {
                    // We already know from our peek_byte that at least one byte remains, so this
                    // split will not panic.
                    let (_type, body) = reader.into_remaining().split_at(1).ok().unwrap();
                    VendorIe::MsftLegacyWpa(body)
                }
                Some(wsc::VENDOR_SPECIFIC_TYPE) => {
                    let (_type, body) = reader.into_remaining().split_at(1).ok().unwrap();
                    VendorIe::Wsc(body)
                }
                // The first three bytes after OUI are OUI type, OUI subtype, and version.
                Some(WMM_OUI_TYPE) if reader.bytes_remaining() >= 3 => {
                    let body = reader.into_remaining();
                    let subtype = body[1];
                    // The version byte is 0x01 for both WMM Information and Parameter elements
                    // as of WFA WMM v1.2.0.
                    if body[2] != 0x01 {
                        return Err(FrameParseError(format!("Unexpected WMM Version byte")));
                    }
                    match subtype {
                        // Safe to split because we already checked that there are at least 3
                        // bytes remaining.
                        WMM_INFO_OUI_SUBTYPE => VendorIe::WmmInfo(body.split_at(3).ok().unwrap().1),
                        WMM_PARAM_OUI_SUBTYPE => {
                            VendorIe::WmmParam(body.split_at(3).ok().unwrap().1)
                        }
                        _ => VendorIe::Unknown { oui, body },
                    }
                }
                _ => VendorIe::Unknown { oui, body: reader.into_remaining() },
            }
        }
        Oui::WFA => {
            let ie_type = reader.peek_byte();
            match ie_type {
                Some(owe_transition::VENDOR_SPECIFIC_TYPE) => {
                    let (_type, body) = reader.into_remaining().split_at(1).ok().unwrap();
                    VendorIe::OweTransition(body)
                }
                _ => VendorIe::Unknown { oui, body: reader.into_remaining() },
            }
        }
        _ => VendorIe::Unknown { oui, body: reader.into_remaining() },
    };
    Ok(vendor_ie)
}

pub fn parse_rsnxe<B: SplitByteSlice>(raw_body: B) -> RsnxeView<B> {
    let mut reader = BufferReader::new(raw_body);
    let rsnxe_octet_1 = reader.read();
    RsnxeView { rsnxe_octet_1, remaining: reader.into_remaining() }
}

pub fn parse_diffie_hellman_param<B: SplitByteSlice>(
    raw_body: B,
) -> FrameParseResult<DiffieHellmanParamView<B>> {
    let mut reader = BufferReader::new(raw_body);
    let group = reader
        .read_value::<u16>()
        .ok_or_else(|| FrameParseError(format!("Element body is too short to include DH group")))?;
    let public_key = reader.into_remaining();
    Ok(DiffieHellmanParamView { group, public_key })
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

    #[repr(C)]
    #[derive(IntoBytes, KnownLayout, FromBytes, Immutable)]
    pub struct SomeIe {
        some_field: u16,
    }
    simple_parse_func!(some_ie);

    #[test]
    pub fn simple_parse_func_ok() {
        let some_ie = parse_some_ie(&[0xfa, 0xde][..]).unwrap();
        assert_eq!(some_ie.some_field, 0xdefa);
    }

    #[test]
    pub fn simple_parse_func_wrong_size() {
        let err_too_short = parse_some_ie(&[0xfa][..]).err().unwrap();
        assert_eq!(
            "Error parsing frame: Invalid length or alignment for SomeIe",
            &err_too_short.to_string()
        );
        let err_too_long = parse_some_ie(&[0xfa, 0xde, 0xed][..]).err().unwrap();
        assert_eq!(
            "Error parsing frame: Invalid length or alignment for SomeIe",
            &err_too_long.to_string()
        );
    }

    #[test]
    pub fn simple_parse_func_wrong_alignment() {
        // Construct valid length but incorrectly aligned SomeIe
        struct Buf {
            b: [u8; 3],
            _t: u16, // Make Buf align to u16
        }
        let buf = Buf { b: [0x00, 0xfa, 0xde], _t: 0 };
        let buf_slice = &buf.b[1..];
        assert_eq!(buf_slice.len(), std::mem::size_of::<SomeIe>());

        let err_not_aligned = parse_some_ie(buf_slice).err().unwrap();
        assert_eq!(
            "Error parsing frame: Invalid length or alignment for SomeIe",
            &err_not_aligned.to_string()
        );
    }

    #[test]
    pub fn ssid_ok() {
        assert_eq!(Ok(&[][..]), parse_ssid(&[][..]));
        assert_eq!(Ok(&[1, 2, 3][..]), parse_ssid(&[1, 2, 3][..]));
    }

    #[test]
    pub fn ssid_too_long() {
        assert_eq!(Err(FrameParseError(format!("SSID is too long"))), parse_ssid(&[0u8; 33][..]));
    }

    #[test]
    pub fn supported_rates_ok() {
        let r = parse_supported_rates(&[1, 2, 3][..]).expect("expected Ok");
        assert_eq!(&[SupportedRate(1), SupportedRate(2), SupportedRate(3)][..], &r[..]);
    }

    #[test]
    pub fn supported_rates_empty() {
        let err = parse_supported_rates(&[][..]).expect_err("expected Err");
        assert_eq!("Error parsing frame: Empty Supported Rates IE", &err.to_string());
    }

    // This test expects to pass despite IEEE Std 802.11-2016, 9.2.4.3 specifying a limit of eight
    // rates. This limit is intentionally ignored when parsing Supported Rates to improve
    // interoperability with devices that write more than eight rates into the IE.
    #[test]
    pub fn supported_rates_ok_overloaded() {
        let rates =
            parse_supported_rates(&[0u8; 9][..]).expect("rejected overloaded Supported Rates IE");
        assert_eq!(&rates[..], &[SupportedRate(0); 9][..],);
    }

    #[test]
    pub fn tim_ok() {
        let r = parse_tim(&[1, 2, 3, 4, 5][..]).expect("expected Ok");
        assert_eq!(2, r.header.dtim_period);
        assert_eq!(&[4, 5][..], r.bitmap);
    }

    #[test]
    pub fn tim_too_short_for_header() {
        let err = parse_tim(&[1, 2][..]).err().expect("expected Err");
        assert_eq!(
            "Error parsing frame: Element body is too short to include a TIM header",
            &err.to_string()
        );
    }

    #[test]
    pub fn tim_empty_bitmap() {
        let err = parse_tim(&[1, 2, 3][..]).err().expect("expected Err");
        assert_eq!("Error parsing frame: Bitmap in TIM is empty", &err.to_string());
    }

    #[test]
    pub fn tim_bitmap_too_long() {
        let err = parse_tim(&[0u8; 255][..]).err().expect("expected Err");
        assert_eq!("Error parsing frame: Bitmap in TIM is too long", &err.to_string());
    }

    #[test]
    pub fn country_ok() {
        // Country element without Element Id and length
        #[rustfmt::skip]
        let raw_body = [
            0x55, 0x53, // Country: US
            0x20, // Environment: Any
            0x24, 0x04, 0x24, // Subband triplet 1
            0x34, 0x04, 0x1e, // Subband triplet 2
            0x64, 0x0c, 0x1e, // Subband triplet 3
            0x95, 0x05, 0x24, // Subband triplet 4
            0x00, // padding
        ];
        let country = parse_country(&raw_body[..]).expect("valid frame should result in OK");

        assert_eq!(country.country_code, [0x55, 0x53]);
        assert_eq!(country.environment, CountryEnvironment::ANY);
        assert_eq!(country.subbands, &raw_body[3..]);
    }

    #[test]
    pub fn country_too_short() {
        let err = parse_country(&[0x55, 0x53][..]).err().expect("expected Err");
        assert_eq!(
            "Error parsing frame: Element body is too short to include the whole country string",
            &err.to_string()
        );
    }

    #[test]
    pub fn channel_switch_announcement() {
        let raw_csa = [1, 30, 40];
        let csa =
            parse_channel_switch_announcement(&raw_csa[..]).expect("valid CSA should result in OK");
        assert_eq!(csa.mode, 1);
        assert_eq!(csa.new_channel_number, 30);
        assert_eq!(csa.channel_switch_count, 40);
    }

    #[test]
    pub fn extended_channel_switch_announcement_valid_2g() {
        let raw_ecsa = [1, 81, 30, 40];
        let ecsa = parse_extended_channel_switch_announcement(&raw_ecsa[..])
            .expect("valid ECSA should result in OK");
        assert_eq!(ecsa.mode, 1);
        assert_eq!(ecsa.new_operating_class, 81);
        assert_eq!(ecsa.new_channel_number, 30);
        assert_eq!(ecsa.channel_switch_count, 40);
        assert_eq!(ecsa.get_band(), fidl_ieee80211::WlanBand::TwoGhz);
    }

    #[test]
    pub fn extended_channel_switch_announcement_valid_5g() {
        let raw_ecsa = [1, 115, 30, 40];
        let ecsa = parse_extended_channel_switch_announcement(&raw_ecsa[..])
            .expect("valid ECSA should result in OK");
        assert_eq!(ecsa.get_band(), fidl_ieee80211::WlanBand::FiveGhz);
    }

    #[test]
    pub fn extended_channel_switch_announcement_invalid_op_class() {
        let raw_ecsa = [1, 20, 30, 40];
        let ecsa = parse_extended_channel_switch_announcement(&raw_ecsa[..])
            .expect("valid ECSA should result in OK even with invalid op class");
        assert_eq!(ecsa.get_band(), fidl_ieee80211::WlanBand::unknown());
    }

    #[test]
    pub fn wide_bandwidth_channel_switch() {
        let raw_wbcs = [0, 10, 20];
        let wbcs = parse_wide_bandwidth_channel_switch(&raw_wbcs[..])
            .expect("valid WBCS should result in OK");
        assert_eq!(wbcs.new_width, VhtChannelBandwidth::CBW_20_40);
        assert_eq!(wbcs.new_center_freq_seg0, 10);
        assert_eq!(wbcs.new_center_freq_seg1, 20);
    }

    #[test]
    pub fn transmit_power_envelope_view() {
        #[rustfmt::skip]
        let raw_tpe = [
            // transmit power information: All fields present, EIRP unit
            0b00_000_011,
            20, 40, 80, 160,
        ];
        let tpe =
            parse_transmit_power_envelope(&raw_tpe[..]).expect("valid TPE should result in OK");
        assert_eq!(tpe.transmit_power_info.max_transmit_power_count(), 3);
        assert_eq!(
            tpe.transmit_power_info.max_transmit_power_unit_interpretation(),
            MaxTransmitPowerUnitInterpretation::EIRP
        );
        assert_eq!(*tpe.max_transmit_power_20, TransmitPower(20));
        assert_eq!(tpe.max_transmit_power_40.map(|t| *t), Some(TransmitPower(40)));
        assert_eq!(tpe.max_transmit_power_80.map(|t| *t), Some(TransmitPower(80)));
        assert_eq!(tpe.max_transmit_power_160.map(|t| *t), Some(TransmitPower(160)));
    }

    #[test]
    pub fn transmit_power_envelope_view_20_only() {
        #[rustfmt::skip]
        let raw_tpe = [
            // transmit power information: Only 20 MHz, EIRP unit
            0b00_000_000,
            20,
        ];
        let tpe =
            parse_transmit_power_envelope(&raw_tpe[..]).expect("valid TPE should result in OK");
        assert_eq!(tpe.transmit_power_info.max_transmit_power_count(), 0);
        assert_eq!(
            tpe.transmit_power_info.max_transmit_power_unit_interpretation(),
            MaxTransmitPowerUnitInterpretation::EIRP
        );
        assert_eq!(*tpe.max_transmit_power_20, TransmitPower(20));
        assert_eq!(tpe.max_transmit_power_40, None);
        assert_eq!(tpe.max_transmit_power_80, None);
        assert_eq!(tpe.max_transmit_power_160, None);
    }

    #[test]
    pub fn transmit_power_envelope_view_too_long() {
        #[rustfmt::skip]
        let raw_tpe = [
            // transmit power information: Only 20 MHz, EIRP unit
            0b00_000_000,
            20, 40, 80, 160
        ];
        let err = parse_transmit_power_envelope(&raw_tpe[..]).err().expect("expected Err");
        assert_eq!(
            "Error parsing frame: Transmit Power Envelope element too long",
            &err.to_string()
        );
    }

    #[test]
    pub fn transmit_power_envelope_view_too_short() {
        #[rustfmt::skip]
        let raw_tpe = [
            // transmit power information: 20 + 40 MHz, EIRP unit
            0b00_000_001,
            20,
        ];
        let err = parse_transmit_power_envelope(&raw_tpe[..]).err().expect("expected Err");
        assert_eq!(
            "Error parsing frame: Transmit Power Envelope element too short",
            &err.to_string()
        );
    }

    #[test]
    pub fn transmit_power_envelope_invalid_count() {
        #[rustfmt::skip]
        let raw_tpe = [
            // transmit power information: Invalid count (4), EIRP unit
            0b00_000_100,
            20,
        ];
        let err = parse_transmit_power_envelope(&raw_tpe[..]).err().expect("expected Err");
        assert_eq!(
            "Error parsing frame: Invalid transmit power count for Transmit Power Envelope element",
            &err.to_string()
        );
    }

    #[test]
    pub fn channel_switch_wrapper_view() {
        #[rustfmt::skip]
        let raw_csw = [
            Id::COUNTRY.0, 3, b'U', b'S', b'O',
            Id::WIDE_BANDWIDTH_CHANNEL_SWITCH.0, 3, 0, 10, 20,
            Id::TRANSMIT_POWER_ENVELOPE.0, 2, 0b00_000_000, 20,
        ];
        let csw =
            parse_channel_switch_wrapper(&raw_csw[..]).expect("valid CSW should result in OK");
        let country = csw.new_country.expect("New country present in CSW.");
        assert_eq!(country.country_code, [b'U', b'S']);
        assert_eq!(country.environment, CountryEnvironment::OUTDOOR);
        assert_matches!(csw.wide_bandwidth_channel_switch, Some(wbcs) => {
            assert_eq!(wbcs.new_width, VhtChannelBandwidth::CBW_20_40);
            assert_eq!(wbcs.new_center_freq_seg0, 10);
            assert_eq!(wbcs.new_center_freq_seg1, 20);
        });
        let tpe = csw.new_transmit_power_envelope.expect("Transmit power present in CSW.");
        assert_eq!(*tpe.max_transmit_power_20, TransmitPower(20));
        assert_eq!(tpe.max_transmit_power_40, None);
        assert_eq!(tpe.max_transmit_power_80, None);
        assert_eq!(tpe.max_transmit_power_160, None);
    }

    #[test]
    pub fn partial_channel_switch_wrapper_view() {
        #[rustfmt::skip]
        let raw_csw = [
            Id::WIDE_BANDWIDTH_CHANNEL_SWITCH.0, 3, 0, 10, 20,
        ];
        let csw =
            parse_channel_switch_wrapper(&raw_csw[..]).expect("valid CSW should result in OK");
        assert!(csw.new_country.is_none());
        assert_matches!(csw.wide_bandwidth_channel_switch, Some(wbcs) => {
            assert_eq!(wbcs.new_width, VhtChannelBandwidth::CBW_20_40);
            assert_eq!(wbcs.new_center_freq_seg0, 10);
            assert_eq!(wbcs.new_center_freq_seg1, 20);
        });
        assert!(csw.new_transmit_power_envelope.is_none());
    }

    #[test]
    pub fn channel_switch_wrapper_view_unexpected_subelement() {
        #[rustfmt::skip]
        let raw_csw = [
            Id::WIDE_BANDWIDTH_CHANNEL_SWITCH.0, 3, 40, 10, 20,
            Id::HT_OPERATION.0, 3, 1, 2, 3,
        ];
        let err = parse_channel_switch_wrapper(&raw_csw[..]).err().expect("expected Err");
        assert_eq!(
            "Error parsing frame: Unexpected sub-element Id in Channel Switch Wrapper",
            &err.to_string()
        );
    }

    #[test]
    fn ht_capabilities_ok() {
        // HtCapabilities element without Element Id and length
        #[rustfmt::skip]
        let raw_body = [
            0x4e, 0x11, // HtCapabilitiInfo(u16)
            0x1b, // AmpduParams(u8)
            0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xab, 0xcd, 0x00, 0x00, 0x00, 0x00, // SupportedMcsSet(u128)
            0x06, 0x03, // HtExtCapabilities(u16)
            0xc0, 0xb0, 0xcb, 0x13, // TxBfCapability(u32)
            0x00, // AselCapability(u8)
        ];
        let ht_cap = parse_ht_capabilities(&raw_body[..]).expect("valid frame should result in OK");

        let ht_cap_info = ht_cap.ht_cap_info;
        assert_eq!(ht_cap_info.0, 0x114e);
        assert_eq!(ht_cap_info.chan_width_set(), ChanWidthSet::TWENTY_FORTY);
        assert_eq!(ht_cap_info.sm_power_save(), SmPowerSave::DISABLED);
        assert_eq!(ht_cap_info.max_amsdu_len(), MaxAmsduLen::OCTETS_3839);

        let ampdu_params = ht_cap.ampdu_params;
        assert_eq!(ampdu_params.0, 0x1b);
        assert_eq!(ampdu_params.max_ampdu_exponent().to_len(), 65535);
        assert_eq!(ampdu_params.min_start_spacing(), MinMpduStartSpacing::EIGHT_USEC);

        let mcs_set = ht_cap.mcs_set;
        assert_eq!(mcs_set.0, 0x00000000_cdab0000_00000000_000000ff);
        assert_eq!(mcs_set.rx_mcs().0, 0xff);
        assert_eq!(mcs_set.rx_mcs().support(7), true);
        assert_eq!(mcs_set.rx_mcs().support(8), false);
        assert_eq!(mcs_set.rx_highest_rate(), 0x01ab);

        let ht_ext_cap = ht_cap.ht_ext_cap;
        let raw_value = ht_ext_cap.0;
        assert_eq!(raw_value, 0x0306);
        assert_eq!(ht_ext_cap.pco_transition(), PcoTransitionTime::PCO_5000_USEC);
        assert_eq!(ht_ext_cap.mcs_feedback(), McsFeedback::BOTH);

        let txbf_cap = ht_cap.txbf_cap;
        let raw_value = txbf_cap.0;
        assert_eq!(raw_value, 0x13cbb0c0);
        assert_eq!(txbf_cap.calibration(), Calibration::RESPOND_INITIATE);
        assert_eq!(txbf_cap.csi_feedback(), Feedback::IMMEDIATE);
        assert_eq!(txbf_cap.noncomp_feedback(), Feedback::DELAYED);
        assert_eq!(txbf_cap.min_grouping(), MinGroup::TWO);

        // human-readable representation
        assert_eq!(txbf_cap.csi_antennas().to_human(), 2);
        assert_eq!(txbf_cap.noncomp_steering_ants().to_human(), 3);
        assert_eq!(txbf_cap.comp_steering_ants().to_human(), 4);
        assert_eq!(txbf_cap.csi_rows().to_human(), 2);
        assert_eq!(txbf_cap.chan_estimation().to_human(), 3);

        let asel_cap = ht_cap.asel_cap;
        assert_eq!(asel_cap.0, 0);
    }

    #[test]
    pub fn extended_supported_rates_ok() {
        let r = parse_extended_supported_rates(&[1, 2, 3][..]).expect("expected Ok");
        assert_eq!(&[SupportedRate(1), SupportedRate(2), SupportedRate(3)][..], &r[..]);
    }

    #[test]
    pub fn extended_supported_rates_empty() {
        let err = parse_extended_supported_rates(&[][..]).expect_err("expected Err");
        assert_eq!("Error parsing frame: Empty Extended Supported Rates IE", &err.to_string());
    }

    #[test]
    fn ht_operation_ok() {
        // HtOperation element without Element Id and length
        #[rustfmt::skip]
        let raw_body = [
            99, // primary_channel
            0xff, 0xfe, 0xff, 0xff, 0xff, // ht_op_info
            // basic_ht_mcs_set
            0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0xab, 0xcd, 0x00, 0x00, 0x00, 0x00,
        ];
        let ht_op = parse_ht_operation(&raw_body[..]).expect("valid frame should result in OK");

        assert_eq!(ht_op.primary_channel, 99);

        let ht_op_info = ht_op.ht_op_info;
        assert_eq!(ht_op_info.secondary_chan_offset(), SecChanOffset::SECONDARY_BELOW);
        assert_eq!(ht_op_info.sta_chan_width(), StaChanWidth::ANY);
        assert_eq!(ht_op_info.ht_protection(), HtProtection::TWENTY_MHZ);
        assert_eq!(ht_op_info.pco_phase(), PcoPhase::FORTY_MHZ);

        let basic_mcs_set = ht_op.basic_ht_mcs_set;
        assert_eq!(basic_mcs_set.0, 0x00000000_cdab0000_00000000_000000ff);
    }

    #[test]
    fn rm_enabled_capabilities_ok() {
        #[rustfmt::skip]
        let raw_body = [
            0x03, 0x00, 0x00, 0x00, 0x02, // rm_enabled_capabilities
        ];

        let caps =
            parse_rm_enabled_capabilities(&raw_body[..]).expect("valid frame should result in OK");
        assert!(caps.link_measurement_enabled());
        assert!(caps.neighbor_report_enabled());
        assert!(!caps.lci_azimuth_enabled());
        assert!(caps.antenna_enabled());
        assert!(!caps.ftm_range_report_enabled());
    }

    #[test]
    fn sec_chan_offset_ok() {
        let sec_chan_offset =
            parse_sec_chan_offset(&[3][..]).expect("valid sec chan offset should result in OK");
        assert_eq!(sec_chan_offset.0, 3);
    }

    #[test]
    fn ext_capabilities_ok() {
        let data = [0x04, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x40];
        let ext_capabilities = parse_ext_capabilities(&data[..]);
        assert_matches!(ext_capabilities.ext_caps_octet_1, Some(caps) => {
            assert!(caps.extended_channel_switching());
            assert!(!caps.psmp_capability());
        });
        assert_matches!(ext_capabilities.ext_caps_octet_2, Some(caps) => {
            assert!(!caps.civic_location());
        });
        assert_matches!(ext_capabilities.ext_caps_octet_3, Some(caps) => {
            assert!(caps.bss_transition());
            assert!(!caps.ac_station_count());
        });
        assert_eq!(ext_capabilities.remaining, &[0x00, 0x00, 0x00, 0x00, 0x40]);
    }

    #[test]
    fn vht_capabilities_ok() {
        // VhtCapabilities element without Element Id and length
        #[rustfmt::skip]
        let raw_body = [
            0xfe, 0xff, 0xff, 0xff, // VhtCapabilitiesInfo(u32)
            0xff, 0xaa, 0x00, 0x00, 0x55, 0xff, 0x00, 0x00, // VhtMcsNssSet(u64)
        ];
        let vht_cap = parse_vht_capabilities(&raw_body[..]).expect("expected OK from valid frames");

        let cap_info = vht_cap.vht_cap_info;
        assert_eq!(cap_info.max_mpdu_len(), MaxMpduLen::OCTECTS_11454);
        assert_eq!(cap_info.link_adapt(), VhtLinkAdaptation::BOTH);
        let max_ampdu_component = cap_info.max_ampdu_exponent();
        assert_eq!(max_ampdu_component.to_len(), 1048575);

        let mcs_nss = vht_cap.vht_mcs_nss;
        assert_eq!(mcs_nss.rx_max_mcs().ss1(), VhtMcsSet::NONE);
        assert_eq!(mcs_nss.rx_max_mcs().ss7(), VhtMcsSet::UP_TO_9);
        assert_eq!(mcs_nss.tx_max_mcs().ss1(), VhtMcsSet::UP_TO_8);
        assert_eq!(mcs_nss.tx_max_mcs().ss7(), VhtMcsSet::NONE);

        assert_eq!(mcs_nss.rx_max_mcs().ss(2), Ok(VhtMcsSet::NONE));
        assert_eq!(mcs_nss.rx_max_mcs().ss(6), Ok(VhtMcsSet::UP_TO_9));
        assert_eq!(mcs_nss.tx_max_mcs().ss(2), Ok(VhtMcsSet::UP_TO_8));
        assert_eq!(mcs_nss.tx_max_mcs().ss(6), Ok(VhtMcsSet::NONE));
    }

    #[test]
    fn vht_operation_ok() {
        // VhtOperation element without Element Id and length
        #[rustfmt::skip]
        let raw_body = [
            231, // vht_cbw(u8)
            232, // center_freq_seg0(u8)
            233, // center_freq_seg1(u8)
            0xff, 0x66, // basic_mcs_nss(VhtMcsNssMap(u16))
        ];
        let vht_op = parse_vht_operation(&raw_body[..]).expect("expected OK from valid frames");
        assert_eq!(231, vht_op.vht_cbw.0);
        assert_eq!(232, vht_op.center_freq_seg0);
        assert_eq!(233, vht_op.center_freq_seg1);
    }

    #[test]
    fn rsnxe_ok() {
        let data = [0b00100001, 0x00, 0x00, 0x40];
        let rsnxe = parse_rsnxe(&data[..]);
        assert_matches!(rsnxe.rsnxe_octet_1, Some(oct1) => {
            assert_eq!(oct1.field_length(), 1);
            assert!(!oct1.protected_twt_operations_support());
            assert!(oct1.sae_hash_to_element());
        });
        assert_eq!(rsnxe.remaining, &[0x00, 0x00, 0x40]);
    }

    #[test]
    fn diffie_hellman_param_ok() {
        let raw_body: Vec<u8> = vec![
            0xaa, 0x00, // group
            0xde, 0xad, 0xbe, 0xef, // public key (truncated)
        ];
        let dh_param =
            parse_diffie_hellman_param(&raw_body[..]).expect("failed to parse DH param ie");
        assert_eq!(dh_param.group, 0xaa);
        assert_eq!(dh_param.public_key, &raw_body[2..]);
    }

    #[test]
    fn parse_wpa_ie_ok() {
        let raw_body: Vec<u8> = vec![
            0x00, 0x50, 0xf2, // MSFT OUI
            0x01, 0x01, 0x00, // WPA IE header
            0x00, 0x50, 0xf2, 0x02, // multicast cipher: AKM
            0x01, 0x00, 0x00, 0x50, 0xf2, 0x02, // 1 unicast cipher: TKIP
            0x01, 0x00, 0x00, 0x50, 0xf2, 0x02, // 1 AKM: PSK
        ];
        let wpa_ie = parse_vendor_ie(&raw_body[..]).expect("failed to parse wpa vendor ie");
        assert_matches!(wpa_ie, VendorIe::MsftLegacyWpa(wpa_body) => {
            parse_wpa_ie(&wpa_body[..]).expect("failed to parse wpa vendor ie")
        });
    }

    #[test]
    fn parse_bad_wpa_ie() {
        let raw_body: Vec<u8> = vec![
            0x00, 0x50, 0xf2, // MSFT OUI
            0x01, 0x01, 0x00, // WPA IE header
            0x00, 0x50, 0xf2, 0x02, // multicast cipher: AKM
                  // truncated
        ];
        // parse_vendor_ie does not validate the actual wpa ie body, so this
        // succeeds.
        let wpa_ie = parse_vendor_ie(&raw_body[..]).expect("failed to parse wpa vendor ie");
        assert_matches!(wpa_ie, VendorIe::MsftLegacyWpa(wpa_body) => {
            parse_wpa_ie(&wpa_body[..]).expect_err("parsed truncated wpa ie")
        });
    }

    #[test]
    fn parse_wmm_info_ie_ok() {
        let raw_body = [
            0x00, 0x50, 0xf2, // MSFT OUI
            0x02, 0x00, 0x01, // WMM Info IE header
            0x80, // QoS Info: U-APSD enabled
        ];
        let wmm_info_ie = parse_vendor_ie(&raw_body[..]).expect("expected Ok");
        assert_matches!(wmm_info_ie, VendorIe::WmmInfo(body) => {
            assert_matches!(parse_wmm_info(&body[..]), Ok(wmm_info) => {
                assert_eq!(wmm_info.0, 0x80);
            })
        });
    }

    #[test]
    fn parse_wmm_info_ie_too_short() {
        let raw_body = [
            0x00, 0x50, 0xf2, // MSFT OUI
            0x02, 0x00, 0x01, // WMM Info IE header
                  // truncated
        ];
        let wmm_info_ie = parse_vendor_ie(&raw_body[..]).expect("expected Ok");
        assert_matches!(wmm_info_ie, VendorIe::WmmInfo(body) => {
            parse_wmm_info(&body[..]).expect_err("parsed truncated WMM info ie")
        });
    }

    #[test]
    fn parse_wmm_param_ie_ok() {
        let raw_body = [
            0x00, 0x50, 0xf2, // MSFT OUI
            0x02, 0x01, 0x01, // WMM Param IE header
            0x80, // QoS Info: U-APSD enabled
            0x00, // reserved
            0x03, 0xa4, 0x00, 0x00, // AC_BE Params - ACM no, AIFSN 3, ECWmin/max 4/10, TXOP 0
            0x27, 0xa4, 0x00, 0x00, // AC_BK Params - ACM no, AIFSN 7, ECWmin/max 4/10, TXOP 0
            0x42, 0x43, 0x5e, 0x00, // AC_VI Params - ACM no, AIFSN 2, ECWmin/max 3/4, TXOP 94
            0x62, 0x32, 0x2f, 0x00, // AC_VO Params - ACM no, AIFSN 2, ECWmin/max 2/3, TXOP 47
        ];
        let wmm_param_ie = parse_vendor_ie(&raw_body[..]).expect("expected Ok");
        assert_matches!(wmm_param_ie, VendorIe::WmmParam(body) => {
            assert_matches!(parse_wmm_param(&body[..]), Ok(wmm_param) => {
                assert_eq!(wmm_param.wmm_info.0, 0x80);
                let ac_be = wmm_param.ac_be_params;
                assert_eq!(ac_be.aci_aifsn.aifsn(), 3);
                assert_eq!(ac_be.aci_aifsn.acm(), false);
                assert_eq!(ac_be.aci_aifsn.aci(), 0);
                assert_eq!(ac_be.ecw_min_max.ecw_min(), 4);
                assert_eq!(ac_be.ecw_min_max.ecw_max(), 10);
                assert_eq!({ ac_be.txop_limit }, 0);

                let ac_bk = wmm_param.ac_bk_params;
                assert_eq!(ac_bk.aci_aifsn.aifsn(), 7);
                assert_eq!(ac_bk.aci_aifsn.acm(), false);
                assert_eq!(ac_bk.aci_aifsn.aci(), 1);
                assert_eq!(ac_bk.ecw_min_max.ecw_min(), 4);
                assert_eq!(ac_bk.ecw_min_max.ecw_max(), 10);
                assert_eq!({ ac_bk.txop_limit }, 0);

                let ac_vi = wmm_param.ac_vi_params;
                assert_eq!(ac_vi.aci_aifsn.aifsn(), 2);
                assert_eq!(ac_vi.aci_aifsn.acm(), false);
                assert_eq!(ac_vi.aci_aifsn.aci(), 2);
                assert_eq!(ac_vi.ecw_min_max.ecw_min(), 3);
                assert_eq!(ac_vi.ecw_min_max.ecw_max(), 4);
                assert_eq!({ ac_vi.txop_limit }, 94);

                let ac_vo = wmm_param.ac_vo_params;
                assert_eq!(ac_vo.aci_aifsn.aifsn(), 2);
                assert_eq!(ac_vo.aci_aifsn.acm(), false);
                assert_eq!(ac_vo.aci_aifsn.aci(), 3);
                assert_eq!(ac_vo.ecw_min_max.ecw_min(), 2);
                assert_eq!(ac_vo.ecw_min_max.ecw_max(), 3);
                assert_eq!({ ac_vo.txop_limit }, 47);
            });
        });
    }

    #[test]
    fn parse_wmm_param_ie_too_short() {
        let raw_body = [
            0x00, 0x50, 0xf2, // MSFT OUI
            0x02, 0x01, 0x01, // WMM Param IE header
            0x80, // QoS Info: U-APSD enabled
            0x00, // reserved
                  // truncated
        ];
        let wmm_param_ie = parse_vendor_ie(&raw_body[..]).expect("expected Ok");
        assert_matches!(wmm_param_ie, VendorIe::WmmParam(body) => {
            parse_wmm_param(&body[..]).expect_err("parsed truncated WMM param ie")
        });
    }

    #[test]
    fn parse_unknown_msft_ie() {
        let raw_body: Vec<u8> = vec![
            0x00, 0x50, 0xf2, // MSFT OUI
            0xff, 0x01, 0x00, // header with unknown vendor specific IE type
            0x00, 0x50, 0xf2, 0x02, // multicast cipher: AKM
            0x01, 0x00, 0x00, 0x50, 0xf2, 0x02, // 1 unicast cipher: TKIP
            0x01, 0x00, 0x00, 0x50, 0xf2, 0x02, // 1 AKM: PSK
        ];
        let ie = parse_vendor_ie(&raw_body[..]).expect("failed to parse ie");
        assert_matches!(ie, VendorIe::Unknown { .. });
    }

    #[test]
    fn parse_unknown_vendor_ie() {
        let raw_body: Vec<u8> = vec![0x00, 0x12, 0x34]; // Made up OUI
        let ie = parse_vendor_ie(&raw_body[..]).expect("failed to parse wpa vendor ie");
        assert_matches!(ie, VendorIe::Unknown { .. });
    }

    #[test]
    fn to_and_from_fidl_ht_cap() {
        fidl_ieee80211::HtCapabilities {
            bytes: fake_ht_capabilities().as_bytes().try_into().expect("HT Cap to FIDL"),
        };
        let fidl =
            fidl_ieee80211::HtCapabilities { bytes: [0; fidl_ieee80211::HT_CAP_LEN as usize] };
        assert!(parse_ht_capabilities(&fidl.bytes[..]).is_ok());
    }

    #[test]
    fn to_and_from_fidl_vht_cap() {
        fidl_ieee80211::VhtCapabilities {
            bytes: fake_vht_capabilities().as_bytes().try_into().expect("VHT Cap to FIDL"),
        };
        let fidl =
            fidl_ieee80211::VhtCapabilities { bytes: [0; fidl_ieee80211::VHT_CAP_LEN as usize] };
        assert!(parse_vht_capabilities(&fidl.bytes[..]).is_ok());
    }

    #[test]
    fn to_and_from_fidl_ht_op() {
        fidl_ieee80211::HtOperation {
            bytes: fake_ht_operation().as_bytes().try_into().expect("HT Op to FIDL"),
        };
        let fidl = fidl_ieee80211::HtOperation { bytes: [0; fidl_ieee80211::HT_OP_LEN as usize] };
        assert!(parse_ht_operation(&fidl.bytes[..]).is_ok());
    }

    #[test]
    fn to_and_from_fidl_vht_op() {
        fidl_ieee80211::VhtOperation {
            bytes: fake_vht_operation().as_bytes().try_into().expect("VHT Op to FIDL"),
        };
        let fidl = fidl_ieee80211::VhtOperation { bytes: [0; fidl_ieee80211::VHT_OP_LEN as usize] };
        assert!(parse_vht_operation(&fidl.bytes[..]).is_ok());
    }
}
