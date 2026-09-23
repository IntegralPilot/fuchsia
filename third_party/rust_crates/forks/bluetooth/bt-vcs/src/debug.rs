// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::debug_command::CommandRunner;
use bt_common::debug_command::CommandSet;
use bt_common::gen_commandset;

use parking_lot::Mutex;

use crate::*;

gen_commandset! {
    VcsCmd {
        Connect = ("connect", [], [], "Connect to VCS"),
        Info = ("info", [], [], "Info about the current status"),
        Up = ("up", [], [], "Volume up"),
        Down = ("down", [], [], "Volume down"),
        Update = ("update", [], [], "Update"),
        Mute = ("mute", [], [], "Mute"),
        Unmute = ("unmute", [], [], "Unmute"),
        Set = ("set", [], ["level"], "Set the level"),
    }
}

pub struct VcsDebug<T: bt_gatt::GattTypes> {
    peer_client: T::Client,
    client: Mutex<Option<Arc<VolumeControlClient<T>>>>,
}

impl<T: bt_gatt::GattTypes> VcsDebug<T> {
    pub fn new(client: T::Client) -> Self {
        Self { peer_client: client, client: Mutex::new(None) }
    }

    async fn connect(&self) -> Option<Arc<VolumeControlClient<T>>> {
        let client_result = VolumeControlClient::connect(&self.peer_client).await;
        let Ok(client) = client_result else {
            eprintln!("Could not connect to VCS: {:?}", client_result.err());
            return None;
        };
        if client.is_none() {
            eprintln!("Found no clients or had an error connecting.");
            return None;
        }
        Some(Arc::new(client.unwrap()))
    }

    fn try_client(&self) -> Option<Arc<VolumeControlClient<T>>> {
        let lock = self.client.lock();
        let Some(vcs_client) = lock.as_ref() else {
            eprintln!("Client not connected, connect first");
            return None;
        };
        Some(vcs_client.clone())
    }
}

impl<T: bt_gatt::GattTypes> CommandRunner for VcsDebug<T> {
    type Set = VcsCmd;

    fn run(
        &self,
        cmd: Self::Set,
        args: Vec<String>,
    ) -> impl futures::Future<Output = Result<(), impl std::error::Error>> {
        async move {
            match cmd {
                // TODO(fxbug.dev/438282674): Add a way to register for vol state changes.
                VcsCmd::Connect => {
                    let lock = self.client.lock();
                    if lock.is_some() {
                        eprintln!("Already connected to VCS");
                        return Ok(());
                    }
                    drop(lock);
                    let Some(vcs_client) = self.connect().await else {
                        eprintln!("Could not connect to VCS");
                        return Ok(());
                    };
                    *self.client.lock() = Some(vcs_client);
                    println!("Connected to VCS");
                }
                VcsCmd::Info => {
                    let Some(client) = self.try_client() else {
                        return Ok(());
                    };
                    println!("{}", client);
                }
                VcsCmd::Up => {
                    let Some(client) = self.try_client() else {
                        return Ok(());
                    };
                    client.volume_up(false).await?;
                }
                VcsCmd::Down => {
                    let Some(client) = self.try_client() else {
                        return Ok(());
                    };
                    client.volume_down(false).await?;
                }
                VcsCmd::Update => {
                    let Some(client) = self.try_client() else {
                        return Ok(());
                    };
                    client.update().await?;
                    println!("{}", client);
                }
                VcsCmd::Mute => {
                    let Some(client) = self.try_client() else {
                        return Ok(());
                    };
                    client.mute().await?;
                }
                VcsCmd::Unmute => {
                    let Some(client) = self.try_client() else {
                        return Ok(());
                    };
                    client.unmute().await?;
                }
                VcsCmd::Set => {
                    let Some(client) = self.try_client() else {
                        return Ok(());
                    };
                    if args.len() != 1 {
                        eprintln!("Expecting one arg: level to set (0-255)");
                        return Ok(());
                    }
                    let Ok(level) = args[0].parse::<u8>() else {
                        eprintln!("Couldn't parse level");
                        return Ok(());
                    };
                    client.set_absolute_volume(level).await?;
                }
            }

            Ok::<(), Error>(())
        }
    }
}
