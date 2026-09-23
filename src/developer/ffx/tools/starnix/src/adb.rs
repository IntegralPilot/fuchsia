// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use argh::{ArgsInfo, FromArgs};
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_starnix_container as fstarcontainer;
use fdomain_fuchsia_starnix_container::ControllerProxy;
use ffx_config::EnvironmentContext;
use ffx_config::keys::EMU_INSTANCE_ROOT_DIR;
use ffx_emulator_config::ShowDetail;
use ffx_emulator_engines::EngineBuilder;
use fho::{FfxContext, Result, return_bug, return_user_error};
use fuchsia_async as fasync;
use futures::io::AsyncReadExt;
use futures::stream::StreamExt;
use futures::{FutureExt, channel};
use log::info;
use netext::{MultithreadedTokioAsyncWrapper, TcpListenerStream, TokioAsyncReadExt};
use safe_string::TermSafe;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use signal_hook::consts::signal::SIGINT;
use signal_hook::iterator::Signals;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener as SyncTcpListener};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use target_connector::Connector;
use target_holders::{RemoteControlProxyHolder, SshAddrHolder};
use tokio::net::{TcpListener, TcpStream};

use crate::common::*;

const ADB_DEFAULT_PORT: u16 = 5555;
const ADB_INSTALL_HELP: &str = "Install the adb tool from the Android SDK Platform-Tools and \
    ensure adb is in your $PATH. For more information, see \
    https://fuchsia.dev/fuchsia-src/development/starnix/troubleshooting_ffx_starnix_adb_connect#install-adb";

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "adb",
    example = "ffx starnix adb proxy",
    description = "Bridge from host adb to adbd running inside starnix"
)]
pub struct StarnixAdbCommand {
    /// path to the adb client command
    #[argh(option, default = "String::from(\"adb\")")]
    adb: String,
    #[argh(subcommand)]
    subcommand: AdbSubcommand,
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand)]
enum AdbSubcommand {
    Connect(AdbConnectArgs),
    Proxy(AdbProxyArgs),
}

#[derive(Debug, JsonSchema, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AdbCommandOutput {
    Connect(ConnectOutput),
    Proxy(()),
}

impl std::fmt::Display for AdbCommandOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(o) => write!(f, "{o}"),
            Self::Proxy(()) => Ok(()),
        }
    }
}

impl StarnixAdbCommand {
    pub async fn run(
        self,
        context: &EnvironmentContext,
        rcs_connector: &Connector<RemoteControlProxyHolder>,
        socket_addr: SshAddrHolder,
        nodename: Option<TermSafe>,
    ) -> Result<AdbCommandOutput> {
        self.check_adb()?;
        match self.subcommand {
            AdbSubcommand::Connect(args) => args
                .run_connect(context, self.adb, &socket_addr, nodename)
                .await
                .map(AdbCommandOutput::Connect),
            AdbSubcommand::Proxy(args) => {
                args.run_proxy(&self.adb, rcs_connector).await.map(AdbCommandOutput::Proxy)
            }
        }
    }

    pub fn check_adb(&self) -> Result<()> {
        let mut adb_cmd = Command::new(&self.adb);
        adb_cmd.arg("--version");
        if let Err(io_err) = adb_cmd.output() {
            if io_err.kind() == ErrorKind::NotFound {
                return_user_error!(
                    "Could not find adb binary named `{}`. \
                    adb is required for this command. {}",
                    self.adb,
                    ADB_INSTALL_HELP
                );
            }
            return_bug!("Failed to check for adb at `{}`: {}", self.adb, io_err);
        }
        Ok(())
    }
}

/// directly connect the local adb server to an adbd instance running on the target.
#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(subcommand, name = "connect")]
struct AdbConnectArgs {}

fn run_adb_connect(adb_path: &str, adb_address: &SocketAddr) -> Result<String> {
    let mut adb_cmd = Command::new(adb_path);
    adb_cmd.arg("connect").arg(adb_address.to_string());
    info!("running `{adb_cmd:?}`");

    let connect_res = adb_cmd.output().bug_context("running adb connect")?;
    let stdout = String::from_utf8_lossy(&connect_res.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&connect_res.stderr).into_owned();

    if !connect_res.status.success() {
        return_bug!("Couldn't run adb connect. stdout={stdout} stderr={stderr}");
    }

    Ok(stdout + &stderr)
}

impl AdbConnectArgs {
    async fn run_connect(
        self,
        context: &EnvironmentContext,
        adb: String,
        ssh_address: &SshAddrHolder,
        nodename: Option<TermSafe>,
    ) -> Result<ConnectOutput> {
        let Self {} = self;
        if ssh_address.port() != 22 && ssh_address.port() != 8022 {
            // If the device doesn't have a standard SSH port, it's likely an emulator
            // instance with user networking. Look up the instance and get its host port mapping
            // to find the port we need.
            let Some(nodename) = nodename else {
                return_bug!("No nodename available to find emulator");
            };
            let adb_port = get_emu_host_adb_port(context, &nodename)
                .await
                .bug_context("Finding host adb port for user networking emulator")?;
            let mut adb_address = *ssh_address.clone();
            adb_address.set_port(adb_port);
            run_adb_connect(&adb, &adb_address)?;
            return Ok(ConnectOutput { serial_number: adb_address.to_string() });
        }

        // Try to connect directly. This will work if adbd is already reachable.
        // This only covers `--net tap` emulators, physical devices with CDC ethernet, and
        // funnel. funnel also forwards adb to local 5555, so it's fine to use the default port.
        let mut direct_connect_addr = *ssh_address.clone();
        direct_connect_addr.set_port(ADB_DEFAULT_PORT);
        let output = run_adb_connect(&adb, &direct_connect_addr)?;
        // Note: failed to authenticate occurs if adb can connect but adb is running in secure
        // mode. A developer will either need to accept the adb connection on their device or
        // authenticate against a pre-installed key by the ADB_VENDOR_KEYS environment variable.
        if output.contains("connected to") // also covers "already connected to"
            || output.contains("failed to authenticate to")
        {
            return Ok(ConnectOutput { serial_number: direct_connect_addr.to_string() });
        }

        // Try to connect to the default ADB TCP port. This is set by a property, usually one of
        // `service.adb.listen_addrs`, `service.adb.tcp.port`, or `persist.adb.tcp.port`. Starnix
        // test builds use the ADB default of 5555.
        let forward_addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, ADB_DEFAULT_PORT);
        let output = run_adb_connect(&adb, &forward_addr.into())?;
        if output.contains("connected to") // also covers "already connected to"
            || output.contains("failed to authenticate to")
        {
            return Ok(ConnectOutput { serial_number: forward_addr.to_string() });
        }
        return_user_error!(
            "adb could not connect: {output}\n\
            If ffx is connected, try setting up port forwarding first with \
            `ffx forward '5555=>5555'` followed by `adb connect localhost:5555` in a separate \
            terminal.\n\
            Your connection's \"serial number\" will be 'localhost:5555'."
        );
    }
}

#[derive(Debug, JsonSchema, Deserialize, Serialize)]
pub struct ConnectOutput {
    serial_number: String,
}

impl std::fmt::Display for ConnectOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "adb is connected!")?;
        writeln!(f, "See https://fuchsia.dev/go/troubleshoot-adb-connect if it doesn't work.")?;
        writeln!(f, "This connection's \"serial number\" for adb is '{}'.", self.serial_number)?;
        Ok(())
    }
}

async fn get_emu_host_adb_port(context: &EnvironmentContext, name: &str) -> Result<u16> {
    let instance_dir: PathBuf =
        context.get(EMU_INSTANCE_ROOT_DIR).bug_context("getting emulator instance dir")?;
    let emu_instances = emulator_instance::EmulatorInstances::new(instance_dir);
    let builder = EngineBuilder::new(context, emu_instances);
    let mut instance_name = Some(name.to_string());
    let engine = builder
        .get_engine_by_name(&mut instance_name)
        .with_bug_context(|| format!("getting emulator engine for target {name}"))?
        .bug_context("have configured emulator but no engine was returned")?;
    let mut details = engine.show(vec![ShowDetail::Net {
        mac_address: Default::default(),
        mode: Default::default(),
        ports: Default::default(),
        upscript: Default::default(),
    }]);
    if details.len() != 1 {
        return_bug!("expected emulator details of length 1, got {details:?}");
    }
    match details.remove(0) {
        ShowDetail::Net { ports, .. } => {
            let ports = ports.bug_context("getting host port mappings")?;
            let adb_mapping = ports.get("adb").bug_context("getting adb port mapping")?;
            adb_mapping.host.bug_context("getting adb host port")
        }
        unexpected => return_bug!("asked for networking details, got {unexpected:?}"),
    }
}

async fn serve_adb_connection(
    mut stream: MultithreadedTokioAsyncWrapper<TcpStream>,
    bridge_socket: fdomain_client::Socket,
) -> anyhow::Result<()> {
    let mut bridge = bridge_socket;
    let (breader, mut bwriter) = (&mut bridge).split();
    let (sreader, mut swriter) = (&mut stream).split();

    let copy_result = futures::select! {
        r = futures::io::copy(breader, &mut swriter).fuse() => {
            r
        },
        r = futures::io::copy(sreader, &mut bwriter).fuse() => {
            r
        },
    };

    copy_result.map(|_| ()).map_err(|e| e.into())
}

fn find_open_port(start: u16) -> u16 {
    let mut addr = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), start);

    info!("probing for an open port for the adb bridge...");
    loop {
        info!("probing {addr:?}...");
        match SyncTcpListener::bind(addr) {
            Ok(_) => {
                info!("{addr:?} appears to be available");
                return addr.port();
            }
            Err(e) => {
                info!("{addr:?} appears unavailable: {e:?}");
                addr.set_port(
                    addr.port().checked_add(1).expect("should find open port before overflow"),
                );
            }
        }
    }
}

#[derive(ArgsInfo, FromArgs, Debug, PartialEq)]
#[argh(
    subcommand,
    name = "proxy",
    description = "Bridge from host adb to adbd running inside starnix"
)]
struct AdbProxyArgs {
    /// the moniker of the container running adbd
    /// (defaults to looking for a container in the current session)
    #[argh(option, short = 'm')]
    pub moniker: Option<String>,

    /// which port to serve the adb server on
    #[argh(option, short = 'p', default = "find_open_port(5556)")]
    pub port: u16,

    /// disable automatically running "adb connect"
    #[argh(switch)]
    pub no_autoconnect: bool,
}

impl AdbProxyArgs {
    async fn reconnect(
        rcs_connector: &Connector<RemoteControlProxyHolder>,
        moniker: &Option<String>,
    ) -> anyhow::Result<(ControllerProxy, Arc<AtomicBool>)> {
        let rcs_proxy = connect_to_rcs(rcs_connector).await?;
        anyhow::Ok((
            connect_to_controller(&rcs_proxy, moniker.clone()).await?,
            Arc::new(AtomicBool::new(false)),
        ))
    }
    async fn run_proxy(
        &self,
        adb: &str,
        rcs_connector: &Connector<RemoteControlProxyHolder>,
    ) -> Result<()> {
        let mut controller_proxy = AdbProxyArgs::reconnect(rcs_connector, &self.moniker).await?;
        let mut signals = Signals::new(&[SIGINT]).unwrap();
        let handle = signals.handle();
        let (tx, mut rx) = channel::oneshot::channel();
        let _signal_thread = std::thread::spawn(move || {
            if let Some(signal) = signals.forever().next() {
                assert_eq!(signal, SIGINT);
                eprintln!("Caught interrupt. Shutting down starnix adb bridge...");
                let _ = tx.send(());
                return;
            }
        });

        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, self.port))
            .await
            .expect("cannot bind to adb address");
        let listen_address = listener.local_addr().expect("cannot get adb server address");

        if !self.no_autoconnect {
            // It's necessary to run adb connect on a separate thread so it doesn't block the async
            // executor running the socket listener.
            let adb_path = adb.to_string();
            std::thread::spawn(move || {
                let mut adb_command = Command::new(&adb_path);
                adb_command.arg("connect").arg(listen_address.to_string());
                adb_command
                    .status()
                    .expect("Failed to run adb connect. This should have been caught by check_adb");
            });
        } else {
            println!("ADB bridge started. To connect: adb connect {listen_address}");
        }
        let mut listener = TcpListenerStream(listener);

        loop {
            let handle_stream_fut = Self::handle_stream_next(
                &mut listener,
                &rcs_connector,
                &self.moniker,
                &mut controller_proxy,
            );
            futures::select! {
                stream_res = handle_stream_fut.fuse() => {
                    log::debug!("Handled stream event");
                    if !stream_res? {
                        log::debug!("Stream ran out of connections.");
                        break;
                    }
                },
                _signal_res = rx => {
                    log::debug!("Got interrupt");
                    break;
                }
            }
        }

        handle.close();
        Ok(())
    }

    async fn handle_stream_next(
        listener: &mut TcpListenerStream,
        rcs_connector: &Connector<RemoteControlProxyHolder>,
        moniker: &Option<String>,
        controller_proxy: &mut (ControllerProxy, Arc<AtomicBool>),
    ) -> Result<bool> {
        if let Some(stream) = listener.next().await {
            if controller_proxy.1.load(Ordering::SeqCst) {
                *controller_proxy = Self::reconnect(&rcs_connector, &moniker).await?;
            }

            let stream = stream.map_err(|e| fho::Error::Unexpected(e.into()))?;
            let (sbridge, cbridge) = controller_proxy.0.domain().create_stream_socket();

            controller_proxy
                .0
                .vsock_connect(fstarcontainer::ControllerVsockConnectRequest {
                    port: Some(ADB_DEFAULT_PORT as u32),
                    bridge_socket: Some(sbridge),
                    ..Default::default()
                })
                .context("connecting to adbd")?;

            let reconnect_flag = Arc::clone(&controller_proxy.1);
            fasync::Task::spawn(async move {
                serve_adb_connection(stream.into_multithreaded_futures_stream(), cbridge)
                    .await
                    .unwrap_or_else(|e| println!("serve_adb_connection returned with {:?}", e));
                reconnect_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            })
            .detach();
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::AsyncWriteExt;
    use netext::{TcpListenerStream, TokioAsyncReadExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn run_connection(listener: TcpListener, socket: fdomain_client::Socket) {
        let mut listener = TcpListenerStream(listener);
        if let Some(stream) = listener.next().await {
            let stream = stream.unwrap();
            serve_adb_connection(stream.into_multithreaded_futures_stream(), socket).await.unwrap();
        } else {
            panic!("did not get a connection");
        }
    }

    #[fuchsia::test]
    async fn test_adb_relay() {
        let any_local_address = "127.0.0.1:0";
        let listener = TcpListener::bind(any_local_address).await.unwrap();
        let local_address = listener.local_addr().unwrap();

        let port = local_address.port();

        let client = fdomain_local::local_client_empty();
        let (sbridge, cbridge) = client.create_stream_socket();

        fasync::Task::spawn(async move {
            run_connection(listener, sbridge).await;
        })
        .detach();

        let connect_address = format!("127.0.0.1:{}", port);
        let mut stream = TcpStream::connect(connect_address).await.unwrap().into_futures_stream();

        let test_data_1: Vec<u8> = vec![1, 2, 3, 4, 5];
        stream.write_all(&test_data_1).await.unwrap();

        let mut buf = [0u8; 64];
        let mut async_socket = cbridge;
        let bytes_read = async_socket.read(&mut buf).await.unwrap();
        assert_eq!(test_data_1.len(), bytes_read);
        for (a, b) in test_data_1.iter().zip(buf[..bytes_read].iter()) {
            assert_eq!(a, b);
        }

        let test_data_2: Vec<u8> = vec![6, 7, 8, 9, 10, 11];
        let bytes_written = async_socket.write(&test_data_2).await.unwrap();
        assert_eq!(bytes_written, test_data_2.len());

        let mut buf = [0u8; 64];
        let bytes_read = stream.read(&mut buf).await.unwrap();
        assert_eq!(bytes_written, bytes_written);

        for (a, b) in test_data_2.iter().zip(buf[..bytes_read].iter()) {
            assert_eq!(a, b);
        }
    }
}
