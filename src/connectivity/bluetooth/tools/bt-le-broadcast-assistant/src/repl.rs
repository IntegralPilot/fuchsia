// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use bt_broadcast_assistant::debug::*;
use bt_common::core::AddressType;
use bt_common::debug_command::CommandSet;
use bt_gatt::pii::GetPeerAddr;
use fuchsia_async::{self as fasync, DurationExt, TimeoutExt};
use futures::channel::mpsc::{SendError, channel};
use futures::stream::FusedStream;
use futures::{Sink, SinkExt, Stream, StreamExt};
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{CompletionType, Config, EditMode, Editor};
use std::str::FromStr;
use std::thread;

use crate::address_lookup::LocalPeerAddrCache;
use crate::assistant::*;
use crate::commands::*;

const DEFAULT_SCAN_DURATION_SEC: i64 = 10;

async fn look_for_scan_delegators<
    'a,
    T: bt_gatt::GattTypes + 'static,
    R: GetPeerAddr + Send + Sync + 'static,
>(
    state: &mut AssistantState<T, R>,
    scan_duration_sec: i64,
) where
    T::ScanResultStream: FusedStream + Send + Unpin,
{
    let mut scan_result_stream = match state.debug.look_for_scan_delegators() {
        Ok(stream) => stream,
        Err(e) => {
            eprintln!("\n[Error] Error starting scan: {e:#?}\n");
            return;
        }
    };

    let timeout = fasync::MonotonicDuration::from_seconds(scan_duration_sec).after_now();

    eprintln!("\nLooking for scan delegators for {} seconds...", scan_duration_sec);
    loop {
        let result = scan_result_stream.next().on_timeout(timeout, || None).await;
        match result {
            Some(Ok(scanned)) => {
                let name = match &scanned.name {
                    bt_gatt::central::PeerName::CompleteName(n) => n.as_str(),
                    bt_gatt::central::PeerName::PartialName(n) => n.as_str(),
                    bt_gatt::central::PeerName::Unknown => "<Unknown>",
                };
                eprintln!(
                    "\tFound scan delegator peer ({}): {} (connectable? {})",
                    scanned.id, name, scanned.connectable
                );
            }
            Some(Err(e)) => {
                eprintln!("\n\t[Error] Received scan error: {e:#?}\n");
                break;
            }
            None => {
                eprintln!("\n\tNo more scan results, scan finished.\n");
                break;
            }
        }
    }
    eprintln!("Finished looking for scan delegators after {scan_duration_sec} seconds...\n");
}

// Starts the GATT REPL.
pub async fn start_command_loop<
    'a,
    T: bt_gatt::GattTypes + 'static,
    R: GetPeerAddr + Send + Sync + 'static,
>(
    debug: AssistantDebug<T, R>,
    local_cache: Option<LocalPeerAddrCache>,
) -> Result<(), Error>
where
    T::PeerService: Send,
    T::ScanResultStream: FusedStream + Send + Unpin,
    T::Central: Send,
    T::Client: Send,
    T::NotificationStream: Send,
{
    let mut state = AssistantState::new(debug, local_cache);
    state.start_broadcast_assistant();

    let (mut commands, mut acks) = cmd_stream();
    while let Some(cmd) = commands.next().await {
        handle_cmd(cmd, &mut state).await.map_err(|e| {
            println!("\n[Error] {}\n", e);
            e
        })?;
        acks.send(()).await?;
    }

    Ok(())
}

/// Generates a rustyline `Editor` in a separate thread to manage user input.
fn cmd_stream() -> (impl Stream<Item = String>, impl Sink<(), Error = SendError>) {
    let (mut cmd_sender, cmd_receiver) = channel(512);
    let (ack_sender, mut ack_receiver) = channel(512);

    let _ = thread::spawn(move || -> Result<(), Error> {
        let mut exec = fasync::LocalExecutor::default();
        let fut = async {
            let config = Config::builder()
                .auto_add_history(true)
                .history_ignore_space(true)
                .completion_type(CompletionType::List)
                .edit_mode(EditMode::Emacs)
                .build();
            let c = CmdHelper::new();
            let mut rl: Editor<CmdHelper, DefaultHistory> = Editor::with_config(config)?;
            rl.set_helper(Some(c));
            loop {
                let readline = rl.readline("ASSISTANT> ");
                match readline {
                    Ok(line) => {
                        cmd_sender.try_send(line).expect("should succeed");
                    }
                    Err(ReadlineError::Eof) | Err(ReadlineError::Interrupted) => {
                        return Ok(());
                    }
                    Err(e) => {
                        println!("\n[Error] Error reading input: {e:#?}\n");
                        return Err(e.into());
                    }
                }
                if ack_receiver.next().await.is_none() {
                    return Ok(());
                }
            }
        };
        exec.run_singlethreaded(fut)
    });
    (cmd_receiver, ack_sender)
}

fn handle_set_peer_addr_cmd(
    cache: &LocalPeerAddrCache,
    args: Vec<&str>,
    cmd: &Cmd,
) -> Result<(), Error> {
    if args.len() != 3 {
        return Err(format_err!("Usage: {}", cmd.help_simple()));
    }

    let peer_id = bt_broadcast_assistant::debug::parse_peer_id(args[0])
        .map_err(|e| format_err!("Invalid Peer ID: {}", e))?;

    let addr = bt_broadcast_assistant::debug::parse_bd_addr(args[1])
        .map_err(|e| format_err!("Invalid Address: {}", e))?;

    let addr_type = match args[2] {
        "Public" => AddressType::Public,
        "Random" => AddressType::Random,
        _ => {
            return Err(format_err!(
                "Invalid address type: {}. Must be Public or Random.",
                args[2]
            ));
        }
    };

    cache.set_peer_address(peer_id, addr, addr_type);
    println!("\nCaching {peer_id} address as {addr:?} {addr_type:?}\n");
    Ok(())
}

// Processes `cmd` and returns its result.
async fn handle_cmd<T: bt_gatt::GattTypes + 'static, R: GetPeerAddr + Send + Sync + 'static>(
    line: String,
    state: &mut AssistantState<T, R>,
) -> Result<(), Error>
where
    T::ScanResultStream: FusedStream + Send + Unpin,
    T::NotificationStream: Send,
    T::Central: Send,
    T::Client: Send,
    T::PeerService: Send,
{
    let mut components = line.trim().split_whitespace();
    let command = components.next();
    let args: Vec<&str> = components.collect();
    let args_str: Vec<String> = args.iter().map(|s| String::from(*s)).collect();

    match command {
        Some(token) if Cmd::from_str(token).is_ok() => {
            let cmd: Cmd = token.parse().unwrap();
            match cmd {
                Cmd::Help => {
                    eprintln!("\n{}", Cmd::help_msg());
                    eprintln!("Broadcast Assistant Commands:\r\n{}\n", AssistantCmd::help_all());
                }
                Cmd::Scan => {
                    let mut duration = DEFAULT_SCAN_DURATION_SEC;
                    if args.len() == 1 {
                        match args[0].parse::<i64>() {
                            Ok(d) if d > 0 => duration = d,
                            Ok(_) => {
                                eprintln!("\n[Error] Scan duration must be a positive number.\n");
                                return Ok(());
                            }
                            Err(e) => {
                                eprintln!("\n[Error] Invalid duration: {}\n", e);
                                return Ok(());
                            }
                        }
                    } else if args.len() > 1 {
                        eprintln!("\n[Error] Usage: {}\n", cmd.help_simple());
                        return Ok(());
                    }
                    state.stop_broadcast_assistant().await;
                    look_for_scan_delegators(state, duration).await;
                    state.start_broadcast_assistant();
                }
                Cmd::SetPeerAddr => {
                    if let Some(cache) = &state.local_cache {
                        if let Err(e) = handle_set_peer_addr_cmd(cache, args, &cmd) {
                            eprintln!("\n[Error] {}\n", e);
                        }
                    } else {
                        eprintln!(
                            "\n[Error] `set-peer-addr` is only available with the --use-static-address flag.\n"
                        );
                    }
                }
                Cmd::Exit => {
                    state.stop_broadcast_assistant().await;
                    return Err(format_err!("exited").into());
                }
            }
        }
        Some(token) if AssistantCmd::from_str(token).is_ok() => {
            let cmd: AssistantCmd = token.parse().unwrap();
            assistant_cmd(state, cmd, args_str).await;
        }
        Some(val) => {
            eprintln!("\n[Error] Unknown command: {:?}\n", val);
        }
        None => {}
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::AssistantState;
    use bt_common::core::AddressType;
    use bt_gatt::pii::StaticPeerAddr;
    use bt_gatt_fuchsia::{Central, FuchsiaTypes};

    fn setup_test_state() -> AssistantState<FuchsiaTypes, StaticPeerAddr> {
        let (central_proxy, _central_mock) =
            fidl::endpoints::create_proxy_and_stream::<fidl_fuchsia_bluetooth_le::CentralMarker>();
        let central = Central::new(central_proxy);
        let peer_addr_getter = StaticPeerAddr::new([0; 6], AddressType::Public);
        let debug = AssistantDebug::<FuchsiaTypes, _>::new(central, peer_addr_getter);
        AssistantState::new(debug, None)
    }

    #[fuchsia::test]
    async fn test_handle_cmd_exit() {
        let mut state = setup_test_state();
        let res = handle_cmd("exit".to_string(), &mut state).await;
        assert!(res.is_err());
    }

    #[fuchsia::test]
    async fn test_handle_cmd_help() {
        let mut state = setup_test_state();
        let res = handle_cmd("help".to_string(), &mut state).await;
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_handle_cmd_unknown() {
        let mut state = setup_test_state();
        let res = handle_cmd("foobar".to_string(), &mut state).await;
        // Unknown commands are printed to stderr but are not considered an error in the loop.
        assert!(res.is_ok());
    }

    #[fuchsia::test]
    async fn test_handle_cmd_assistant_cmd() {
        let mut state = setup_test_state();
        // "scan" is a valid AssistantCmd. We expect it to be handled without error.
        let res = handle_cmd("scan".to_string(), &mut state).await;
        assert!(res.is_ok());
    }
}
