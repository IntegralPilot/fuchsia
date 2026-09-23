// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::uapi;

// Note: The uapi::BXXX constants (e.g., B9600) do not have the same values as the baud rates
// they represent (e.g., 9600). For example, B9600 is 13. Therefore, we need these conversion
// functions to map between the numerical baud rates (used in c_ispeed/c_ospeed) and the
// c_cflag bitmasks.

pub fn into_termios2(value: uapi::termio) -> uapi::termios2 {
    let mut cc = [0; 19];
    cc[0..8].copy_from_slice(&value.c_cc[0..8]);
    let c_cflag = value.c_cflag as uapi::tcflag_t;
    let speed = cbaud_to_speed(c_cflag & uapi::CBAUD);
    uapi::termios2 {
        c_iflag: value.c_iflag as uapi::tcflag_t,
        c_oflag: value.c_oflag as uapi::tcflag_t,
        c_cflag,
        c_lflag: value.c_lflag as uapi::tcflag_t,
        c_line: value.c_line as uapi::cc_t,
        c_cc: cc,
        c_ispeed: speed,
        c_ospeed: speed,
    }
}

pub fn into_termio(value: &uapi::termios2) -> uapi::termio {
    let mut cc = [0; 8];
    cc.copy_from_slice(&value.c_cc[0..8]);
    uapi::termio {
        c_iflag: value.c_iflag as u16,
        c_oflag: value.c_oflag as u16,
        c_cflag: value.c_cflag as u16,
        c_lflag: value.c_lflag as u16,
        c_line: value.c_line,
        c_cc: cc,
        ..Default::default()
    }
}

pub fn termios2_from_termios(value: &uapi::termios) -> uapi::termios2 {
    let cbaud = value.c_cflag & uapi::CBAUD;
    let speed = cbaud_to_speed(cbaud);
    uapi::termios2 {
        c_iflag: value.c_iflag,
        c_oflag: value.c_oflag,
        c_cflag: value.c_cflag,
        c_lflag: value.c_lflag,
        c_line: value.c_line,
        c_cc: value.c_cc,
        c_ispeed: speed,
        c_ospeed: speed,
    }
}

pub fn termios_from_termios2(value: &uapi::termios2) -> uapi::termios {
    let mut c_cflag = value.c_cflag;
    if let Some(cbaud) = speed_to_cbaud(value.c_ospeed) {
        c_cflag = (c_cflag & !uapi::CBAUD) | cbaud;
    }
    uapi::termios {
        c_iflag: value.c_iflag,
        c_oflag: value.c_oflag,
        c_cflag,
        c_lflag: value.c_lflag,
        c_line: value.c_line,
        c_cc: value.c_cc,
    }
}

pub fn speed_to_cbaud(speed: u32) -> Option<uapi::tcflag_t> {
    match speed {
        0 => Some(uapi::B0),
        50 => Some(uapi::B50),
        75 => Some(uapi::B75),
        110 => Some(uapi::B110),
        134 => Some(uapi::B134),
        150 => Some(uapi::B150),
        200 => Some(uapi::B200),
        300 => Some(uapi::B300),
        600 => Some(uapi::B600),
        1200 => Some(uapi::B1200),
        1800 => Some(uapi::B1800),
        2400 => Some(uapi::B2400),
        4800 => Some(uapi::B4800),
        9600 => Some(uapi::B9600),
        19200 => Some(uapi::B19200),
        38400 => Some(uapi::B38400),
        57600 => Some(uapi::B57600),
        115200 => Some(uapi::B115200),
        230400 => Some(uapi::B230400),
        460800 => Some(uapi::B460800),
        500000 => Some(uapi::B500000),
        576000 => Some(uapi::B576000),
        921600 => Some(uapi::B921600),
        1000000 => Some(uapi::B1000000),
        1152000 => Some(uapi::B1152000),
        1500000 => Some(uapi::B1500000),
        2000000 => Some(uapi::B2000000),
        2500000 => Some(uapi::B2500000),
        3000000 => Some(uapi::B3000000),
        3500000 => Some(uapi::B3500000),
        4000000 => Some(uapi::B4000000),
        _ => Some(uapi::BOTHER),
    }
}

pub fn cbaud_to_speed(cbaud: uapi::tcflag_t) -> u32 {
    match cbaud {
        uapi::B0 => 0,
        uapi::B50 => 50,
        uapi::B75 => 75,
        uapi::B110 => 110,
        uapi::B134 => 134,
        uapi::B150 => 150,
        uapi::B200 => 200,
        uapi::B300 => 300,
        uapi::B600 => 600,
        uapi::B1200 => 1200,
        uapi::B1800 => 1800,
        uapi::B2400 => 2400,
        uapi::B4800 => 4800,
        uapi::B9600 => 9600,
        uapi::B19200 => 19200,
        uapi::B38400 => 38400,
        uapi::B57600 => 57600,
        uapi::B115200 => 115200,
        uapi::B230400 => 230400,
        uapi::B460800 => 460800,
        uapi::B500000 => 500000,
        uapi::B576000 => 576000,
        uapi::B921600 => 921600,
        uapi::B1000000 => 1000000,
        uapi::B1152000 => 1152000,
        uapi::B1500000 => 1500000,
        uapi::B2000000 => 2000000,
        uapi::B2500000 => 2500000,
        uapi::B3000000 => 3000000,
        uapi::B3500000 => 3500000,
        uapi::B4000000 => 4000000,
        _ => 0,
    }
}
