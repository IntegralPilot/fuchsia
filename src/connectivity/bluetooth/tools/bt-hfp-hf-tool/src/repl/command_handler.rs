// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use bt_hfp::dtmf::Code as DtmfCode;
use fidl_fuchsia_bluetooth_hfp as hfp;
use fuchsia_sync::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

use super::commands::Command;

use crate::fidl::call::{Call, LocalCallId};
use crate::fidl::peer::{LocalPeerId, Peer};

#[allow(unused)]
pub struct CommandHandler {
    peers: Arc<Mutex<HashMap<LocalPeerId, Peer>>>,
    calls: Arc<Mutex<HashMap<LocalCallId, Call>>>,
}

impl CommandHandler {
    pub fn new(
        peers: Arc<Mutex<HashMap<LocalPeerId, Peer>>>,
        calls: Arc<Mutex<HashMap<LocalCallId, Call>>>,
    ) -> Self {
        Self { peers, calls }
    }

    /// Handles a command and returns `Ok(true)` if the REPL should terminate.
    pub async fn handle_command(
        &mut self,
        command: Command,
        args: Vec<&str>,
    ) -> Result<bool, Error> {
        match command {
            Command::Help => print!("{}", Command::help_msg()),
            Command::ListPeers => self.list_peers(command, args),
            Command::ListCalls => self.list_calls(command, args),
            Command::DialFromNumber => self.dial_from_number(command, args).await,
            Command::DialFromMemoryLocation => self.dial_from_memory_location(command, args).await,
            Command::TransferToHf => self.transfer_to_hf(command, args).await,
            Command::RedialLast => self.redial_last(command, args).await,
            Command::RequestActive => self.request_active(command, args).await,
            Command::RequestTransferToAg => self.request_transfer_to_ag(command, args).await,
            Command::RequestTerminate => self.request_terminate(command, args).await,
            Command::SendDtmfCode => self.send_dtmf_code(command, args).await,
            Command::Exit => return Ok(true),
            command => println! {"{command} not implemented!"},
        }
        Ok(false)
    }

    fn get_peer_by_str_id(&self, str: &str) -> Option<Peer> {
        let Ok(id) = str.parse() else {
            println!("Invalid local peer ID: \"{str}\".");
            return None;
        };

        let peers = self.peers.lock();
        let peer_option = peers.get(&id);

        let Some(peer) = peer_option else {
            println!("No peer with local peer ID {id}.");
            return None;
        };

        Some(peer.clone())
    }

    fn get_all_peers(&self) -> Vec<Peer> {
        let mut peers: Vec<Peer> =
            self.peers.lock().iter().map(|id_and_peer| id_and_peer.1.clone()).collect();
        peers.sort_by_key(|p| p.local_id);

        peers
    }

    fn get_call_by_str_id(&self, str: &str) -> Option<Call> {
        let id_result = str.parse();
        let id = match id_result {
            Err(_) => {
                println!("Invalid call ID: \"{str}\".");
                return None;
            }
            Ok(id) => id,
        };

        let calls = self.calls.lock();
        let call_option = calls.get(&id);

        let call = match call_option {
            None => {
                println!("No call with call ID {id}.");
                return None;
            }
            Some(call) => call,
        };

        Some(call.clone())
    }

    fn get_all_calls(&self) -> Vec<Call> {
        let calls = self.calls.lock();
        let mut calls: Vec<Call> = calls.iter().map(|id_and_call| id_and_call.1.clone()).collect();

        calls.sort_by_key(|c| c.local_id);

        calls
    }

    fn dtmf_code_from_str(str: &str) -> Option<DtmfCode> {
        match str.try_into() {
            Ok(dtmf_code) => Some(dtmf_code),
            Err(()) => {
                println!("Invalid DTMF code: \"{str}\".");
                None
            }
        }
    }

    fn list_peers(&mut self, command: Command, args: Vec<&str>) {
        let len = args.len();
        match len {
            0 => {
                let peers = self.get_all_peers();
                for peer in peers {
                    println!("{peer:?}");
                }
            }
            1 => {
                let str_id = args[0];
                let peer_option = self.get_peer_by_str_id(str_id);
                if let Some(peer) = peer_option {
                    println!("{peer:?}");
                }
                // Else the errors have already been printed.
            }
            _ => println!("Too many argments for {command}:\n\t{}", command.cmd_help()),
        }
    }

    fn list_calls(&mut self, command: Command, args: Vec<&str>) {
        let len = args.len();
        match len {
            0 => {
                let calls = self.get_all_calls();
                for call in calls {
                    println!("{call:?}");
                }
            }
            1 => {
                let str_id = args[0];
                let call_option = self.get_call_by_str_id(str_id);
                if let Some(call) = call_option {
                    println!("{call:?}");
                }
                // Else the errors have already been printed.
            }
            _ => println!("Too many argments for {command}:\n\t{}", command.cmd_help()),
        }
    }

    async fn request_outgoing_call(&self, peer_id_str: &str, call_action: hfp::CallAction) {
        if let Some(peer) = self.get_peer_by_str_id(peer_id_str) {
            let call_result = peer.proxy.request_outgoing_call(&call_action).await;
            match call_result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => println!("HFP error: {err}"),
                Err(err) => println!("FIDL error: {err}"),
            }
        }
        // Else the errors have already been printed.
    }

    async fn dial_from_number(&mut self, command: Command, args: Vec<&str>) {
        let len = args.len();
        match len {
            0 | 1 => println!("Not enough arguments for {command}:\n\t{}", command.cmd_help()),
            2 => {
                let number = String::from(args[1]);
                let call_action = hfp::CallAction::DialFromNumber(number);
                self.request_outgoing_call(/* peer_id = */ args[0], call_action).await
            }
            _ => println!("Too many argments for {command}:\n\t{}", command.cmd_help()),
        }
    }

    async fn dial_from_memory_location(&mut self, command: Command, args: Vec<&str>) {
        let len = args.len();
        match len {
            0 | 1 => println!("Not enough arguments for {command}:\n\t{}", command.cmd_help()),
            2 => {
                let location = String::from(args[1]);
                let call_action = hfp::CallAction::DialFromLocation(location);
                self.request_outgoing_call(/* peer_id = */ args[0], call_action).await
            }
            _ => println!("Too many argments for {command}:\n\t{}", command.cmd_help()),
        }
    }

    async fn transfer_to_hf(&mut self, command: Command, args: Vec<&str>) {
        let len = args.len();
        match len {
            0 => println!("Not enough arguments for {command}:\n\t{}", command.cmd_help()),
            1 => {
                let call_action = hfp::CallAction::TransferActive(hfp::TransferActive);
                self.request_outgoing_call(/* peer_id = */ args[0], call_action).await
            }
            _ => println!("Too many argments for {command}:\n\t{}", command.cmd_help()),
        }
    }

    async fn redial_last(&mut self, command: Command, args: Vec<&str>) {
        let len = args.len();
        match len {
            0 => println!("Not enough arguments for {command}:\n\t{}", command.cmd_help()),
            1 => {
                let call_action = hfp::CallAction::RedialLast(hfp::RedialLast);
                self.request_outgoing_call(/* peer_id = */ args[0], call_action).await
            }
            _ => println!("Too many argments for {command}:\n\t{}", command.cmd_help()),
        }
    }

    async fn request_active(&mut self, command: Command, args: Vec<&str>) {
        let len = args.len();
        match len {
            0 => println!("Not enough arguments for {command}:\n\t{}", command.cmd_help()),
            1 => {
                if let Some(call) = self.get_call_by_str_id(args[0]) {
                    if let Err(err) = call.proxy.request_active() {
                        println!("Error: {:?}", err);
                    }
                }
                // Else the errors have already been printed.
            }
            _ => println!("Too many argments for {command}:\n\t{}", command.cmd_help()),
        }
    }

    async fn request_transfer_to_ag(&mut self, command: Command, args: Vec<&str>) {
        let len = args.len();
        match len {
            0 => println!("Not enough arguments for {command}:\n\t{}", command.cmd_help()),
            1 => {
                if let Some(call) = self.get_call_by_str_id(args[0]) {
                    if let Err(err) = call.proxy.request_transfer_audio() {
                        println!("Error: {:?}", err);
                    }
                }
                // Else the errors have already been printed.
            }
            _ => println!("Too many argments for {command}:\n\t{}", command.cmd_help()),
        }
    }

    async fn request_terminate(&mut self, command: Command, args: Vec<&str>) {
        let len = args.len();
        match len {
            0 => println!("Not enough arguments for {command}:\n\t{}", command.cmd_help()),
            1 => {
                if let Some(call) = self.get_call_by_str_id(args[0]) {
                    if let Err(err) = call.proxy.request_terminate() {
                        println!("Error: {:?}", err);
                    }
                }
                // Else the errors have already been printed.
            }
            _ => println!("Too many argments for {command}:\n\t{}", command.cmd_help()),
        }
    }

    async fn send_dtmf_code(&mut self, command: Command, args: Vec<&str>) {
        let len = args.len();
        match len {
            0 | 1 => println!("Not enough arguments for {command}:\n\t{}", command.cmd_help()),
            2 => {
                let call = match self.get_call_by_str_id(args[0]) {
                    Some(call) => call,
                    None => return, // Errors have already been printed.
                };
                let dtmf_code = match Self::dtmf_code_from_str(args[1]) {
                    Some(dtmf_code) => dtmf_code,
                    None => return, // Errors have already been printed.
                };
                if let Err(err) = call.proxy.send_dtmf_code(dtmf_code.into()).await {
                    println!("Error: {:?}", err);
                }
            }
            _ => println!("Too many argments for {command}:\n\t{}", command.cmd_help()),
        }
    }
}
