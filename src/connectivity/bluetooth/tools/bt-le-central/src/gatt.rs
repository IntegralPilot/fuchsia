// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::StreamExt;
use num::Num;
use std::collections::HashMap;
use std::num::ParseIntError;
use std::str::FromStr;
use std::sync::Arc;

use bt_common::Uuid;
use bt_common::debug_command::{CommandRunner, CommandSet};
use bt_gatt::client::{Client, PeerService, PeerServiceHandle};
use bt_gatt::types::Characteristic;
use bt_pacs::debug::{PacsCmd, PacsDebug};
use bt_vcs::debug::{VcsCmd, VcsDebug};

use self::commands::Cmd;

pub mod commands;
pub mod repl;

struct ActiveService<T: bt_gatt::GattTypes> {
    service: T::PeerService,
    notifiers: Mutex<HashMap<u64, fasync::Task<()>>>,
    found_chars: Mutex<Vec<Characteristic>>,
}

impl<T: bt_gatt::GattTypes> ActiveService<T> {
    fn new(service: T::PeerService) -> Self {
        ActiveService {
            service,
            notifiers: Mutex::new(HashMap::new()),
            found_chars: Mutex::new(Vec::new()),
        }
    }

    /// Discover the characteristics of the currently connected service and
    /// cache them.
    async fn discover_characteristics(&self) -> Result<Vec<Characteristic>, Error> {
        let chars = self.service.discover_characteristics(None).await?;
        let mut lock = self.found_chars.lock();
        *lock = chars.clone();
        Ok(chars)
    }

    fn service(&self) -> &T::PeerService {
        &self.service
    }

    fn subscribe(&self, handle: bt_gatt::types::Handle) -> bool {
        let mut lock = self.notifiers.lock();
        use std::collections::hash_map::Entry;
        let Entry::Vacant(e) = lock.entry(handle.0) else {
            return false;
        };

        let stream = self.service.subscribe(&handle);

        let task = fasync::Task::local(async move {
            let mut stream = Box::pin(stream);
            let id = handle.0;
            while let Some(req) = stream.next().await {
                match req {
                    Ok(bt_gatt::client::CharacteristicNotification {
                        handle: _,
                        value,
                        maybe_truncated: _,
                    }) => {
                        print!("{}{}", repl::CLEAR_LINE, repl::CHA);
                        println!(
                            "(id = {}) value updated: {:X?} {}",
                            id,
                            value,
                            decoded_string_value(&value)
                        );
                    }
                    Err(e) => {
                        println!("(id = {}) notifier closed due to error: {}", id, e);
                        return;
                    }
                }
            }
            println!("(id = {}) notifier closed", id);
        });

        let _ = e.insert(task);
        return true;
    }

    fn unsubscribe(&self, handle: bt_gatt::types::Handle) -> bool {
        self.notifiers.lock().remove(&(handle.0)).is_some()
    }
}

type GattClientPtr<T> = Arc<GattClient<T>>;

struct GattClient<T: bt_gatt::GattTypes> {
    client: T::Client,

    // Services discovered on this client.
    services: Mutex<HashMap<Uuid, Vec<Arc<T::PeerServiceHandle>>>>,

    // Proxy and associated state for the currently connected service, if any.
    active_service: Mutex<Option<Arc<ActiveService<T>>>>,

    // Pacs client
    pacs: Arc<PacsDebug<T>>,

    // Vcs client
    vol: Arc<VcsDebug<T>>,
}

impl<T: bt_gatt::GattTypes> GattClient<T> {
    fn new(client: T::Client) -> GattClientPtr<T>
    where
        <T as bt_gatt::GattTypes>::Client: Clone,
    {
        Arc::new(GattClient {
            client: client.clone(),
            services: Mutex::new(HashMap::new()),
            active_service: Mutex::new(None),
            pacs: Arc::new(PacsDebug::new(client.clone())),
            vol: Arc::new(VcsDebug::new(client)),
        })
    }

    async fn find_service(&self, uuid: Uuid) {
        match self.client.find_service(uuid).await {
            Ok(services) if !services.is_empty() => {
                println!("Found {} services for {}", services.len(), uuid.recognize());
                let _ = self.services.lock().insert(
                    services[0].uuid(),
                    services.into_iter().map(|s| Arc::new(s)).collect(),
                );
            }
            Ok(_) => {
                println!("Found no service for {}", uuid.recognize());
            }
            Err(e) => {
                println!("Error finding service: {e:?}");
            }
        }

        self.display_services();
    }

    fn display_services(&self) {
        for (uuid, svcs) in self.services.lock().iter() {
            let rec = uuid.recognize();
            for (idx, svc) in svcs.iter().enumerate() {
                println!("{} @ {}: Primary {:?}", rec, idx, svc.is_primary());
            }
        }
    }

    /// Connect a service that was already found, putting it in the active service slot.
    /// Another service already connected will be dropped.
    async fn connect_service(&self, uuid: Uuid, idx: usize) -> Result<(), Error> {
        let Some(services) = self.services.lock().get(&uuid).cloned() else {
            println!("No services for {} found", uuid.recognize());
            return Err(format_err!("Not found"));
        };
        let Some(service_handle) = services.get(idx) else {
            println!(
                "There are only {} (< {}) services with UUID {}",
                services.len(),
                idx + 1,
                uuid.recognize()
            );
            return Err(format_err!("Out of range"));
        };

        match service_handle.connect().await {
            Ok(service) => {
                let mut lock = self.active_service.lock();
                *lock = Some(Arc::new(ActiveService::new(service)));
                Ok(())
            }
            Err(e) => {
                println!("Failed to connect to {} {}: {:?}", uuid.recognize(), idx, e);
                Err(e.into())
            }
        }
    }

    fn pacs(&self) -> Arc<PacsDebug<T>> {
        self.pacs.clone()
    }

    fn vcs(&self) -> Arc<VcsDebug<T>> {
        self.vol.clone()
    }

    fn active(&self) -> Option<Arc<ActiveService<T>>> {
        self.active_service.lock().clone()
    }
}

async fn read_characteristic<T: bt_gatt::GattTypes>(
    svc: Arc<ActiveService<T>>,
    id: u64,
) -> Result<(), Error> {
    let mut value: [u8; 22] = [0; 22];
    let (bytes, trunc) = svc
        .service()
        .read_characteristic(&bt_gatt::types::Handle(id), 0, &mut value)
        .await
        .map_err(|e| format_err!("Failed to read characteristic: {e:?}"))?;

    println!(
        "(id = {}) value: {:X?} {} {}",
        id,
        &value[..bytes],
        decoded_string_value(&value[..bytes]),
        if trunc { "maybe truncated.." } else { "" }
    );
    Ok(())
}

async fn read_long_characteristic<T: bt_gatt::GattTypes>(
    svc: Arc<ActiveService<T>>,
    id: u64,
    offset: u16,
    max_bytes: u16,
) -> Result<(), Error> {
    let mut value = vec![0; max_bytes.into()];
    let (bytes, trunc) = svc
        .service()
        .read_characteristic(&bt_gatt::types::Handle(id), offset, value.as_mut_slice())
        .await
        .map_err(|e| format_err!("Failed to read characteristic: {e:?}"))?;

    println!(
        "(id = {}, offset = {}) value (read {}): {:X?} {} {}",
        id,
        offset,
        bytes,
        &value[..bytes],
        decoded_string_value(&value[..bytes]),
        if trunc { "maybe truncated.." } else { "" }
    );
    Ok(())
}

async fn write_characteristic<T: bt_gatt::GattTypes>(
    svc: Arc<ActiveService<T>>,
    id: u64,
    mode: bt_gatt::types::WriteMode,
    offset: u16,
    value: Vec<u8>,
) -> Result<(), Error> {
    svc.service()
        .write_characteristic(&bt_gatt::types::Handle(id), mode, offset, value.as_slice())
        .await
        .map_err(|e| format_err!("Failed to write characteristic: {:?}", e))?;

    println!("(id = {id}, offset = {offset}) done");
    Ok(())
}

async fn read_descriptor<T: bt_gatt::GattTypes>(
    svc: Arc<ActiveService<T>>,
    id: u64,
) -> Result<(), Error> {
    let mut value: [u8; 22] = [0; 22];
    let (bytes, trunc) = svc
        .service()
        .read_descriptor(&bt_gatt::types::Handle(id), 0, &mut value)
        .await
        .map_err(|e| format_err!("Failed to read characteristic: {e:?}"))?;

    println!(
        "(id = {}) value: {:X?} {}",
        id,
        &value[..bytes],
        if trunc { "maybe truncated.." } else { "" }
    );
    Ok(())
}

async fn read_long_descriptor<T: bt_gatt::GattTypes>(
    svc: Arc<ActiveService<T>>,
    id: u64,
    offset: u16,
    max_bytes: u16,
) -> Result<(), Error> {
    let mut value = vec![0; max_bytes.into()];
    let (bytes, trunc) = svc
        .service()
        .read_descriptor(&bt_gatt::types::Handle(id), offset, value.as_mut_slice())
        .await
        .map_err(|e| format_err!("Failed to read characteristic: {e:?}"))?;

    println!(
        "(id = {}, offset = {}) value (read {}): {:X?} {} {}",
        id,
        offset,
        bytes,
        &value[..bytes],
        decoded_string_value(&value[..bytes]),
        if trunc { "maybe truncated.." } else { "" }
    );
    Ok(())
}

async fn write_descriptor<T: bt_gatt::GattTypes>(
    svc: Arc<ActiveService<T>>,
    id: u64,
    offset: u16,
    value: Vec<u8>,
) -> Result<(), Error> {
    let _ = svc
        .service()
        .write_descriptor(&bt_gatt::types::Handle(id), offset, value.as_slice())
        .await
        .map_err(|e| format_err!("Failed to write descriptor: {:?}", e))?;

    println!("(id = {}, offset = {}) desc write done", id, offset);
    Ok(())
}

// ===== REPL =====

fn do_list<T: bt_gatt::GattTypes>(args: &[&str], client: &GattClientPtr<T>) {
    if !args.is_empty() {
        println!("list: expected 0 arguments");
    } else {
        client.display_services();
    }
}

async fn do_find_services<'a, T: bt_gatt::GattTypes>(
    args: &'a [&'a str],
    client: &'a GattClientPtr<T>,
) {
    if args.is_empty() {
        println!("find: expected UUID argument");
        return;
    }
    let Ok(uuid) = parse_uuid(args[0]) else {
        println!("invalid uuid: {}", args[0]);
        return;
    };

    println!("Finding services for {}", uuid.recognize());
    client.find_service(uuid).await;
}

async fn do_connect<'a, T: bt_gatt::GattTypes>(
    args: &'a [&'a str],
    client: &'a GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() != 2 {
        println!("usage: {}", Cmd::Connect.cmd_help());
        return Ok(());
    }

    let Ok(uuid) = parse_uuid(args[0]) else {
        println!("invalid uuid: {}", args[0]);
        return Ok(());
    };

    let index: usize = match parse_int(args[1]) {
        Err(_) => {
            println!("invalid index: {}", args[1]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let Ok(()) = client.connect_service(uuid, index).await else {
        return Ok(());
    };

    let Some(service) = client.active() else {
        println!("No connected service\n");
        return Ok(());
    };

    match service.discover_characteristics().await {
        Ok(characteristics) => {
            for chr in characteristics {
                let handle = chr.handle;
                let uuid = chr.uuid.recognize();
                let descriptors = chr.descriptors;
                let properties = chr.properties;
                println!("{handle:?} {uuid} {properties:?} {descriptors:?}");
            }
        }
        Err(e) => {
            println!("Issue getting characteristics: {e:?}");
        }
    }

    Ok(())
}

async fn do_read_chr<'a, T: bt_gatt::GattTypes>(
    args: &'a [&'a str],
    client: &'a GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() != 1 {
        println!("usage: {}", Cmd::ReadChr.cmd_help());
        return Ok(());
    }

    let id: u64 = match parse_int(args[0]) {
        Err(_) => {
            println!("invalid id: {}", args[0]);
            return Ok(());
        }
        Ok(i) => i,
    };

    match client.active() {
        Some(svc) => read_characteristic(svc, id).await,
        None => {
            println!("no service connected");
            Ok(())
        }
    }
}

async fn do_read_long_chr<'a, T: bt_gatt::GattTypes>(
    args: &'a [&'a str],
    client: &'a GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() != 3 {
        println!("usage: {}", Cmd::ReadLongChr.cmd_help());
        return Ok(());
    }

    let id: u64 = match parse_int(args[0]) {
        Err(_) => {
            println!("invalid id: {}", args[0]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let offset: u16 = match parse_int(args[1]) {
        Err(_) => {
            println!("invalid offset: {}", args[1]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let max_bytes: u16 = match parse_int(args[2]) {
        Err(_) => {
            println!("invalid max bytes: {}", args[2]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let Some(svc) = client.active() else {
        println!("no service connected");
        return Ok(());
    };

    read_long_characteristic(svc, id, offset, max_bytes).await
}

async fn do_write_chr<'a, T: bt_gatt::GattTypes>(
    mut args: Vec<&'a str>,
    client: &'a GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() < 2 {
        println!("usage: {}", Cmd::WriteChr.cmd_help());
        return Ok(());
    }

    let mode = if args[0] == "-w" {
        let _ = args.remove(0);
        bt_gatt::types::WriteMode::WithoutResponse
    } else {
        bt_gatt::types::WriteMode::None
    };

    let id: u64 = match parse_int(args[0]) {
        Err(_) => {
            println!("invalid id: {}", args[0]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let value: Result<Vec<u8>, _> = args[1..].iter().map(|arg| parse_int(arg)).collect();

    match value {
        Err(_) => {
            println!("invalid value");
            Ok(())
        }
        Ok(v) => match client.active() {
            Some(svc) => write_characteristic(svc, id, mode, 0, v).await,
            None => {
                println!("no service connected");
                Ok(())
            }
        },
    }
}

async fn do_write_long_chr<'a, T: bt_gatt::GattTypes>(
    mut args: Vec<&'a str>,
    client: &'a GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() < 3 {
        println!("usage: {}", Cmd::WriteLongChr.cmd_help());
        return Ok(());
    }

    let mode = if args[0] == "-r" {
        let _ = args.remove(0);
        bt_gatt::types::WriteMode::Reliable
    } else {
        bt_gatt::types::WriteMode::None
    };

    let id: u64 = match parse_int(args[0]) {
        Err(_) => {
            println!("invalid id: {}", args[0]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let offset: u16 = match parse_int(args[1]) {
        Err(_) => {
            println!("invalid offset: {}", args[1]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let value: Result<Vec<u8>, _> = args[2..].iter().map(|arg| parse_int(arg)).collect();

    match value {
        Err(_) => {
            println!("invalid value");
            Ok(())
        }
        Ok(v) => match client.active() {
            Some(svc) => write_characteristic(svc, id, mode, offset, v).await,
            None => {
                println!("no service connected");
                Ok(())
            }
        },
    }
}

async fn do_read_desc<'a, T: bt_gatt::GattTypes>(
    args: &'a [&'a str],
    client: &'a GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() != 1 {
        println!("usage: {}", Cmd::ReadDesc.cmd_help());
        return Ok(());
    }

    let id: u64 = match parse_int(args[0]) {
        Err(_) => {
            println!("invalid id: {}", args[0]);
            return Ok(());
        }
        Ok(i) => i,
    };

    match client.active() {
        Some(svc) => read_descriptor(svc, id).await,
        None => {
            println!("no service connected");
            Ok(())
        }
    }
}

async fn do_read_long_desc<'a, T: bt_gatt::GattTypes>(
    args: &'a [&'a str],
    client: &'a GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() != 3 {
        println!("usage: {}", Cmd::ReadLongDesc.cmd_help());
        return Ok(());
    }

    let id: u64 = match parse_int(args[0]) {
        Err(_) => {
            println!("invalid id: {}", args[0]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let offset: u16 = match parse_int(args[1]) {
        Err(_) => {
            println!("invalid offset: {}", args[1]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let max_bytes: u16 = match parse_int(args[2]) {
        Err(_) => {
            println!("invalid max bytes: {}", args[2]);
            return Ok(());
        }
        Ok(i) => i,
    };

    match client.active() {
        Some(svc) => read_long_descriptor(svc, id, offset, max_bytes).await,
        None => {
            println!("no service connected");
            Ok(())
        }
    }
}

async fn do_write_desc<'a, T: bt_gatt::GattTypes>(
    args: Vec<&'a str>,
    client: &'a GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() < 2 {
        println!("usage: {}", Cmd::WriteDesc.cmd_help());
        return Ok(());
    }

    let id: u64 = match parse_int(args[0]) {
        Err(_) => {
            println!("invalid id: {}", args[0]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let value: Result<Vec<u8>, _> = args[1..].iter().map(|arg| parse_int(arg)).collect();

    match value {
        Err(_) => {
            println!("invalid value");
            Ok(())
        }
        Ok(v) => match client.active() {
            Some(svc) => write_descriptor(svc, id, 0, v).await,
            None => {
                println!("no service connected");
                Ok(())
            }
        },
    }
}

async fn do_write_long_desc<'a, T: bt_gatt::GattTypes>(
    args: &'a [&'a str],
    client: &'a GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() < 3 {
        println!("usage: {}", Cmd::WriteLongDesc.cmd_help());
        return Ok(());
    }

    let id: u64 = match parse_int(args[0]) {
        Err(_) => {
            println!("invalid id: {}", args[0]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let offset: u16 = match parse_int(args[1]) {
        Err(_) => {
            println!("invalid offset: {}", args[1]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let value: Result<Vec<u8>, _> = args[2..].iter().map(|arg| parse_int(arg)).collect();

    match value {
        Err(_) => {
            println!("invalid value");
            Ok(())
        }
        Ok(v) => match client.active() {
            Some(svc) => write_descriptor(svc, id, offset, v).await,
            None => {
                println!("no service connected");
                Ok(())
            }
        },
    }
}

async fn do_enable_notify<'a, T: bt_gatt::GattTypes>(
    args: &'a [&'a str],
    client: &'a GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() != 1 {
        println!("usage: {}", Cmd::EnableNotify.cmd_help());
        return Ok(());
    }

    let id: u64 = match parse_int(args[0]) {
        Err(_) => {
            println!("invalid id: {}", args[0]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let Some(svc) = client.active() else {
        println!("no service connected");
        return Ok(());
    };

    if svc.subscribe(bt_gatt::types::Handle(id)) {
        println!("(id = {id}) subscribed");
    } else {
        println!("(id = {id}) already subscribed");
    }
    Ok(())
}

async fn do_disable_notify<'a, T: bt_gatt::GattTypes>(
    args: &'a [&'a str],
    client: &'a GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() != 1 {
        println!("usage: {}", Cmd::DisableNotify.cmd_help());
        return Ok(());
    }

    let id: u64 = match parse_int(args[0]) {
        Err(_) => {
            println!("invalid id: {}", args[0]);
            return Ok(());
        }
        Ok(i) => i,
    };

    let Some(svc) = client.active() else {
        println!("no service connected");
        return Ok(());
    };

    if svc.unsubscribe(bt_gatt::types::Handle(id)) {
        println!("(id = {id}) unsubscribed");
    } else {
        println!("(id = {id}) notifications not enabled");
    }
    Ok(())
}

async fn pacs_cmd<T: bt_gatt::GattTypes>(
    pacs: Arc<PacsDebug<T>>,
    sub_cmd: PacsCmd,
    args: Vec<String>,
) -> Result<(), Error> {
    if let Err(e) = pacs.run(sub_cmd, args).await {
        println!("Error running pacs command: {e:?}");
    }
    Ok(())
}

async fn do_pacs<T: bt_gatt::GattTypes>(
    args: Vec<String>,
    client: GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() < 1 {
        println!("usage: pacs {}", "variants");
        return Ok(());
    }

    let Ok(sub_cmd) = args[0].parse::<PacsCmd>() else {
        println!(
            "subcommands are: {}",
            PacsCmd::variants()
                .into_iter()
                .map(|mut s| {
                    s.push_str(" ");
                    s
                })
                .collect::<String>()
        );
        return Ok(());
    };

    let pacs = client.pacs();
    pacs_cmd(pacs, sub_cmd, args.clone()).await?;
    Ok(())
}

async fn vcs_cmd<T: bt_gatt::GattTypes>(
    vcs: Arc<VcsDebug<T>>,
    sub_cmd: VcsCmd,
    args: Vec<String>,
) -> Result<(), Error> {
    if let Err(e) = vcs.run(sub_cmd, args).await {
        println!("Error running vol command: {e:?}");
    }
    Ok(())
}

async fn do_vcs<T: bt_gatt::GattTypes>(
    args: Vec<String>,
    client: GattClientPtr<T>,
) -> Result<(), Error> {
    if args.len() < 1 {
        println!("usage: vol {}", "variants");
        return Ok(());
    }

    let Ok(sub_cmd) = args[0].parse::<VcsCmd>() else {
        println!(
            "subcommands are: {}",
            VcsCmd::variants()
                .into_iter()
                .map(|mut s| {
                    s.push_str(" ");
                    s
                })
                .collect::<String>()
        );
        return Ok(());
    };

    let vcs = client.vcs();
    vcs_cmd(vcs, sub_cmd, args[1..].iter().cloned().collect()).await?;
    Ok(())
}

/// Attempt to decode the value as a utf-8 string, replacing any invalid byte sequences with '.'
/// characters. Returns an empty string if there are not any valid utf-8 characters.
fn decoded_string_value(value: &[u8]) -> String {
    let decoded_value = String::from_utf8_lossy(value);
    if decoded_value.chars().any(|c| c != '�') {
        decoded_value.replace("�", ".")
    } else {
        String::new()
    }
}

/// Attempt to parse a string into an integer.  If the string begins with 0x, treat the rest
/// of the string as a hex value, otherwise treat it as decimal.
fn parse_int<N>(input: &str) -> Result<N, ParseIntError>
where
    N: Num<FromStrRadixErr = ParseIntError>,
{
    if input.starts_with("0x") {
        N::from_str_radix(&input[2..], 16)
    } else {
        N::from_str_radix(input, 10)
    }
}

fn parse_uuid(input: &str) -> Result<Uuid, Error> {
    Uuid::from_str(input).map_err(Into::into)
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use super::*;
    use bt_fidl_mocks::gatt2::{ClientMock, RemoteServiceMock};
    use bt_fidl_mocks::le::{CentralMock, ConnectionMock};
    use bt_gatt::Central;
    use fidl::endpoints::create_proxy_and_stream;
    use fidl_fuchsia_bluetooth_gatt2::{self as fidl_gatt2, ClientMarker, ServiceHandle};
    use fuchsia_bluetooth::types::{PeerId, Uuid};
    use futures::future::FutureExt;
    use futures::task::Poll;
    use futures::{join, select};
    use std::pin::pin;

    async fn is_pending<T>(
        fut: &mut (impl futures::Future<Output = T> + std::marker::Unpin),
    ) -> bool {
        fasync::TestExecutor::poll_until_stalled(fut).await.is_pending()
    }

    const TIMEOUT: fasync::MonotonicDuration = fasync::MonotonicDuration::from_seconds(1);

    #[fuchsia::test(allow_stalls = false)]
    async fn test_read_chr() {
        let (proxy, central_stream) =
            create_proxy_and_stream::<fidl_fuchsia_bluetooth_le::CentralMarker>();
        let mut central_mock = CentralMock::from_stream(central_stream, TIMEOUT);
        let central = bt_gatt_fuchsia::Central::new(proxy);
        let Poll::Ready(Ok(client)) = fasync::TestExecutor::poll_until_stalled(
            central.connect(bt_gatt_fuchsia::to_gatt_peer_id(&(PeerId(1).into()))),
        )
        .await
        else {
            panic!("couldn't connect");
        };

        let (_, connection_server_end) =
            central_mock.expect_connect(Some(PeerId(1).into())).await.unwrap();

        let mut connection_mock =
            ConnectionMock::from_stream(connection_server_end.into_stream(), TIMEOUT);
        let gatt2_client_server_end = connection_mock.expect_request_gatt_client().await.unwrap();
        let mut client_mock =
            ClientMock::from_stream(gatt2_client_server_end.into_stream(), TIMEOUT);
        client_mock.add_service(fidl_gatt2::ServiceInfo {
            handle: Some(fidl_gatt2::ServiceHandle { value: 1 }),
            kind: Some(fidl_gatt2::ServiceKind::Primary),
            type_: Some(Uuid::new16(0x100d).into()),
            ..Default::default()
        });

        let gatt_client = GattClient::<bt_gatt_fuchsia::FuchsiaTypes>::new(client);

        let args = vec!["0x100d"];
        let mut find_service_fut = pin!(do_find_services(&args, &gatt_client));
        assert!(is_pending(&mut find_service_fut).await);

        client_mock.expect_watch_services().await.unwrap();

        find_service_fut.await;

        let args = vec!["0x100d", "0"];
        let mut connect_fut = pin!(do_connect(&args, &gatt_client));

        assert!(is_pending(&mut connect_fut).await);

        let (_, remote_service_server_end) = client_mock
            .expect_connect_to_service(fidl_gatt2::ServiceHandle { value: 1 })
            .await
            .unwrap();

        let mut service_mock =
            RemoteServiceMock::from_stream(remote_service_server_end.into_stream(), TIMEOUT);

        let chars = vec![fidl_gatt2::Characteristic {
            handle: Some(fidl_gatt2::Handle { value: 1 }),
            type_: Some(Uuid::new16(0x180d).into()),
            properties: Some(fidl_gatt2::CharacteristicPropertyBits::READ),
            permissions: None,
            descriptors: Some(Vec::new()),
            ..Default::default()
        }];
        service_mock.expect_discover_characteristics(&chars).await.unwrap();

        connect_fut.await.unwrap();
        let args = vec!["1"];
        let read_chr_fut = do_read_chr(&args, &gatt_client);

        let _expected_uuid = Uuid::new16(0x180d);
        let read_val = fidl_gatt2::ReadValue {
            handle: Some(fidl_gatt2::Handle { value: 1 }),
            value: Some(vec![0x01, 0x02, 0x03]),
            maybe_truncated: Some(false),
            ..Default::default()
        };
        let expect_fut = service_mock.expect_read_characteristic(1, Ok(&read_val));
        let (read_result, expect_result) = join!(read_chr_fut, expect_fut);

        let _ = read_result.expect("do read chr failed");
        let _ = expect_result.expect("read  expectation not satisfied");
    }

    #[fuchsia::test(allow_stalls = false)]
    async fn test_connect_and_enable_notify() {
        let (proxy, mut central_mock) = CentralMock::new(TIMEOUT);
        let central = bt_gatt_fuchsia::Central::new(proxy);

        let Poll::Ready(Ok(client)) = fasync::TestExecutor::poll_until_stalled(
            central.connect(bt_gatt_fuchsia::to_gatt_peer_id(&(PeerId(1).into()))),
        )
        .await
        else {
            panic!("couldn't connect");
        };
        let (_, connection_server_end) =
            central_mock.expect_connect(Some(PeerId(1).into())).await.unwrap();

        let mut connection_mock =
            ConnectionMock::from_stream(connection_server_end.into_stream(), TIMEOUT);
        let gatt2_client_server_end = connection_mock.expect_request_gatt_client().await.unwrap();
        let mut client_mock =
            ClientMock::from_stream(gatt2_client_server_end.into_stream(), TIMEOUT);
        client_mock.add_service(fidl_gatt2::ServiceInfo {
            handle: Some(fidl_gatt2::ServiceHandle { value: 1 }),
            kind: Some(fidl_gatt2::ServiceKind::Primary),
            type_: Some(Uuid::new16(0x100d).into()),
            ..Default::default()
        });

        let gatt_client = GattClient::<bt_gatt_fuchsia::FuchsiaTypes>::new(client);

        let args = vec!["0x100d"];
        let mut find_service_fut = pin!(do_find_services(&args, &gatt_client));
        assert!(is_pending(&mut find_service_fut).await);

        client_mock.expect_watch_services().await.unwrap();

        find_service_fut.await;

        let args = vec!["0x100d", "0"];
        let mut connect_fut = pin!(do_connect(&args, &gatt_client));

        assert!(is_pending(&mut connect_fut).await);

        let (_, remote_service_server_end) = client_mock
            .expect_connect_to_service(fidl_gatt2::ServiceHandle { value: 1 })
            .await
            .unwrap();

        let mut service_mock =
            RemoteServiceMock::from_stream(remote_service_server_end.into_stream(), TIMEOUT);

        let chars = vec![fidl_gatt2::Characteristic {
            handle: Some(fidl_gatt2::Handle { value: 1 }),
            type_: Some(Uuid::new16(0x180d).into()),
            properties: Some(
                fidl_gatt2::CharacteristicPropertyBits::READ
                    | fidl_gatt2::CharacteristicPropertyBits::NOTIFY,
            ),
            permissions: None,
            descriptors: Some(Vec::new()),
            ..Default::default()
        }];
        let expect_discover_fut = service_mock.expect_discover_characteristics(&chars);
        let (connect_result, expect_discover_result) = join!(connect_fut, expect_discover_fut);
        connect_result.expect("failed to connect");
        expect_discover_result.expect("expect discover failed");

        let args = vec!["1"]; // characteristic handle
        let register_fut = pin!(do_enable_notify(&args, &gatt_client));
        let expect_register_fut =
            service_mock.expect_register_characteristic_notifier(fidl_gatt2::Handle { value: 1 });

        let register_result = register_fut.await;
        let expect_register_result = expect_register_fut.await;

        register_result.expect("failed to register notifier");
        let notifier_client = expect_register_result.expect("expect register failed");
        let notifier = notifier_client.into_proxy();
        let notification_value = fidl_gatt2::ReadValue {
            handle: Some(fidl_gatt2::Handle { value: 1 }),
            value: Some(vec![0x00, 0x01, 0x02]),
            maybe_truncated: Some(false),
            ..Default::default()
        };
        // The notification should immediately receive a flow control response.
        notifier.on_notification(&notification_value).await.expect("on_notification");
    }
}
