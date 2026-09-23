// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use log::debug;
use objects::ObexObjectError as Error;

pub mod event_report;
pub mod messages_listing;

/// The ISO 8601 time format used in the Time Header packet.
/// The format is YYYYMMDDTHHMMSS where "T" delimits the date from the time.
// TODO(b/348051261): support UTC timestamp.
pub(crate) const ISO_8601_TIME_FORMAT: &str = "%Y%m%dT%H%M%S";

/// Some string values have byte data length limit.
/// We truncate the strings to fit that limit if necessary.
pub(crate) fn truncate_string(value: &String, max_len: usize) -> String {
    let mut v = value.clone();
    if v.len() <= max_len {
        return v;
    }

    let l = v.floor_char_boundary(max_len);
    v.truncate(l);

    debug!("truncated string value from length {} to {}", value.len(), v.len());
    v
}

// Converts the "yes" / "no" values to corresponding boolean.
pub(crate) fn str_to_bool(val: &str) -> Result<bool, Error> {
    match val {
        "yes" => Ok(true),
        "no" => Ok(false),
        val => Err(Error::invalid_data(val)),
    }
}

pub(crate) fn bool_to_string(val: bool) -> String {
    if val { "yes".to_string() } else { "no".to_string() }
}
