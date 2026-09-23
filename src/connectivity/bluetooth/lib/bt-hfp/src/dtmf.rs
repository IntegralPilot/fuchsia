// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_bluetooth_hfp as fidl;

/// Represents a single dual-tone multi-frequency signaling code.
/// This is a native representation of the FIDL enum `fuchsia.bluetooth.hfp.DtmfCode`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Code {
    One,
    Two,
    Three,
    Four,
    Five,
    Six,
    Seven,
    Eight,
    Nine,
    NumberSign,
    Zero,
    Asterisk,
    A,
    B,
    C,
    D,
}

impl TryFrom<&str> for Code {
    type Error = ();
    fn try_from(x: &str) -> Result<Self, Self::Error> {
        match x {
            "1" => Ok(Self::One),
            "2" => Ok(Self::Two),
            "3" => Ok(Self::Three),
            "4" => Ok(Self::Four),
            "5" => Ok(Self::Five),
            "6" => Ok(Self::Six),
            "7" => Ok(Self::Seven),
            "8" => Ok(Self::Eight),
            "9" => Ok(Self::Nine),
            "#" => Ok(Self::NumberSign),
            "0" => Ok(Self::Zero),
            "*" => Ok(Self::Asterisk),
            "A" => Ok(Self::A),
            "B" => Ok(Self::B),
            "C" => Ok(Self::C),
            "D" => Ok(Self::D),
            _ => Err(()),
        }
    }
}

impl From<Code> for String {
    fn from(x: Code) -> Self {
        match x {
            Code::One => String::from("1"),
            Code::Two => String::from("2"),
            Code::Three => String::from("3"),
            Code::Four => String::from("4"),
            Code::Five => String::from("5"),
            Code::Six => String::from("6"),
            Code::Seven => String::from("7"),
            Code::Eight => String::from("8"),
            Code::Nine => String::from("9"),
            Code::NumberSign => String::from("#"),
            Code::Zero => String::from("0"),
            Code::Asterisk => String::from("*"),
            Code::A => String::from("A"),
            Code::B => String::from("B"),
            Code::C => String::from("C"),
            Code::D => String::from("D"),
        }
    }
}

impl From<fidl::DtmfCode> for Code {
    fn from(x: fidl::DtmfCode) -> Self {
        match x {
            fidl::DtmfCode::One => Self::One,
            fidl::DtmfCode::Two => Self::Two,
            fidl::DtmfCode::Three => Self::Three,
            fidl::DtmfCode::Four => Self::Four,
            fidl::DtmfCode::Five => Self::Five,
            fidl::DtmfCode::Six => Self::Six,
            fidl::DtmfCode::Seven => Self::Seven,
            fidl::DtmfCode::Eight => Self::Eight,
            fidl::DtmfCode::Nine => Self::Nine,
            fidl::DtmfCode::NumberSign => Self::NumberSign,
            fidl::DtmfCode::Zero => Self::Zero,
            fidl::DtmfCode::Asterisk => Self::Asterisk,
            fidl::DtmfCode::A => Self::A,
            fidl::DtmfCode::B => Self::B,
            fidl::DtmfCode::C => Self::C,
            fidl::DtmfCode::D => Self::D,
        }
    }
}

impl From<Code> for fidl::DtmfCode {
    fn from(x: Code) -> Self {
        match x {
            Code::One => Self::One,
            Code::Two => Self::Two,
            Code::Three => Self::Three,
            Code::Four => Self::Four,
            Code::Five => Self::Five,
            Code::Six => Self::Six,
            Code::Seven => Self::Seven,
            Code::Eight => Self::Eight,
            Code::Nine => Self::Nine,
            Code::NumberSign => Self::NumberSign,
            Code::Zero => Self::Zero,
            Code::Asterisk => Self::Asterisk,
            Code::A => Self::A,
            Code::B => Self::B,
            Code::C => Self::C,
            Code::D => Self::D,
        }
    }
}
