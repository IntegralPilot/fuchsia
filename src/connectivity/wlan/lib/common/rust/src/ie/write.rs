// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::constants::*;
use super::fields::*;
use super::id::Id;
use super::rsn::rsne;
use super::{wpa, wsc};
use crate::append::{Append, BufferTooSmall};
use crate::error::FrameWriteError;
use crate::organization::Oui;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use zerocopy::IntoBytes;

macro_rules! validate {
    ( $condition:expr, $fmt:expr $(, $args:expr)* $(,)? ) => {
        if !$condition {
            return Err($crate::error::FrameWriteError::InvalidData(format!($fmt, $($args,)*)));
        }
    };
}

macro_rules! write_ie {
    ( $buf:expr, $id:expr, $( $part:expr ),* ) => {
        {
            let body_len = 0 $( + ::std::mem::size_of_val($part) )*;
            validate!(body_len <= crate::ie::IE_MAX_LEN,
                "Element body length {} exceeds max of 255", body_len);
            if !$buf.can_append(2 + body_len) {
                return Err(FrameWriteError::BufferTooSmall);
            }
            $buf.append_value(&$id)
                    .expect("expected enough room in the buffer for element id");
            $buf.append_byte(body_len as u8)
                    .expect("expected enough room in the buffer for element length");
            $(
                $buf.append_value($part)
                        .expect("expected enough room in the buffer for element fields");
            )*
            Ok(())
        }
    }
}

pub fn write_ssid<B: Append>(buf: &mut B, ssid: &[u8]) -> Result<(), FrameWriteError> {
    validate!(
        ssid.len() <= (fidl_ieee80211::MAX_SSID_BYTE_LEN as usize),
        "SSID is too long (max: {} bytes, got: {})",
        fidl_ieee80211::MAX_SSID_BYTE_LEN,
        ssid.len()
    );
    write_ie!(buf, Id::SSID, ssid)
}

pub fn write_supported_rates<B: Append>(buf: &mut B, rates: &[u8]) -> Result<(), FrameWriteError> {
    validate!(!rates.is_empty(), "List of Supported Rates is empty");
    validate!(
        rates.len() <= SUPPORTED_RATES_MAX_LEN,
        "Too many Supported Rates (max {}, got {})",
        SUPPORTED_RATES_MAX_LEN,
        rates.len()
    );
    write_ie!(buf, Id::SUPPORTED_RATES, rates)
}

pub fn write_extended_supported_rates<B: Append>(
    buf: &mut B,
    rates: &[u8],
) -> Result<(), FrameWriteError> {
    validate!(!rates.is_empty(), "List of Extended Supported Rates is empty");
    validate!(
        rates.len() <= EXTENDED_SUPPORTED_RATES_MAX_LEN,
        "Too many Extended Supported Rates (max {}, got {})",
        EXTENDED_SUPPORTED_RATES_MAX_LEN,
        rates.len()
    );
    write_ie!(buf, Id::EXTENDED_SUPPORTED_RATES, rates)
}

pub fn write_rsne<B: Append>(buf: &mut B, rsne: &rsne::Rsne) -> Result<(), FrameWriteError> {
    rsne.write_into(buf).map_err(|e| e.into())
}

pub fn write_ht_capabilities<B: Append>(
    buf: &mut B,
    ht_cap: &HtCapabilities,
) -> Result<(), FrameWriteError> {
    write_ie!(buf, Id::HT_CAPABILITIES, ht_cap.as_bytes())
}

pub fn write_ht_operation<B: Append>(
    buf: &mut B,
    ht_op: &HtOperation,
) -> Result<(), FrameWriteError> {
    write_ie!(buf, Id::HT_OPERATION, ht_op.as_bytes())
}

pub fn write_dsss_param_set<B: Append>(
    buf: &mut B,
    dsss: &DsssParamSet,
) -> Result<(), FrameWriteError> {
    write_ie!(buf, Id::DSSS_PARAM_SET, dsss)
}

pub fn write_tim<B: Append>(
    buf: &mut B,
    header: &TimHeader,
    bitmap: &[u8],
) -> Result<(), FrameWriteError> {
    validate!(!bitmap.is_empty(), "Partial virtual bitmap in TIM is empty");
    validate!(
        bitmap.len() <= TIM_MAX_BITMAP_LEN,
        "Partial virtual bitmap in TIM too large (max: {} bytes, got {})",
        TIM_MAX_BITMAP_LEN,
        bitmap.len()
    );
    write_ie!(buf, Id::TIM, header, bitmap)
}

pub fn write_bss_max_idle_period<B: Append>(
    buf: &mut B,
    bss_max_idle_period: &BssMaxIdlePeriod,
) -> Result<(), FrameWriteError> {
    write_ie!(buf, Id::BSS_MAX_IDLE_PERIOD, bss_max_idle_period)
}

pub fn write_vht_capabilities<B: Append>(
    buf: &mut B,
    vht_cap: &VhtCapabilities,
) -> Result<(), FrameWriteError> {
    write_ie!(buf, Id::VHT_CAPABILITIES, vht_cap.as_bytes())
}

pub fn write_vht_operation<B: Append>(
    buf: &mut B,
    vht_op: &VhtOperation,
) -> Result<(), FrameWriteError> {
    write_ie!(buf, Id::VHT_OPERATION, vht_op.as_bytes())
}

pub fn write_rsnxe<B: Append>(buf: &mut B, octet_1: RsnxeOctet1) -> Result<(), FrameWriteError> {
    write_ie!(buf, Id::RSNXE, &[octet_1.0])
}

/// Writes the entire WPA1 IE into the given buffer, including the vendor IE header.
pub fn write_wpa1_ie<B: Append>(buf: &mut B, wpa_ie: &wpa::WpaIe) -> Result<(), BufferTooSmall> {
    let len = std::mem::size_of::<Oui>() + 1 + wpa_ie.len();
    if !buf.can_append(len + 2) {
        return Err(BufferTooSmall);
    }
    buf.append_value(&Id::VENDOR_SPECIFIC)?;
    buf.append_byte(len as u8)?;
    buf.append_value(&Oui::MSFT)?;
    buf.append_byte(wpa::VENDOR_SPECIFIC_TYPE)?;
    wpa_ie.write_into(buf)
}

/// Writes the entire WSC IE into the given buffer, including the vendor IE header.
pub fn write_wsc_ie<B: Append>(buf: &mut B, wsc: &[u8]) -> Result<(), BufferTooSmall> {
    let len = std::mem::size_of::<Oui>() + 1 + wsc.len();
    if !buf.can_append(len + 2) {
        return Err(BufferTooSmall);
    }
    buf.append_value(&Id::VENDOR_SPECIFIC)?;
    buf.append_byte(len as u8)?;
    buf.append_value(&Oui::MSFT)?;
    buf.append_byte(wsc::VENDOR_SPECIFIC_TYPE)?;
    buf.append_bytes(wsc)
}

pub fn write_wmm_param<B: Append>(buf: &mut B, wmm_param: &WmmParam) -> Result<(), BufferTooSmall> {
    let len = std::mem::size_of::<Oui>() + 3 + ::std::mem::size_of_val(wmm_param);
    if !buf.can_append(len + 2) {
        return Err(BufferTooSmall);
    }
    buf.append_value(&Id::VENDOR_SPECIFIC)?;
    buf.append_byte(len as u8)?;
    buf.append_value(&Oui::MSFT)?;
    buf.append_byte(WMM_OUI_TYPE)?;
    buf.append_byte(WMM_PARAM_OUI_SUBTYPE)?;
    // The version byte is 0x01 for both WMM Information and Parameter elements
    // as of WFA WMM v1.2.0.
    buf.append_byte(0x1)?;
    buf.append_bytes(wmm_param.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer_writer::BufferWriter;
    use crate::ie::rsn::{akm, cipher};

    #[test]
    fn write_ie_body_too_long() {
        let mut buf = vec![];
        let mut f = || write_ie!(buf, Id::SSID, &[0u8; 256][..]);
        assert_eq!(
            Err(FrameWriteError::InvalidData(format!(
                "Element body length 256 exceeds max of 255"
            ))),
            f()
        );
    }

    #[test]
    fn write_ie_buffer_too_small() {
        let mut buf = [7u8; 5];
        let mut writer = BufferWriter::new(&mut buf[..]);
        let mut f = || write_ie!(writer, Id::SSID, &[1u8, 2, 3, 4][..]);
        assert_eq!(Err(FrameWriteError::BufferTooSmall), f());
        // Expect the buffer to be left untouched
        assert_eq!(&[7, 7, 7, 7, 7], &buf[..]);
    }

    #[test]
    fn write_ie_buffer_exactly_long_enough() {
        let mut buf = [0u8; 5];
        let mut writer = BufferWriter::new(&mut buf[..]);
        let mut f = || write_ie!(writer, Id::SSID, &[1u8, 2, 3][..]);
        assert_eq!(Ok(()), f());
        assert_eq!(&[0, 3, 1, 2, 3], &buf[..]);
    }

    #[test]
    fn ssid_ok() {
        let mut buf = vec![];
        write_ssid(&mut buf, &[1, 2, 3]).expect("expected Ok");
        assert_eq!(&[0, 3, 1, 2, 3], &buf[..]);
    }

    #[test]
    fn ssid_ok_empty() {
        let mut buf = vec![];
        write_ssid(&mut buf, &[]).expect("expected Ok");
        assert_eq!(&[0, 0], &buf[..]);
    }

    #[test]
    fn ssid_too_long() {
        let mut buf = vec![];
        assert_eq!(
            Err(FrameWriteError::InvalidData(format!("SSID is too long (max: 32 bytes, got: 33)"))),
            write_ssid(&mut buf, &[0u8; 33])
        );
    }

    #[test]
    fn supported_rates_ok() {
        let mut buf = vec![];
        write_supported_rates(&mut buf, &[1, 2, 3, 4, 5, 6, 7, 8]).expect("expected Ok");
        assert_eq!(&[1, 8, 1, 2, 3, 4, 5, 6, 7, 8], &buf[..]);
    }

    #[test]
    fn supported_rates_empty() {
        let mut buf = vec![];
        assert_eq!(
            Err(FrameWriteError::InvalidData(format!("List of Supported Rates is empty"))),
            write_supported_rates(&mut buf, &[])
        );
    }

    #[test]
    fn supported_rates_too_long() {
        let mut buf = vec![];
        assert_eq!(
            Err(FrameWriteError::InvalidData(format!("Too many Supported Rates (max 8, got 9)"))),
            write_supported_rates(&mut buf, &[0u8; 9])
        );
    }

    #[test]
    fn ext_supported_rates_ok() {
        let mut buf = vec![];
        write_extended_supported_rates(&mut buf, &[1, 2, 3, 4, 5, 6, 7, 8]).expect("expected Ok");
        assert_eq!(&[50, 8, 1, 2, 3, 4, 5, 6, 7, 8], &buf[..]);
    }

    #[test]
    fn ext_supported_rates_empty() {
        let mut buf = vec![];
        assert_eq!(
            Err(FrameWriteError::InvalidData(format!("List of Extended Supported Rates is empty"))),
            write_extended_supported_rates(&mut buf, &[])
        );
    }

    #[test]
    fn dsss_param_set() {
        let mut buf = vec![];
        write_dsss_param_set(&mut buf, &DsssParamSet { current_channel: 6 }).expect("expected Ok");
        assert_eq!(&[3, 1, 6], &buf[..]);
    }

    #[test]
    fn tim_ok() {
        let mut buf = vec![];
        write_tim(
            &mut buf,
            &TimHeader { dtim_count: 1, dtim_period: 2, bmp_ctrl: BitmapControl(3) },
            &[4, 5, 6],
        )
        .expect("expected Ok");
        assert_eq!(&[5, 6, 1, 2, 3, 4, 5, 6], &buf[..]);
    }

    #[test]
    fn tim_empty_bitmap() {
        let mut buf = vec![];
        assert_eq!(
            Err(FrameWriteError::InvalidData(format!("Partial virtual bitmap in TIM is empty"))),
            write_tim(
                &mut buf,
                &TimHeader { dtim_count: 1, dtim_period: 2, bmp_ctrl: BitmapControl(3) },
                &[]
            )
        );
    }

    #[test]
    fn tim_bitmap_too_long() {
        let mut buf = vec![];
        assert_eq!(
            Err(FrameWriteError::InvalidData(format!(
                "Partial virtual bitmap in TIM too large (max: 251 bytes, got 252)"
            ))),
            write_tim(
                &mut buf,
                &TimHeader { dtim_count: 1, dtim_period: 2, bmp_ctrl: BitmapControl(3) },
                &[0u8; 252][..]
            )
        );
    }

    #[test]
    fn test_write_wpa1_ie() {
        let wpa_ie = wpa::WpaIe {
            multicast_cipher: cipher::Cipher { oui: Oui::MSFT, suite_type: cipher::TKIP },
            unicast_cipher_list: vec![cipher::Cipher { oui: Oui::MSFT, suite_type: cipher::TKIP }],
            akm_list: vec![akm::Akm { oui: Oui::MSFT, suite_type: akm::PSK }],
        };
        let expected: Vec<u8> = vec![
            0xdd, 0x16, // Vendor IE header
            0x00, 0x50, 0xf2, // MSFT OUI
            0x01, 0x01, 0x00, // WPA IE header
            0x00, 0x50, 0xf2, 0x02, // multicast cipher: AKM
            0x01, 0x00, 0x00, 0x50, 0xf2, 0x02, // 1 unicast cipher: TKIP
            0x01, 0x00, 0x00, 0x50, 0xf2, 0x02, // 1 AKM: PSK
        ];
        let mut buf = vec![];
        write_wpa1_ie(&mut buf, &wpa_ie).expect("WPA1 write to a Vec should never fail");
        assert_eq!(&expected[..], &buf[..]);
    }

    #[test]
    fn test_write_wpa1_ie_buffer_too_small() {
        let wpa_ie = wpa::WpaIe {
            multicast_cipher: cipher::Cipher { oui: Oui::MSFT, suite_type: cipher::TKIP },
            unicast_cipher_list: vec![cipher::Cipher { oui: Oui::MSFT, suite_type: cipher::TKIP }],
            akm_list: vec![akm::Akm { oui: Oui::MSFT, suite_type: akm::PSK }],
        };

        let mut buf = [0u8; 10];
        let mut writer = BufferWriter::new(&mut buf[..]);
        write_wpa1_ie(&mut writer, &wpa_ie).expect_err("WPA1 write to short buf should fail");
        // The buffer is not long enough, so no bytes should be written.
        assert_eq!(writer.into_written().len(), 0);
    }

    #[test]
    fn test_write_wmm_param() {
        let wmm_param = WmmParam {
            wmm_info: WmmInfo(0).with_ap_wmm_info(ApWmmInfo(0).with_uapsd(true)),
            _reserved: 0,
            ac_be_params: WmmAcParams {
                aci_aifsn: WmmAciAifsn(0).with_aifsn(3).with_aci(0),
                ecw_min_max: EcwMinMax(0).with_ecw_min(4).with_ecw_max(10),
                txop_limit: 0,
            },
            ac_bk_params: WmmAcParams {
                aci_aifsn: WmmAciAifsn(0).with_aifsn(7).with_aci(1),
                ecw_min_max: EcwMinMax(0).with_ecw_min(4).with_ecw_max(10),
                txop_limit: 0,
            },
            ac_vi_params: WmmAcParams {
                aci_aifsn: WmmAciAifsn(0).with_aifsn(2).with_aci(2),
                ecw_min_max: EcwMinMax(0).with_ecw_min(3).with_ecw_max(4),
                txop_limit: 94,
            },
            ac_vo_params: WmmAcParams {
                aci_aifsn: WmmAciAifsn(0).with_aifsn(2).with_aci(3),
                ecw_min_max: EcwMinMax(0).with_ecw_min(2).with_ecw_max(3),
                txop_limit: 47,
            },
        };
        let expected: Vec<u8> = vec![
            // WMM parameters
            0xdd, 0x18, // Vendor IE header
            0x00, 0x50, 0xf2, // MSFT OUI
            0x02, 0x01, // WMM Type and WMM Parameter Subtype
            0x01, // Version 1
            0x80, // U-APSD enabled
            0x00, // reserved
            0x03, 0xa4, 0x00, 0x00, // AC_BE parameters
            0x27, 0xa4, 0x00, 0x00, // AC_BK parameters
            0x42, 0x43, 0x5e, 0x00, // AC_VI parameters
            0x62, 0x32, 0x2f, 0x00, // AC_VO parameters
        ];
        let mut buf = vec![];
        write_wmm_param(&mut buf, &wmm_param).expect("WmmParam write to a Vec should never fail");
        assert_eq!(&expected[..], &buf[..]);
    }

    #[test]
    fn ht_capabilities_ok() {
        let mut buf = vec![];
        let ht_cap = crate::ie::fake_ies::fake_ht_capabilities();
        write_ht_capabilities(&mut buf, &ht_cap).expect("writing ht cap");
        assert_eq!(
            &buf[..],
            &[
                45, 26, // HT Cap id and length
                254, 1, 0, 255, 0, 0, 0, 1, // byte 0-7
                0, 0, 0, 0, 0, 0, 0, 1, // byte 8-15
                0, 0, 0, 0, 0, 0, 0, 0, // byte 16-23
                0, 0, // byte 24-25
            ]
        );
    }

    #[test]
    fn ht_operation_ok() {
        let mut buf = vec![];
        let ht_op = crate::ie::fake_ies::fake_ht_operation();
        write_ht_operation(&mut buf, &ht_op).expect("writing ht op");
        assert_eq!(
            &buf[..],
            &[
                61, 22, // HT Op id and length
                36, 5, 20, 0, 0, 0, 255, 0, // byte 0-7
                0, 0, 1, 0, 0, 0, 0, 0, // byte 8-15
                0, 0, 1, 0, 0, 0, // byte 16-21
            ]
        );
    }

    #[test]
    fn vht_capabilities_ok() {
        let mut buf = vec![];
        let vht_cap = crate::ie::fake_ies::fake_vht_capabilities();
        write_vht_capabilities(&mut buf, &vht_cap).expect("writing vht cap");
        assert_eq!(
            &buf[..],
            &[
                191, 12, // VHT Cap id and length
                177, 2, 0, 177, 3, 2, 99, 67, // byte 0-7
                3, 2, 99, 3, // byte 8-11
            ]
        );
    }

    #[test]
    fn vht_operation_ok() {
        let mut buf = vec![];
        let vht_op = crate::ie::fake_ies::fake_vht_operation();
        write_vht_operation(&mut buf, &vht_op).expect("writing vht op");
        assert_eq!(
            &buf[..],
            &[
                192, 5, // VHT Op id and length
                1, 42, 0, 27, 27, // byte 0-4
            ]
        );
    }

    #[test]
    fn rsne_ok() {
        let mut buf = vec![];
        let rsne = rsne::from_bytes(&crate::test_utils::fake_frames::fake_wpa2_rsne()[..])
            .expect("creating rsne")
            .1;
        write_rsne(&mut buf, &rsne).expect("writing rsne");
        assert_eq!(
            &buf[..],
            &[
                48, 18, // RSNE id and length
                1, 0, 0, 15, 172, 4, 1, 0, // byte 0-7
                0, 15, 172, 4, 1, 0, 0, 15, // byte 8-15
                172, 2, // byte 16-17
            ]
        );
    }

    #[test]
    fn bss_max_idle_period_ok() {
        let mut buf = vec![];
        write_bss_max_idle_period(
            &mut buf,
            &BssMaxIdlePeriod {
                max_idle_period: 99,
                idle_options: IdleOptions(0).with_protected_keep_alive_required(true),
            },
        )
        .expect("writing bss max idle period");
        assert_eq!(&buf[..], &[90, 3, 99, 0, 1]);
    }
}
