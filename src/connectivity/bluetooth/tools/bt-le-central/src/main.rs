// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use bt_gatt::Central;
use bt_gatt_fuchsia::FuchsiaTypes;
use fidl_fuchsia_bluetooth_le::CentralMarker;
use fuchsia_async as fasync;
use fuchsia_bluetooth::assigned_numbers;
use fuchsia_bluetooth::types::{PeerId, Uuid};
use getopts::Options;
use std::str::FromStr;

use crate::central::{CentralState, CentralStatePtr};

mod central;
mod gatt;

async fn do_scan<T: bt_gatt::GattTypes>(
    appname: &String,
    args: &[String],
    state: CentralStatePtr<T>,
) -> Result<(), Error>
where
    <T as bt_gatt::GattTypes>::Client: Clone,
{
    let mut opts = Options::new();

    let _ = opts.optflag("h", "help", "");

    // Options for scan/connection behavior.
    let _ = opts.optopt(
        "s",
        "scan-count",
        "number of scan results to return before scanning is stopped",
        "SCAN_COUNT",
    );
    let _ = opts.optflag("c", "connect", "connect to the first connectable scan result");

    // Options for filtering scan results.
    let _ = opts.optopt("n", "name-filter", "filter by device name", "NAME");
    let _ = opts.optopt("u", "uuid-filter", "filter by UUID", "UUID");

    let matches = opts.parse(args)?;

    if matches.opt_present("h") {
        let brief = format!(
            "Usage: {} scan (--connect|--scan-count=N) [--name-filter=NAME] \
             [--uuid-filter=UUID]",
            appname
        );
        print!("{}", opts.usage(&brief));
        return Ok(());
    }

    state.lock().remaining_scan_results = match matches.opt_str("s") {
        Some(num) => match num.parse() {
            Err(_) | Ok(0) => {
                return Err(format_err!(
                    "{} is not a valid input \
                     - the value must be a positive non-zero number",
                    num
                ));
            }
            Ok(num) => Some(num),
        },
        None => None,
    };

    state.lock().connect = matches.opt_present("c");

    {
        let lock = state.lock();
        if lock.remaining_scan_results.is_some() && lock.connect {
            return Err(format_err!("Cannot use both -s and -c options at the same time"));
        }
    }

    let uuid: Option<fidl_fuchsia_bluetooth::Uuid> = match matches.opt_str("u") {
        None => None,
        Some(val) => {
            // Try to find the UUID as an assigned number (name, abbreviation, number), and fall back to
            // constructing a Uuid from a full UUID string.
            let uuid: Option<Uuid> = assigned_numbers::find_service_uuid(&val).map_or_else(
                || Uuid::from_str(val.as_str()).ok(),
                |sn| Some(Uuid::new16(sn.number)),
            );

            if uuid.is_none() {
                return Err(format_err!("invalid service UUID: {}", val));
            }

            uuid.map(Into::into)
        }
    };

    let name = matches.opt_str("n");

    use bt_gatt::central::{Filter, ScanFilter};

    let mut filters = ScanFilter::default();
    if uuid.is_some() {
        let uuid = bt_gatt_fuchsia::to_gatt_uuid(&uuid.unwrap());
        println!("Adding {} to the scan filter..", uuid.recognize());
        let _ = filters.add(Filter::ServiceUuid(uuid));
    }
    if name.is_some() {
        println!("Filtering by names matching {}", name.as_ref().unwrap());
        let _ = filters.add(Filter::MatchesName(name.unwrap()));
    }
    let scan_fut = state.lock().get_central().scan(&[filters]);

    let watch_fut = central::watch_scan_results(state, scan_fut);

    watch_fut.await.map(|_| ())
}

async fn do_connect<'a, T: bt_gatt::GattTypes>(
    state: CentralStatePtr<T>,
    args: &'a [String],
) -> Result<(), Error>
where
    <T as bt_gatt::GattTypes>::Client: Clone,
{
    if args.len() < 1 {
        println!("connect: peer-id is required");
        return Err(format_err!("invalid connect arguments"));
    }

    let mut opts = Options::new();
    let _ = opts.optopt("u", "uuid", "only discover services that match UUID", "UUID");

    let matches = opts.parse(&args[1..])?;

    let possible_uuid = matches.opt_str("u").map(|u| u.parse::<bt_common::Uuid>());
    let uuid = match possible_uuid {
        None => None,
        Some(Ok(uuid)) => Some(uuid),
        Some(Err(_)) => return Err(format_err!("invalid UUID")),
    };

    let peer_id: PeerId = PeerId::from_str(&args[0]).map_err(|_| format_err!("invalid peer id"))?;

    let central = state.lock().get_central();
    central::connect::<T>(central, bt_gatt_fuchsia::to_gatt_peer_id(&peer_id.into()), uuid).await
}

fn usage(appname: &str) -> () {
    eprintln!(
        "usage: {} <command>
commands:
  scan: Scan for nearby devices and optionally connect to \
         them (pass -h for additional usage)
  connect: Connect to a peer using its ID",
        appname
    );
}

fn main() -> Result<(), Error> {
    let args: Vec<String> = std::env::args().collect();
    let appname = &args[0];

    if args.len() < 2 {
        usage(appname);
        return Ok(());
    }

    let mut executor = fasync::LocalExecutorBuilder::new().build();
    let central_svc = fuchsia_component::client::connect_to_protocol::<CentralMarker>()
        .context("Failed to connect to BLE Central service")?;

    let central = bt_gatt_fuchsia::Central::new(central_svc);

    let state = CentralState::<FuchsiaTypes>::new(central);

    let command = &args[1];
    let fut = async {
        match command.as_str() {
            "scan" => do_scan(appname, &args[2..], state.clone()).await,
            "connect" => do_connect(state.clone(), &args[2..]).await,
            _ => {
                println!("Invalid command: {}", command);
                usage(appname);
                Err(format_err!("invalid command input"))
            }
        }
    };

    executor.run_singlethreaded(fut)
}
