// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{format_err, Context as _, Error};
use argh::FromArgs;
use byteorder::{BigEndian, WriteBytesExt};
use fidl_fuchsia_bluetooth_snoop::{
    DevicePackets, PacketFormat, PacketObserverMarker, PacketObserverRequest, SnoopMarker,
    SnoopPacket, SnoopStartRequest,
};
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use futures::TryStreamExt;
use log::{error, info, warn};
use simplelog::{Config, LevelFilter, WriteLogger};
use std::fs::File;
use std::path::Path;
use std::{fmt, io};

const PCAP_CMD: u8 = 0x01;
const PCAP_ACL_DATA: u8 = 0x02;
const PCAP_SCO_DATA: u8 = 0x03;
const PCAP_EVENT: u8 = 0x04;
const PCAP_ISO_DATA: u8 = 0x05;

enum Format {
    Pcap,
    Pretty,
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let format = match self {
            Format::Pcap => "pcap",
            Format::Pretty => "pretty",
        };
        write!(f, "{}", format)
    }
}

impl std::str::FromStr for Format {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "pcap" => Format::Pcap,
            "pretty" => Format::Pretty,
            _ => {
                info!("Unrecognized format. Using pcap");
                Format::Pcap
            }
        })
    }
}

// Format described in https://wiki.wireshark.org/Development/LibpcapFileFormat#Global_Header
pub fn pcap_header() -> Vec<u8> {
    let mut wtr = vec![];
    wtr.write_u32::<BigEndian>(0xa1b2c3d4).unwrap(); // Magic number
    wtr.write_u16::<BigEndian>(2).unwrap(); // Major Version
    wtr.write_u16::<BigEndian>(4).unwrap(); // Minor Version
    wtr.write_i32::<BigEndian>(0).unwrap(); // Timezone: GMT
    wtr.write_u32::<BigEndian>(0).unwrap(); // Sigfigs
    wtr.write_u32::<BigEndian>(65535).unwrap(); // Max packet length
    wtr.write_u32::<BigEndian>(201).unwrap(); // Protocol: BLUETOOTH_HCI_H4_WITH_PHDR
    wtr
}

fn format_to_byte(format: Option<PacketFormat>) -> u8 {
    let Some(format) = format else {
        warn!("Packet format missing, using ACL DATA");
        return PCAP_ACL_DATA;
    };
    match format {
        PacketFormat::Command => PCAP_CMD,
        PacketFormat::AclData => PCAP_ACL_DATA,
        PacketFormat::SynchronousData => PCAP_SCO_DATA,
        PacketFormat::Event => PCAP_EVENT,
        PacketFormat::IsoData => PCAP_ISO_DATA,
        _ => {
            warn!("Unrecognized packet format, using ACL DATA");
            PCAP_ACL_DATA
        }
    }
}

fn timestamp_to_secs_and_micros(timestamp: i64) -> (u32, u32) {
    let timestamp = fasync::MonotonicDuration::from_nanos(timestamp);
    (
        timestamp.into_seconds() as u32,
        (timestamp.into_micros() % fasync::MonotonicDuration::from_seconds(1).into_micros()) as u32,
    )
}

// Format described in
// https://wiki.wireshark.org/Development/LibpcapFileFormat#Record_.28Packet.29_Header
pub fn to_pcap_fmt(pkt: SnoopPacket) -> Vec<u8> {
    let mut wtr = vec![];
    let (seconds, microseconds) = timestamp_to_secs_and_micros(pkt.timestamp.unwrap());
    wtr.write_u32::<BigEndian>(seconds as u32).unwrap(); // timestamp seconds
    wtr.write_u32::<BigEndian>(microseconds as u32).unwrap();
    // length is len(payload) + 4 octets for is_received + 1 octet for packet type
    wtr.write_u32::<BigEndian>((pkt.data.as_ref().unwrap().len() + 5) as u32).unwrap(); // number of octets of packet saved
    wtr.write_u32::<BigEndian>((pkt.length.unwrap() + 5) as u32).unwrap(); // actual length of packet
    let is_received = if pkt.is_received.unwrap() { 1 } else { 0 };
    wtr.write_u32::<BigEndian>(is_received).unwrap();
    wtr.write_u8(format_to_byte(pkt.format)).unwrap();
    wtr.extend(&pkt.data.unwrap());
    wtr
}

// Pretty print packet metadata with hex representation of payload
fn to_pretty_fmt(pkt: SnoopPacket) -> String {
    let payload = pkt
        .data
        .unwrap()
        .iter()
        .map(|byte| format!("{:x}", byte))
        .collect::<Vec<String>>()
        .join("");
    let (seconds, microseconds) = timestamp_to_secs_and_micros(pkt.timestamp.unwrap());
    let rx_or_tx = if pkt.is_received.unwrap() { "RX" } else { "TX" };
    let dbg_format = format!("{:?}", pkt.format.unwrap());
    format!("{seconds}.{microseconds:06}: {dbg_format:5} {rx_or_tx} {payload}\n")
}

/// Define the command line arguments that the tool accepts.
#[derive(FromArgs)]
#[argh(description = "Snoop Bluetooth controller packets")]
struct Opt {
    #[argh(switch, short = 'd')]
    /// dump the available history of snoop packets
    dump: bool,

    #[argh(option, short = 'f', default = "Format::Pcap")]
    /// file format. options: [pcap, pretty]. Defaults to `pcap`.
    format: Format,

    #[argh(option, short = 'c')]
    /// exit after N packets have been recorded
    count: Option<u64>,

    #[argh(option)]
    /// request snoop log for a single device by name.
    device: Option<String>,

    #[argh(option, short = 'o')]
    /// output location. Default: stdout
    output: Option<String>,

    #[argh(option, short = 't')]
    /// truncate packets to N bytes before outputting them
    truncate: Option<usize>,
}

/// Construct and print a human friendly message relaying the behavior the tool has been invoked
/// to use.
fn print_opts(opts: &Opt) {
    let action = if opts.dump { "Dumping" } else { "Following" };
    let device = if let Some(ref device) = opts.device { device } else { "all devices" };
    let truncate = if let Some(size) = opts.truncate {
        format!("Truncating packets to {} bytes. ", size)
    } else {
        String::new()
    };
    let count =
        if opts.count.is_some() { format!("up to {} ", opts.count.unwrap()) } else { "".into() };
    let output = opts.output.clone().unwrap_or_else(|| "stdout".to_string());
    info!("{action} snoop log for \"{device}\". {truncate}Outputting {count}packets to {output}.",);
}

fn main_res() -> Result<(), Error> {
    // Parse and transform command line arguments.
    let opts: Opt = argh::from_env();

    print_opts(&opts);

    let Opt { dump, format, truncate, count, device, output } = opts;

    let follow = !dump;

    let mut out = match output {
        Some(ref s) => {
            let path = Path::new(s);
            Box::new(File::create(&path).unwrap()) as Box<dyn io::Write>
        }
        None => Box::new(io::stdout()) as Box<dyn io::Write>,
    };

    // create and run the main future
    let main_future = async {
        let snoop_svc = connect_to_protocol::<SnoopMarker>()
            .context("failed to connect to bluetooth snoop interface")?;
        let _ = match format {
            Format::Pcap => out.write(pcap_header().as_slice())?,
            Format::Pretty => 0,
        };
        out.flush()?;

        // Send request to start receiving snoop packets
        let (client, mut request_stream) =
            fidl::endpoints::create_request_stream::<PacketObserverMarker>();

        snoop_svc.start(SnoopStartRequest {
            follow: Some(follow),
            host_device: device,
            client: Some(client),
            ..Default::default()
        })?;

        // Receive snoop packet events and output them in the requested format.
        let mut pkt_count = 0;
        while let Some(request) = request_stream.try_next().await.expect("failed to fetch event") {
            if let PacketObserverRequest::Error { payload, .. } = request {
                return Err(format_err!("Error in packet stream: {payload:?}"));
            }
            let PacketObserverRequest::Observe {
                payload: DevicePackets { packets, .. },
                responder,
            } = request
            else {
                warn!("Unrecognized request from PacketObserver: {request:?}");
                continue;
            };
            let Some(packets) = packets else {
                warn!("No packets in Observe?");
                let _ = responder.send();
                continue;
            };
            for mut packet in packets {
                if let Some(size) = truncate {
                    packet.data.as_mut().unwrap().truncate(size);
                }
                let _ = match format {
                    Format::Pcap => out.write(to_pcap_fmt(packet).as_slice())?,
                    Format::Pretty => out.write(to_pretty_fmt(packet).as_bytes())?,
                };
                out.flush()?;
                if let Some(count) = count {
                    pkt_count += 1;
                    if pkt_count == count {
                        break;
                    }
                }
            }
            let _ = responder.send();
        }
        Ok(())
    };

    fasync::LocalExecutorBuilder::new().build().run_singlethreaded(main_future)
}

fn main() {
    let _ = WriteLogger::init(LevelFilter::Info, Config::default(), io::stderr()).unwrap();
    if let Err(e) = main_res() {
        error!("Error: {e}");
    }
}
