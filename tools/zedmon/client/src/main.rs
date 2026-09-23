// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod protocol;
mod zedmon;

use anyhow::{Error, format_err};
use clap::{Arg, ArgMatches, Command};
use serde_json as json;
use std::fs::File;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::Duration;

/// Describes allowable values for the --duration arg of `record`.
const DURATION_REGEX: &'static str = r"^(\d+)(h|m|s|ms)$";

/// Describes allowable values for the --serial arg.
const SERIAL_REGEX: &'static str = r"^\w{24}$";

const ZEDMON_NOMINAL_DATA_RATE_HZ: u32 = 1500;
const ZEDMON_NOMINAL_DATA_INTERVAL_USEC: f32 = 1e6 / ZEDMON_NOMINAL_DATA_RATE_HZ as f32;

/// Validates the --duration arg of `record`.
fn validate_duration(value: &str) -> Result<(), String> {
    let re = regex::Regex::new(DURATION_REGEX).unwrap();
    if re.is_match(&value) {
        Ok(())
    } else {
        Err(format!("Duration must match the regex {}", DURATION_REGEX))
    }
}

/// Validates the --serial arg.
fn validate_serial(value: &str) -> Result<(), String> {
    let re = regex::Regex::new(SERIAL_REGEX).unwrap();
    if re.is_match(&value) {
        Ok(())
    } else {
        Err(format!("Serial must match the regex {}", SERIAL_REGEX))
    }
}

/// Parses the --duration arg of `record`.
fn parse_duration(value: &str) -> Duration {
    let re = regex::Regex::new(DURATION_REGEX).unwrap();
    let captures = re.captures(&value).unwrap();
    let number: u64 = captures[1].parse().unwrap();
    let unit = &captures[2];

    match unit {
        "ms" => Duration::from_millis(number),
        "s" => Duration::from_secs(number),
        "m" => Duration::from_secs(number * 60),
        "h" => Duration::from_secs(number * 3600),
        _ => panic!("Invalid duration string: {}", value),
    }
}

fn validate_downsampling_interval(value: &str) -> Result<(), String> {
    validate_duration(value)?;
    let interval = parse_duration(&value);
    if interval.as_secs_f32() * 1e6 > ZEDMON_NOMINAL_DATA_INTERVAL_USEC {
        Ok(())
    } else {
        Err(format!("Value must be greater than {}us", ZEDMON_NOMINAL_DATA_INTERVAL_USEC))
    }
}

#[fuchsia::main(threads = 1)]
async fn main() -> Result<(), Error> {
    let matches = Command::new("zedmon")
        .about("Utility for interacting with Zedmon power measurement device")
        .subcommand(
            Command::new("describe")
                .about("Describes properties of the device and/or client.")
                .arg(
                    Arg::new("name")
                    .help(
                        "Optional name of a parameter to look up. If provided, only the value will \
                        be printed. Otherwise, all parameter names and values will be printed in \
                        JSON format.")
                    .required(false)
                    .index(1)
                    .value_parser(zedmon::DESCRIBABLE_PROPERTIES),
                ).arg(
                    Arg::new("serial")
                        .help(
                            "Attempts to connect to the attached Zedmon with the specified serial.\
                            Required only if multiple Zedmons are attached.")
                        .short('s')
                        .action(clap::ArgAction::Set)
                        .value_parser(validate_serial)
                )
        )
        .subcommand(
            Command::new("list").about("Lists serial number of connected Zedmon devices"),
        )
        .subcommand(
            Command::new("record").about("Record power data").arg(
                Arg::new("out")
                    .help("Name of output file. Use '-' for stdout.")
                    .short('o')
                    .long("out")
                    .action(clap::ArgAction::Set)
            ).arg(
                Arg::new("average")
                    .help(
                        &format!(
                            "Specifies that the client will output exactly one record, which \
                            averages data over the specified duration. This is equivalent to \
                            setting --duration and --interval to the same value. If specified, \
                            must match the regular expression '{}'.",
                            DURATION_REGEX)
                        )
                    .short('a')
                    .long("average")
                    .action(clap::ArgAction::Set)
                    .value_name("duration")
                    .value_parser(validate_duration)
                    .conflicts_with_all(&["duration", "interval"]),
            ).arg(
                Arg::new("duration")
                    .help(
                        &format!(
                            "Duration of time on the Zedmon device to be spanned by data \
                            recording. If omitted, recording will continue until ENTER is pressed. \
                            If specified, must match the regular expression '{}'.",
                            DURATION_REGEX)
                        )
                    .short('d')
                    .long("duration")
                    .action(clap::ArgAction::Set)
                    .value_parser(validate_duration)
                    .conflicts_with("average"),
            ).arg(
                Arg::new("interval")
                    .help(
                        &format!(
                            "Interval at which to report data. Raw measurements from Zedmon will \
                            be averaged at this interval. \
                            \n  If --interval is omitted, each sample will be reported. If \
                            specified, it must match the regular expression '{}'. It must also be \
                            greater than {:.1}us, Zemdon's nominal reporting interval (corresponding \
                            to {} Hz). \
                            \n  If a gap in raw data contains multiple downsampling output times, \
                            then no samples will be emitted during the gap, and the downsampling \
                            process will reinitialize with the end of the gap as its starting \
                            point.",
                            DURATION_REGEX,
                            ZEDMON_NOMINAL_DATA_INTERVAL_USEC,
                            ZEDMON_NOMINAL_DATA_RATE_HZ)
                        )
                    .short('i')
                    .long("interval")
                    .action(clap::ArgAction::Set)
                    .value_name("duration")
                    .value_parser(validate_downsampling_interval)
                    .conflicts_with("average"),
            ).arg(
                Arg::new("host_timestamps")
                    .help(
                        "If specified, timestamps will be offset to the host clock using a \
                        one-time estimate of the difference between the host and Zedmon \
                        clocks. By default, raw timestamps from Zedmon's clock are emitted.")
                    .short('t')
                    .long("host_timestamps")
                    .action(clap::ArgAction::Set)
            ).arg(
                Arg::new("power")
                    .help("If specified, only output the power result for each sample")
                    .short('p')
                    .long("power")
                    .action(clap::ArgAction::Set)
            ).arg(
                Arg::new("serial")
                    .help(
                        "Attempts to connect to the attached Zedmon with the specified serial.\
                        Required only if multiple Zedmons are attached.")
                    .short('s')
                    .action(clap::ArgAction::Set)
                    .value_parser(validate_serial)
            )
        )
        .subcommand(
            Command::new("relay").about("Enables/disables relay").arg(
                Arg::new("state")
                    .help("State of the relay: 'on' or 'off'")
                    .required(true)
                    .index(1)
                    .value_parser(["on", "off"]))
                    .arg(
                        Arg::new("serial")
                            .help(
                                "Attempts to connect to the attached Zedmon with the specified \
                                serial. Required only if multiple Zedmons are attached.")
                            .short('s')
                            .action(clap::ArgAction::Set)
                            .value_parser(validate_serial)
                    )
        )
        .get_matches();

    match matches.subcommand() {
        Some(("describe", arg_matches)) => run_describe(arg_matches),
        Some(("list", _)) => run_list(),
        Some(("record", arg_matches)) => run_record(arg_matches),
        Some(("relay", arg_matches)) => run_relay(arg_matches),
        _ => panic!("Invalid subcommand"),
    }
}

fn run_describe(arg_matches: &ArgMatches) -> Result<(), Error> {
    let zedmon = zedmon::zedmon(arg_matches.get_one::<String>("serial").map(|s| s.as_str()))?;
    match arg_matches.get_one::<String>("name") {
        Some(name) => println!("{}", zedmon.describe(name).unwrap()),
        None => {
            let mut params = json::Map::<String, json::Value>::new();
            for name in zedmon::DESCRIBABLE_PROPERTIES {
                params.insert(name.to_string(), zedmon.describe(name).unwrap());
            }
            println!("{}", json::to_string_pretty(&json::Value::Object(params)).unwrap());
        }
    }
    Ok(())
}

/// Runs the "list" subcommand.
fn run_list() -> Result<(), Error> {
    let serials = zedmon::list();
    if serials.is_empty() {
        Err(format_err!("No Zedmon devices found"))
    } else {
        for serial in serials {
            println!("{}", serial);
        }
        Ok(())
    }
}

/// Raises a stop signal for Zedmon recording upon the first input to stdin (which, given stdin
/// buffering, means on the first press of ENTER.)
struct StdinStopper {
    receiver: mpsc::Receiver<()>,
    stopped: bool,
}

impl StdinStopper {
    fn new() -> StdinStopper {
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buffer = [0u8; 1];
            loop {
                match stdin.read_exact(&mut buffer) {
                    Ok(_) => {
                        sender.send(()).unwrap();
                        return;
                    }
                    Err(e) => eprintln!("Error reading from stdin: {:?}", e),
                }
            }
        });

        StdinStopper { receiver, stopped: false }
    }
}

impl zedmon::StopSignal for StdinStopper {
    fn should_stop(&mut self, _: u64) -> Result<bool, Error> {
        match self.receiver.try_recv() {
            Ok(()) => self.stopped = true,
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(format_err!("stdin sender was disconnected before signalling."));
            }
        }
        Ok(self.stopped)
    }
}

/// Runs the "record" subcommand".
fn run_record(arg_matches: &ArgMatches) -> Result<(), Error> {
    // Parse --out.
    let (output, dest_name): (Box<dyn Write + Send>, &str) =
        match arg_matches.get_one::<String>("out").map(|s| s.as_str()) {
            None => (Box::new(File::create("zedmon.csv")?), "zedmon.csv"),
            Some("-") => (Box::new(std::io::stdout()), "stdout"),
            Some(filename) => (Box::new(File::create(filename)?), filename),
        };
    let dest_name = dest_name.to_string();

    // Parse either --average or --duration and --interval.
    let (duration, reporting_interval) =
        match arg_matches.get_one::<String>("average").map(|s| s.as_str()) {
            Some(value) => {
                let duration = parse_duration(value);
                (Some(duration), Some(duration))
            }
            None => (
                arg_matches.get_one::<String>("duration").map(|s| parse_duration(s.as_str())),
                arg_matches.get_one::<String>("interval").map(|s| parse_duration(s.as_str())),
            ),
        };

    let zedmon = zedmon::zedmon(arg_matches.get_one::<&str>("serial").map(|s| *s))?;

    println!("Recording to {}.", dest_name);
    let options = zedmon::ReportingOptions {
        interval: reporting_interval,
        use_host_timestamps: arg_matches.contains_id("host_timestamps"),
        output_power_only: arg_matches.contains_id("power"),
    };
    match duration {
        Some(duration) => {
            zedmon.read_reports(output, zedmon::DurationStopper::new(duration), options)
        }
        None => {
            println!("Press ENTER to stop.");
            zedmon.read_reports(output, StdinStopper::new(), options)
        }
    }
}

/// Runs the "relay" subcommand.
fn run_relay(arg_matches: &ArgMatches) -> Result<(), Error> {
    let zedmon = zedmon::zedmon(arg_matches.get_one::<String>("serial").map(|s| s.as_str()))?;
    zedmon.set_relay(arg_matches.get_one::<String>("state").unwrap().as_str() == "on")?;
    Ok(())
}
