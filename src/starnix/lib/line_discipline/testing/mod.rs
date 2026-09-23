// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::*;
use serde::Deserialize;
use starnix_uapi::errors::Errno;
use std::collections::HashMap;

#[derive(Deserialize, Debug)]
struct Scenario {
    name: String,
    initial_termios: TermiosConfig,
    events: Vec<Event>,
    #[allow(dead_code)]
    final_termios: TermiosConfig,
}

#[derive(Deserialize, Debug)]
struct TermiosConfig {
    #[serde(default)]
    c_iflag: Vec<String>,
    #[serde(default)]
    c_oflag: Vec<String>,
    #[serde(default)]
    c_lflag: Vec<String>,
    #[allow(dead_code)]
    c_cflag: Option<u32>, // Keeping cflag simple for now or assume default
    c_cc: Option<HashMap<String, u8>>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum TraceData {
    Bytes(Vec<u8>),
    String(String),
}

impl TraceData {
    fn to_bytes(&self) -> Vec<u8> {
        match self {
            TraceData::Bytes(b) => b.clone(),
            TraceData::String(s) => s.as_bytes().to_vec(),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
enum Event {
    #[serde(rename = "write_to_master")]
    WriteToMaster { data: TraceData },
    #[serde(rename = "read_from_master")]
    ReadFromMaster { data: TraceData },
    #[serde(rename = "read_from_slave")]
    ReadFromSlave { data: TraceData },
    #[serde(rename = "write_to_slave")]
    WriteToSlave { data: TraceData },
    #[serde(rename = "write_to_slave_blocked")]
    WriteToSlaveBlocked { data: TraceData },
    #[serde(rename = "write_to_slave_unexpected_success")]
    WriteToSlaveUnexpectedSuccess { data: TraceData },
    #[serde(rename = "set_packet_mode")]
    SetPacketMode { enabled: bool },
    #[serde(rename = "set_termios")]
    SetTermios { termios: TermiosConfig },
    #[serde(rename = "flush")]
    Flush { side: String, queue_selector: String },
    #[serde(rename = "wait_until_readable")]
    WaitUntilReadable { side: String },
}

struct TestBuffer {
    data: Vec<u8>,
}

impl TestBuffer {
    fn new(data: Vec<u8>) -> Self {
        Self { data }
    }
}

impl InputBuffer for TestBuffer {
    fn available(&self) -> usize {
        self.data.len()
    }
    fn read_to_vec_exact(&mut self, size: usize) -> Result<Vec<u8>, Errno> {
        if size > self.data.len() {
            return error!(EAGAIN);
        }
        let result = self.data.drain(0..size).collect();
        Ok(result)
    }
}

struct TestOutputBuffer {
    data: Vec<u8>,
}

impl TestOutputBuffer {
    fn new() -> Self {
        Self { data: vec![] }
    }
}

impl OutputBuffer for TestOutputBuffer {
    fn write(&mut self, data: &[u8]) -> Result<usize, Errno> {
        self.data.extend_from_slice(data);
        Ok(data.len())
    }
}

fn parse_flags(flags: &[String], mapping: &[(u32, &str)]) -> u32 {
    let mut result = 0;
    for flag in flags {
        if let Some((val, _)) = mapping.iter().find(|(_, name)| name == flag) {
            result |= val;
        } else {
            panic!("Unknown flag {}", flag);
        }
    }
    result
}

fn get_iflag_mapping() -> Vec<(u32, &'static str)> {
    vec![
        (starnix_uapi::IGNBRK, "IGNBRK"),
        (starnix_uapi::BRKINT, "BRKINT"),
        (starnix_uapi::IGNPAR, "IGNPAR"),
        (starnix_uapi::PARMRK, "PARMRK"),
        (starnix_uapi::INPCK, "INPCK"),
        (starnix_uapi::ISTRIP, "ISTRIP"),
        (starnix_uapi::INLCR, "INLCR"),
        (starnix_uapi::IGNCR, "IGNCR"),
        (starnix_uapi::ICRNL, "ICRNL"),
        (starnix_uapi::IUCLC, "IUCLC"),
        (starnix_uapi::IXON, "IXON"),
        (starnix_uapi::IXANY, "IXANY"),
        (starnix_uapi::IXOFF, "IXOFF"),
        (starnix_uapi::IMAXBEL, "IMAXBEL"),
        (starnix_uapi::IUTF8, "IUTF8"),
    ]
}

fn get_oflag_mapping() -> Vec<(u32, &'static str)> {
    vec![
        (starnix_uapi::OPOST, "OPOST"),
        (starnix_uapi::OLCUC, "OLCUC"),
        (starnix_uapi::ONLCR, "ONLCR"),
        (starnix_uapi::OCRNL, "OCRNL"),
        (starnix_uapi::ONOCR, "ONOCR"),
        (starnix_uapi::ONLRET, "ONLRET"),
        (starnix_uapi::OFILL, "OFILL"),
        (starnix_uapi::OFDEL, "OFDEL"),
        (starnix_uapi::XTABS, "XTABS"),
    ]
}

fn get_lflag_mapping() -> Vec<(u32, &'static str)> {
    vec![
        (starnix_uapi::ISIG, "ISIG"),
        (starnix_uapi::ICANON, "ICANON"),
        (starnix_uapi::XCASE, "XCASE"),
        (starnix_uapi::ECHO, "ECHO"),
        (starnix_uapi::ECHOE, "ECHOE"),
        (starnix_uapi::ECHOK, "ECHOK"),
        (starnix_uapi::ECHONL, "ECHONL"),
        (starnix_uapi::ECHOCTL, "ECHOCTL"),
        (starnix_uapi::ECHOPRT, "ECHOPRT"),
        (starnix_uapi::ECHOKE, "ECHOKE"),
        (starnix_uapi::FLUSHO, "FLUSHO"),
        (starnix_uapi::NOFLSH, "NOFLSH"),
        (starnix_uapi::TOSTOP, "TOSTOP"),
        (starnix_uapi::PENDIN, "PENDIN"),
        (starnix_uapi::IEXTEN, "IEXTEN"),
    ]
}

fn get_cc_mapping() -> HashMap<&'static str, usize> {
    let mut m = HashMap::new();
    m.insert("VMIN", starnix_uapi::VMIN as usize);
    m.insert("VTIME", starnix_uapi::VTIME as usize);
    m.insert("VINTR", starnix_uapi::VINTR as usize);
    m.insert("VQUIT", starnix_uapi::VQUIT as usize);
    m.insert("VERASE", starnix_uapi::VERASE as usize);
    m.insert("VKILL", starnix_uapi::VKILL as usize);
    m.insert("VEOF", starnix_uapi::VEOF as usize);
    m.insert("VSTART", starnix_uapi::VSTART as usize);
    m.insert("VSTOP", starnix_uapi::VSTOP as usize);
    m.insert("VSUSP", starnix_uapi::VSUSP as usize);
    m.insert("VEOL", starnix_uapi::VEOL as usize);
    m.insert("VREPRINT", starnix_uapi::VREPRINT as usize);
    m.insert("VDISCARD", starnix_uapi::VDISCARD as usize);
    m.insert("VWERASE", starnix_uapi::VWERASE as usize);
    m.insert("VLNEXT", starnix_uapi::VLNEXT as usize);
    m.insert("VEOL2", starnix_uapi::VEOL2 as usize);
    m
}

pub fn test_replay_trace(name: &str, json_data: &str) {
    println!("Running trace: {}", name);
    let scenario: Scenario = serde_json::from_str(json_data).unwrap_or_else(|e| {
        panic!("Failed to parse trace {}: {}", name, e);
    });
    run_scenario(scenario);
}

fn run_scenario(scenario: Scenario) {
    let iflags = get_iflag_mapping();
    let oflags = get_oflag_mapping();
    let lflags = get_lflag_mapping();

    let mut ld = LineDiscipline::default();
    ld.main_open();
    ld.replica_open();

    // Set initial termios
    let mut termios = crate::get_default_termios();
    termios.c_iflag = parse_flags(&scenario.initial_termios.c_iflag, &iflags);
    termios.c_oflag = parse_flags(&scenario.initial_termios.c_oflag, &oflags);
    termios.c_lflag = parse_flags(&scenario.initial_termios.c_lflag, &lflags);

    if let Some(cc) = &scenario.initial_termios.c_cc {
        let mapping = get_cc_mapping();
        for (name, &val) in cc {
            if let Some(&idx) = mapping.get(name.as_str()) {
                if idx < termios.c_cc.len() {
                    termios.c_cc[idx] = val;
                }
            } else {
                // Decide if panic or warn. Let's panic for correctness.
                panic!("Unknown c_cc name {}", name);
            }
        }
    }

    let _ = ld.set_termios(termios);

    for event in scenario.events {
        match event {
            Event::WriteToMaster { data } => {
                let mut buffer = TestBuffer::new(data.to_bytes());
                let _ = ld.main_write(&mut buffer).expect("main_write failed");
            }
            Event::ReadFromMaster { data } => {
                let mut buffer = TestOutputBuffer::new();
                loop {
                    match ld.main_read(&mut buffer) {
                        Ok(_) => {}
                        Err(e) if e == (error!(EAGAIN) as Result<(), Errno>).unwrap_err() => {
                            break;
                        }
                        Err(e) => panic!("main_read failed: {:?}", e),
                    }
                }
                assert_eq!(
                    String::from_utf8_lossy(&buffer.data),
                    String::from_utf8_lossy(&data.to_bytes()),
                    "ReadFromMaster mismatch in {}",
                    scenario.name
                );
            }
            Event::ReadFromSlave { data } => {
                let mut buffer = TestOutputBuffer::new();
                loop {
                    match ld.replica_read(&mut buffer) {
                        Ok(_) => {}
                        Err(e) if e == (error!(EAGAIN) as Result<(), Errno>).unwrap_err() => {
                            break;
                        }
                        Err(e) => panic!("replica_read failed: {:?}", e),
                    }
                }
                assert_eq!(
                    String::from_utf8_lossy(&buffer.data),
                    String::from_utf8_lossy(&data.to_bytes()),
                    "ReadFromSlave mismatch in {}",
                    scenario.name
                );
            }
            Event::WriteToSlave { data } => {
                let mut buffer = TestBuffer::new(data.to_bytes());
                let _ = ld.replica_write(&mut buffer).expect("replica_write failed");
            }
            Event::WriteToSlaveBlocked { data } => {
                let mut buffer = TestBuffer::new(data.to_bytes());
                let result = ld.replica_write(&mut buffer);
                assert!(
                    result.is_err(),
                    "Expected replica_write to block/fail in {}, but it succeeded",
                    scenario.name
                );
                assert_eq!(result, error!(EAGAIN), "Expected EAGAIN in {}", scenario.name);
            }
            Event::WriteToSlaveUnexpectedSuccess { data } => {
                // This event means the trace generator expected it to block but it didn't.
                // It effectively means "WriteToSlave".
                // However, for strictness, maybe we should warn?
                // But if it's in the trace as "Success", we replay it as success.
                let mut buffer = TestBuffer::new(data.to_bytes());
                let _ = ld
                    .replica_write(&mut buffer)
                    .expect("replica_write failed (unexpected success case)");
            }
            Event::SetPacketMode { enabled } => {
                ld.set_packet_mode(enabled);
            }
            Event::SetTermios { termios: ref termios_config } => {
                let mut termios = crate::get_default_termios();
                termios.c_iflag = parse_flags(&termios_config.c_iflag, &iflags);
                termios.c_oflag = parse_flags(&termios_config.c_oflag, &oflags);
                termios.c_lflag = parse_flags(&termios_config.c_lflag, &lflags);
                if let Some(cc) = &termios_config.c_cc {
                    let mapping = get_cc_mapping();
                    for (name, &val) in cc {
                        if let Some(&idx) = mapping.get(name.as_str()) {
                            if idx < termios.c_cc.len() {
                                termios.c_cc[idx] = val;
                            }
                        }
                    }
                }
                let _ = ld.set_termios(termios);
            }
            Event::Flush { side, queue_selector } => {
                let is_main = match side.as_str() {
                    "main" => true,
                    "replica" => false,
                    _ => panic!("Unknown side {}", side),
                };
                let queue_selector_val = match queue_selector.as_str() {
                    "TCIFLUSH" => starnix_uapi::TCIFLUSH,
                    "TCOFLUSH" => starnix_uapi::TCOFLUSH,
                    "TCIOFLUSH" => starnix_uapi::TCIOFLUSH,
                    _ => panic!("Unknown queue_selector {}", queue_selector),
                };
                ld.flush(is_main, queue_selector_val).expect("flush failed");
            }
            Event::WaitUntilReadable { side } => {
                // Line discipline read and write operations are blocking, so once the write has
                // returned, the data is already available in the read queue.
                //
                // Instead, we simply assert that there is data to read.
                struct ZeroSizedBuffer {}
                impl OutputBuffer for ZeroSizedBuffer {
                    fn write(&mut self, _data: &[u8]) -> Result<usize, Errno> {
                        Ok(0)
                    }
                }
                let mut buf = ZeroSizedBuffer {};

                match side.as_str() {
                    "main" => {
                        assert_eq!(Ok(0), ld.main_read(&mut buf));
                    }
                    "replica" => {
                        assert_eq!(Ok(0), ld.replica_read(&mut buf));
                    }
                    _ => panic!("Unknown side {}", side),
                };
            }
        }
    }
}
