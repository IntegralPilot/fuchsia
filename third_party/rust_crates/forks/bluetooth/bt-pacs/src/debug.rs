// Copyright 2023 Google LLC
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use bt_common::debug_command::CommandRunner;
use bt_common::debug_command::CommandSet;
use bt_common::gen_commandset;

use bt_gatt::{
    client::{PeerService, PeerServiceHandle},
    Client,
};

use crate::*;

gen_commandset! {
    PacsCmd {
        Print = ("print", [], [], "Print the current PACS status"),
    }
}

pub struct PacsDebug<T: bt_gatt::GattTypes> {
    client: T::Client,
}

impl<T: bt_gatt::GattTypes> PacsDebug<T> {
    pub fn new(client: T::Client) -> Self {
        Self { client }
    }
}

impl<T: bt_gatt::GattTypes> CommandRunner for PacsDebug<T> {
    type Set = PacsCmd;

    fn run(
        &self,
        _cmd: Self::Set,
        _args: Vec<String>,
    ) -> impl futures::Future<Output = Result<(), impl std::error::Error>> {
        async {
            // Since there is only one command, Print, we just print
            // all the characteristics that are at the remote PACS server.
            let handles = self.client.find_service(PACS_UUID).await?;
            for handle in handles {
                let service = handle.connect().await?;

                let chrs = service.discover_characteristics(None).await?;
                for chr in chrs {
                    let mut buf = [0; 120];
                    match chr.uuid {
                        SourcePac::UUID => {
                            let source_pac = SourcePac::try_read::<T>(chr, &service).await?;
                            println!("{source_pac:?}");
                        }
                        SinkPac::UUID => {
                            let sink_pac = SinkPac::try_read::<T>(chr, &service).await?;
                            println!("{sink_pac:?}");
                        }
                        SinkAudioLocations::UUID => {
                            let locations =
                                SinkAudioLocations::try_read::<T>(chr, &service).await?;
                            println!("{locations:?}");
                        }
                        SourceAudioLocations::UUID => {
                            let (bytes, _trunc) =
                                service.read_characteristic(&chr.handle, 0, &mut buf).await?;
                            let locations = SourceAudioLocations::from_chr(chr, &buf[..bytes])
                                .map_err(bt_gatt::types::Error::other)?;
                            println!("{locations:?}");
                        }
                        AvailableAudioContexts::UUID => {
                            let (bytes, _trunc) =
                                service.read_characteristic(&chr.handle, 0, &mut buf).await?;
                            let contexts = AvailableAudioContexts::from_chr(chr, &buf[..bytes])
                                .map_err(bt_gatt::types::Error::other)?;
                            println!("{contexts:?}");
                        }
                        SupportedAudioContexts::UUID => {
                            let (bytes, _trunc) =
                                service.read_characteristic(&chr.handle, 0, &mut buf).await?;
                            let contexts = SupportedAudioContexts::from_chr(chr, &buf[..bytes])
                                .map_err(bt_gatt::types::Error::other)?;
                            println!("{contexts:?}");
                        }
                        _x => println!("Unrecognized Chr {}", chr.uuid.recognize()),
                    }
                }
            }
            Ok::<(), bt_gatt::types::Error>(())
        }
    }
}
