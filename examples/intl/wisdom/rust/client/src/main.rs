// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Intl wisdom client
//!
//! This is an example implementation of the international wisdom client program.
//! It shows how one can use the available ICU bindings for rust to build a
//! program that makes use of the Unicode support within ICU.

use anyhow::{Context as _, Error};
use clap::Parser;
use fidl_fuchsia_examples_intl_wisdom as fwisdom;

use fuchsia_component::client::connect_to_protocol;
use rust_icu_sys as usys;
use rust_icu_udat as udat;
use rust_icu_uloc as uloc;
use rust_icu_ustring as ustring;

pub(crate) mod wisdom_client_impl;

#[derive(Parser, Debug)]
#[command(name = "intl_wisdom_client_rust")]
struct Opt {
    #[arg(
        long = "timestamp",
        help = "the date-time to request the timestamp for",
        default_value = "2018-10-30T15:30:00-07:00"
    )]
    timestamp: String,

    #[arg(
        long = "timezone",
        help = "the time zone to request the printout for",
        default_value = "Etc/Unknown"
    )]
    timezone: String,
}

// Parses a textual timestamp like "2018-10-30T15:30:00-07:00" into a date-time point.
fn parse_timestamp(timestamp: &str, timezone: &str) -> Result<usys::UDate, Error> {
    let pattern = ustring::UChar::try_from("yyyy-MM-dd'T'HH:mm:ssXX")?;
    let loc = uloc::ULoc::try_from("en-US")?;

    // COMPATIBILITY: This doesn't use the system time zone when converting the timestamp, but
    // rather the passed-in timezone.
    let tz_id = ustring::UChar::try_from(timezone)?;
    let fmt = udat::UDateFormat::new_with_pattern(&loc, &tz_id, &pattern)?;
    fmt.parse(timestamp).map_err(|e| e.into())
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    // Force the loading of ICU data at the beginning of the program.
    let icu_data_loader = icu_data::Loader::new()?;

    // Launch the server and connect to the wisdom service.
    let opts: Opt = Opt::parse();

    let wisdom = connect_to_protocol::<fwisdom::IntlWisdomServer_Marker>()
        .context("failed to connect to intl wisdom service")?;

    let timestamp_ms = parse_timestamp(&opts.timestamp, &opts.timezone)?;

    let client = wisdom_client_impl::Client::new(icu_data_loader.clone(), wisdom);
    let res = client.ask_for_wisdom(timestamp_ms, &opts.timezone).await?;
    println!("Response:\n{}", res);
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn basic() {
        assert!(true);
    }
}
