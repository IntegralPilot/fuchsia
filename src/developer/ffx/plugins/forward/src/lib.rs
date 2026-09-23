// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::VecDeque;
use std::fmt::Display;
use std::net::{Ipv6Addr, SocketAddr};
use std::num::NonZeroU64;
use std::pin::pin;
use std::time::{Duration, Instant};

use errors::ffx_error;
use ffx_forward_args::{Direction, ForwardCommand, ForwardSpec, ProtoSpec};
use ffx_target_net::{Bidirectional, Counters, PortForwarder};
use ffx_writer::{ToolIO, VerifiedMachineWriter};
use fho::{FfxMain, FfxTool};
use fuchsia_async as fasync;
use futures::future::{BoxFuture, Fuse, FusedFuture as _, OptionFuture};
use futures::stream::FusedStream;
use futures::{FutureExt as _, Stream, StreamExt as _};
use log::{error, info};
use schemars::JsonSchema;
use serde::Serialize;
use speedtest::{BytesFormatter, Throughput};
use target_connector::Connector;
use target_holders::RemoteControlProxyHolder;

use termion as _;

const CONNECT_INTERVAL: Duration = Duration::from_secs(5);

#[derive(FfxTool)]
pub struct ForwardTool {
    remote_control_connector: Connector<RemoteControlProxyHolder>,
    #[command]
    cmd: ForwardCommand,
}

fho::embedded_plugin!(ForwardTool);

fn interval(dur: Duration) -> impl Stream<Item = ()> + FusedStream {
    futures::stream::unfold((), move |()| fasync::Timer::new(dur).map(|()| Some(((), ())))).fuse()
}

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardMessage {
    Forwarding(Vec<ForwardSpec>),
}

impl std::fmt::Display for ForwardMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Forwarding(specs) => {
                write!(
                    f,
                    "Forwarding: \n\t{}",
                    specs.iter().map(|f| f.to_string()).collect::<Vec<_>>().join("\n\t")
                )
            }
        }
    }
}

#[async_trait::async_trait(?Send)]
impl FfxMain for ForwardTool {
    type Writer = VerifiedMachineWriter<ForwardMessage>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        let Self { remote_control_connector, cmd } = self;
        let ForwardCommand { quiet, spec, ui_interval, once } = cmd;
        if spec.is_empty() {
            return Err(fho::user_error!("no forwarding specs provided"));
        }

        let mut ui_interval_stream = if quiet {
            futures::stream::pending().left_stream()
        } else {
            interval(ui_interval).right_stream()
        };

        let emit_machine_messages = !quiet && writer.is_machine();
        let forwarder = Forwarder::new(&spec, &remote_control_connector).await?;
        if emit_machine_messages {
            writer.item(&ForwardMessage::Forwarding(forwarder.forwarded_specs()))?;
        }
        // The mutable state kept by the tool:
        // The current forwarder, None if we're disconnected.
        let mut forwarder = Some(forwarder);
        // The last error captured for display in the rendered.
        let mut display_error = None;
        // An optional future that may be waiting for a new forwarder to be
        // created.
        let mut forwarder_fut = pin!(OptionFuture::default());
        // An optional future that is Some when we're delaying a reconnection.
        let mut reconnect_wait_fut = pin!(OptionFuture::default());
        // The timestamp at which we expect connection to start again, only
        // meaningful if the reconnect_wait_fut is set, but we're skipping
        // creating a merged type for both things so we can leverage
        // OptionFuture more easily.
        let mut connection_deadline = Instant::now();

        let mut renderer = (!quiet && !writer.is_machine()).then(Renderer::default);

        loop {
            enum Work {
                ConnectInterval,
                RefreshUi,
                NewForwarder(fho::Result<Forwarder>),
                ForwardError(ffx_target_net::Error),
            }

            let mut sentinel = OptionFuture::from(forwarder.as_mut().map(|f| &mut f.sentinel));
            let work = futures::select! {
                r = reconnect_wait_fut => {
                    r.unwrap();
                    Work::ConnectInterval
                },
                () = ui_interval_stream.select_next_some() => Work::RefreshUi,
                r = forwarder_fut => Work::NewForwarder(r.unwrap()),
                e = sentinel => Work::ForwardError(e.unwrap()),
            };

            match work {
                Work::ConnectInterval => {
                    if forwarder.is_some() {
                        continue;
                    }
                    forwarder_fut
                        .set(Some(Forwarder::new(&spec, &remote_control_connector).fuse()).into());
                }
                Work::NewForwarder(r) => match r {
                    Ok(f) => {
                        if emit_machine_messages {
                            writer.item(&ForwardMessage::Forwarding(f.forwarded_specs()))?;
                        }
                        forwarder = Some(f);
                        forwarder_fut.set(None.into());
                        display_error = None;
                    }
                    Err(e) => {
                        log::error!("failed to create new forwarder: {e}");
                        display_error = Some(e);
                        // Reattempt a connection later.
                        connection_deadline = Instant::now() + CONNECT_INTERVAL;
                        reconnect_wait_fut
                            .set(Some(fasync::Timer::new(connection_deadline).fuse()).into());
                    }
                },
                Work::ForwardError(e) => {
                    let e = fho::Error::User(e.into());
                    if once {
                        return Err(e);
                    }
                    log::error!("lost forwarding connection: {e}");
                    display_error = Some(e);
                    if let Some(f) = forwarder.take() {
                        f.shutdown().await;
                    }
                    // Attempt a reconnection later.
                    connection_deadline = Instant::now() + CONNECT_INTERVAL;
                    reconnect_wait_fut
                        .set(Some(fasync::Timer::new(connection_deadline).fuse()).into());
                }
                // Tick to refresh the TUI.
                Work::RefreshUi => {}
            }

            let Some(renderer) = renderer.as_mut() else {
                continue;
            };

            match forwarder.as_ref() {
                Some(f) => renderer.render(f.inner.read_counters(), ui_interval, &mut writer),
                None => renderer.render_disconnected(
                    forwarder_fut
                        .is_terminated()
                        .then(|| connection_deadline.saturating_duration_since(Instant::now())),
                    display_error.as_ref(),
                    &mut writer,
                ),
            }
            .map_err(|e| ffx_error!(e))?;
        }
    }
}

async fn new_port_forwarder(
    connector: &Connector<RemoteControlProxyHolder>,
) -> fho::Result<PortForwarder> {
    let rcs_proxy = connector
        .try_connect(|target, err| {
            log::info!(
                "Waiting for target '{}'",
                match target {
                    Some(s) => s,
                    _ => "None",
                }
            );
            if let Some(err) = err {
                log::error!("target connect is in error state: {err:?}");
            }
            Ok(())
        })
        .await?;
    let forwarder = PortForwarder::new_with_rcs(Duration::from_secs(10), &*rcs_proxy)
        .await
        .map_err(|e| ffx_error!(e))?;
    Ok(forwarder)
}

struct Forwarder {
    inner: PortForwarder,
    scope: fasync::Scope,
    sentinel: Fuse<BoxFuture<'static, ffx_target_net::Error>>,
    forwarded_specs: Vec<ForwardSpec>,
}

impl Forwarder {
    async fn new(
        spec: &Vec<ForwardSpec>,
        connector: &Connector<RemoteControlProxyHolder>,
    ) -> fho::Result<Self> {
        let forwarder = new_port_forwarder(connector).await?;
        Self::new_with_forwarder(spec, forwarder).await
    }

    async fn new_with_forwarder(
        spec: &Vec<ForwardSpec>,
        forwarder: PortForwarder,
    ) -> fho::Result<Self> {
        let scope = fasync::Scope::new();

        // Create a sentinel socket whose accept call just drops all
        // connections, but failure on its accept call means we lost connection
        // to the target.
        let sentinel = forwarder
            .socket_provider()
            .listen(SocketAddr::new(Ipv6Addr::LOCALHOST.into(), 0), Some(1))
            .await
            .map_err(|e| ffx_error!("failed to create sentinel listener: {e}"))?;
        let sentinel = sentinel
            .into_stream()
            .filter_map(|r| futures::future::ready(r.err()))
            .boxed()
            .into_future()
            .map(|(r, _)| r.unwrap_or(ffx_target_net::Error::Hangup))
            .boxed()
            .fuse();

        // The ForwardSpec's that are passed to us may have `0` in them,
        // instructing the host OS to pick a port for us. For debugging/reporting
        // purposes we need to keep track of the "final" forwarding specs.
        let mut forwarded_specs = vec![];

        for ForwardSpec { host, target, direction } in spec.iter() {
            info!(
                "setting up forwarding host: {:?}, target: {:?}, direction: {:?}",
                host, target, direction
            );
            match direction {
                Direction::HostToTarget => {
                    let listener = match host {
                        ProtoSpec::Tcp(addr) => tokio::net::TcpListener::bind(addr)
                            .await
                            .map_err(|e| fho::user_error!(e))?,
                    };
                    let target_addr = match target {
                        ProtoSpec::Tcp(addr) => addr,
                    };
                    let host_addr = listener
                        .local_addr()
                        .map_err(|e| ffx_error!("Could not get local address: {}", e))?;

                    let _: fasync::JoinHandle<()> = scope.spawn_local(
                        forwarder
                            .forward(listener, *target_addr)
                            .map(|r| r.unwrap_or_else(|e| error!("forwarding error: {e:?}"))),
                    );
                    forwarded_specs.push(ForwardSpec {
                        direction: *direction,
                        host: ProtoSpec::Tcp(host_addr),
                        target: target.clone(),
                    });
                }
                Direction::TargetToHost => {
                    let listener = match target {
                        ProtoSpec::Tcp(addr) => forwarder
                            .socket_provider()
                            .listen(*addr, None)
                            .await
                            .map_err(|e| fho::user_error!(e))?,
                    };
                    let host_addr = match host {
                        ProtoSpec::Tcp(addr) => addr,
                    };
                    let target_addr = ProtoSpec::Tcp(listener.local_addr());
                    let _: fasync::JoinHandle<()> =
                        scope.spawn_local(forwarder.reverse(listener, *host_addr).map(|r| {
                            r.unwrap_or_else(|e| error!("reverse forwarding error: {e:?}"))
                        }));
                    forwarded_specs.push(ForwardSpec {
                        direction: *direction,
                        host: host.clone(),
                        target: target_addr,
                    });
                }
            }
        }

        Ok(Self { inner: forwarder, forwarded_specs, scope, sentinel })
    }

    async fn shutdown(self) {
        let Self { inner: _, scope, sentinel: _, forwarded_specs: _ } = self;
        scope.abort().await;
    }

    fn forwarded_specs(&self) -> Vec<ForwardSpec> {
        self.forwarded_specs.clone()
    }
}

#[derive(Default)]
struct Renderer {
    prev_bytes: Option<Bidirectional>,
    period: usize,
    max_throughput: f64,
    host_to_target: BarRenderer,
    target_to_host: BarRenderer,
    rendered_lines: u16,
}

impl Renderer {
    const PERIOD_SPINNER: [char; 2] = ['⇋', '⇌'];
    const MIN_BARS: u16 = 5;

    fn render<W: std::io::Write>(
        &mut self,
        new_counters: Counters,
        delta: Duration,
        w: &mut W,
    ) -> std::io::Result<()> {
        let bar_width = termion::terminal_size()
            .map(|(w, _)| w.saturating_sub(35).max(Self::MIN_BARS))
            .unwrap_or(Self::MIN_BARS)
            .into();

        self.move_up(w)?;
        let Self {
            prev_bytes,
            period,
            max_throughput,
            host_to_target: host_to_target_render,
            target_to_host: target_to_host_render,
            rendered_lines: _,
        } = self;
        let Counters { active_connections, total_bytes } = new_counters;

        let interval_bytes = match prev_bytes {
            Some(Bidirectional { host_to_target, target_to_host }) => Bidirectional {
                host_to_target: total_bytes.host_to_target.saturating_sub(*host_to_target),
                target_to_host: total_bytes.target_to_host.saturating_sub(*target_to_host),
            },
            None => Bidirectional::default(),
        };
        let spinner = Self::PERIOD_SPINNER[*period];
        let host_to_target_throughput = Throughput::from_len_and_duration(
            interval_bytes.host_to_target.try_into().unwrap_or(u32::MAX),
            delta,
        );
        let target_to_host_throughput = Throughput::from_len_and_duration(
            interval_bytes.target_to_host.try_into().unwrap_or(u32::MAX),
            delta,
        );
        *max_throughput = max_throughput
            .max(host_to_target_throughput.as_f64())
            .max(target_to_host_throughput.as_f64());
        host_to_target_render.update(
            host_to_target_throughput.as_f64(),
            bar_width,
            *max_throughput,
        );
        target_to_host_render.update(
            target_to_host_throughput.as_f64(),
            bar_width,
            *max_throughput,
        );

        writeln!(
            w,
            "{}({}) → Host to Target {} ({}) ← Target to Host",
            termion::clear::CurrentLine,
            active_connections.host_to_target,
            spinner,
            active_connections.target_to_host,
        )?;
        writeln!(
            w,
            "{}→ |{}| {} / {}",
            termion::clear::CurrentLine,
            host_to_target_render,
            host_to_target_throughput,
            BytesFormatter(total_bytes.host_to_target.try_into().unwrap_or(u64::MAX)),
        )?;
        writeln!(
            w,
            "{}← |{}| {} / {}",
            termion::clear::CurrentLine,
            target_to_host_render,
            target_to_host_throughput,
            BytesFormatter(total_bytes.target_to_host.try_into().unwrap_or(u64::MAX)),
        )?;
        *prev_bytes = Some(total_bytes);
        *period = (*period + 1) % Self::PERIOD_SPINNER.len();
        self.update_rendered(3, w)?;
        Ok(())
    }

    fn move_up<W: std::io::Write>(&mut self, w: &mut W) -> std::io::Result<()> {
        if self.rendered_lines == 0 {
            return Ok(());
        }
        write!(w, "{}", termion::cursor::Up(self.rendered_lines))
    }

    fn update_rendered<W: std::io::Write>(&mut self, to: u16, w: &mut W) -> std::io::Result<()> {
        if let Some(diff) = self.rendered_lines.checked_sub(to) {
            if diff != 0 {
                for _ in 0..diff {
                    writeln!(w, "{}", termion::clear::CurrentLine)?;
                }
                write!(w, "{}", termion::cursor::Up(diff))?;
            }
        }
        self.rendered_lines = to;
        Ok(())
    }

    fn render_disconnected<W: std::io::Write>(
        &mut self,
        reconnect_delay: Option<Duration>,
        display_error: Option<&fho::Error>,
        w: &mut W,
    ) -> std::io::Result<()> {
        self.move_up(w)?;

        // Consider anything below 1s as connecting now, it's just a better ui.
        let reconnect_delay = reconnect_delay.and_then(|dur| NonZeroU64::new(dur.as_secs()));
        match reconnect_delay {
            Some(delay) => {
                writeln!(
                    w,
                    "{}Connection lost. Reconnecting in {}s...",
                    termion::clear::CurrentLine,
                    delay,
                )?;
            }
            None => {
                writeln!(w, "{}Reconnecting to target...", termion::clear::CurrentLine)?;
            }
        }
        let mut rendered = 1;

        if let Some(e) = display_error {
            writeln!(
                w,
                "{}Last error: {}{}{}",
                termion::clear::CurrentLine,
                termion::color::Fg(termion::color::Red),
                e,
                termion::color::Fg(termion::color::Reset),
            )?;
            rendered += 1;
        }
        self.update_rendered(rendered, w)?;
        Ok(())
    }
}

#[derive(Default)]
struct BarRenderer {
    history: VecDeque<f64>,
    scale: f64,
}

impl BarRenderer {
    const THROUGHPUT_BARS: [char; 7] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇'];

    fn update(&mut self, v: f64, width: usize, scale: f64) {
        self.scale = scale;
        while self.history.len() < width - 1 {
            self.history.push_front(0f64);
        }
        self.history.push_back(v);
        while self.history.len() > width {
            self.history.pop_front();
        }
    }
}

impl Display for BarRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for v in &self.history {
            let ch = if *v == 0f64 {
                ' '
            } else {
                let bar_index = (v / self.scale * (Self::THROUGHPUT_BARS.len() as f64)) as usize;
                Self::THROUGHPUT_BARS[bar_index.min(Self::THROUGHPUT_BARS.len() - 1)]
            };
            write!(f, "{ch}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use ffx_forward_args::{Direction, ForwardSpec, ProtoSpec};
    use ffx_target_net_testutil::FakeNetstack;
    use ffx_writer::{Format, TestBuffers};
    use itertools::Itertools;
    use net_declare::{std_ip_v4, std_socket_addr};
    use serde_json::json;
    use vte::{Parser, Perform};

    const INTERVAL: Duration = Duration::from_secs(1);

    #[derive(Default)]
    struct TestWriter {
        data: InnerWriter,
        parser: Parser,
    }

    impl std::io::Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.parser.advance(&mut self.data, buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl TestWriter {
        fn to_string(&mut self) -> String {
            String::from_utf8(self.data.lines.join("\n".as_bytes())).expect("invalid string")
        }
    }

    #[derive(Default)]
    struct InnerWriter {
        lines: Vec<Vec<u8>>,
        cur_line: usize,
    }

    impl InnerWriter {
        fn get_line(&mut self) -> &mut Vec<u8> {
            if self.lines.len() <= self.cur_line {
                self.lines.resize_with(self.cur_line + 1, Vec::new);
            }
            &mut self.lines[self.cur_line]
        }
    }

    impl Perform for InnerWriter {
        fn print(&mut self, c: char) {
            let mut buf = [0u8; 4];
            let len = c.len_utf8();
            c.encode_utf8(&mut buf);
            self.get_line().extend_from_slice(&buf[..len]);
        }

        fn execute(&mut self, c: u8) {
            const NEWLINE: u8 = '\n' as u8;
            match c {
                NEWLINE => {
                    self.cur_line += 1;
                    // Ensure the line is created in the buffer.
                    let _ = self.get_line();
                }
                c => panic!("unrecognized control char: {c}"),
            }
        }

        fn csi_dispatch(
            &mut self,
            params: &vte::Params,
            _intermediates: &[u8],
            ignore: bool,
            action: char,
        ) {
            assert!(!ignore);
            match action {
                // Cursor up.
                'A' => {
                    let param = params.iter().next().expect("missing param");
                    let param = assert_matches!(param, [p] => *p);
                    self.cur_line = self.cur_line.checked_sub(param.into()).expect("bad cursor up");
                }
                // Clear line.
                'K' => {
                    self.get_line().clear();
                }
                'm' => {}
                c => panic!("unrecognized csi sequence: {c}"),
            }
        }

        fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
            unimplemented!("escape sequences not implemented: {intermediates:?}, {byte}")
        }
    }

    #[test]
    fn renderer_idle() {
        let mut renderer = Renderer::default();
        let mut writer = TestWriter::default();
        let counters = Counters::default();
        renderer.render(counters, INTERVAL, &mut writer).unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "(0) → Host to Target ⇋ (0) ← Target to Host\n\
            → |     | 0.0 bps / 0.0 B\n\
            ← |     | 0.0 bps / 0.0 B\n"
        );
        renderer.render(counters, INTERVAL, &mut writer).unwrap();
    }

    #[test]
    fn renderer_traffic() {
        let mut renderer = Renderer::default();
        let mut writer = TestWriter::default();

        let mut start_counters = Counters {
            active_connections: Bidirectional { host_to_target: 5, target_to_host: 2 },
            total_bytes: Default::default(),
        };
        let mut counters = |host_to_target, target_to_host| {
            start_counters.total_bytes.host_to_target += host_to_target;
            start_counters.total_bytes.target_to_host += target_to_host;
            start_counters.clone()
        };

        // Don't have a delta to calculate in the first round, so there's no
        // throughput info.
        renderer.render(counters(100, 200), INTERVAL, &mut writer).unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "(5) → Host to Target ⇋ (2) ← Target to Host\n\
            → |     | 0.0 bps / 100.0 B\n\
            ← |     | 0.0 bps / 200.0 B\n"
        );
        renderer.render(counters(100, 200), INTERVAL, &mut writer).unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "(5) → Host to Target ⇌ (2) ← Target to Host\n\
            → |    ▄| 800.0 bps / 200.0 B\n\
            ← |    ▇| 1.6 Kbps / 400.0 B\n"
        );
        renderer.render(counters(0, 0), INTERVAL, &mut writer).unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "(5) → Host to Target ⇋ (2) ← Target to Host\n\
            → |   ▄ | 0.0 bps / 200.0 B\n\
            ← |   ▇ | 0.0 bps / 400.0 B\n"
        );
        renderer.render(counters(400, 200), INTERVAL, &mut writer).unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "(5) → Host to Target ⇌ (2) ← Target to Host\n\
            → |  ▂ ▇| 3.2 Kbps / 600.0 B\n\
            ← |  ▄ ▄| 1.6 Kbps / 600.0 B\n"
        );
        renderer.render(counters(40, 20), INTERVAL, &mut writer).unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "(5) → Host to Target ⇋ (2) ← Target to Host\n\
            → | ▂ ▇▁| 320.0 bps / 640.0 B\n\
            ← | ▄ ▄▁| 160.0 bps / 620.0 B\n"
        );
        renderer.render(counters(0, 0), INTERVAL, &mut writer).unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "(5) → Host to Target ⇌ (2) ← Target to Host\n\
            → |▂ ▇▁ | 0.0 bps / 640.0 B\n\
            ← |▄ ▄▁ | 0.0 bps / 620.0 B\n"
        );
        renderer.render(counters(0, 0), INTERVAL, &mut writer).unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "(5) → Host to Target ⇋ (2) ← Target to Host\n\
            → | ▇▁  | 0.0 bps / 640.0 B\n\
            ← | ▄▁  | 0.0 bps / 620.0 B\n"
        );
    }

    #[test]
    fn rendered_error() {
        let mut renderer = Renderer::default();
        let mut writer = TestWriter::default();

        renderer.render_disconnected(Some(Duration::from_secs(1)), None, &mut writer).unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "Connection lost. Reconnecting in 1s...\n"
        );
        renderer.render_disconnected(Some(Duration::from_secs(0)), None, &mut writer).unwrap();
        pretty_assertions::assert_eq!(writer.to_string(), "Reconnecting to target...\n");
        renderer
            .render_disconnected(
                Some(Duration::from_secs(2)),
                Some(&ffx_error!("oops").into()),
                &mut writer,
            )
            .unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "Connection lost. Reconnecting in 2s...\n\
            Last error: oops\n"
        );
    }

    #[test]
    fn connected_to_error_and_back() {
        let mut renderer = Renderer::default();
        let mut writer = TestWriter::default();

        renderer.render(Default::default(), INTERVAL, &mut writer).unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "(0) → Host to Target ⇋ (0) ← Target to Host\n\
            → |     | 0.0 bps / 0.0 B\n\
            ← |     | 0.0 bps / 0.0 B\n"
        );
        renderer.render_disconnected(None, None, &mut writer).unwrap();
        pretty_assertions::assert_eq!(writer.to_string(), "Reconnecting to target...\n\n\n");
        renderer.render(Default::default(), INTERVAL, &mut writer).unwrap();
        pretty_assertions::assert_eq!(
            writer.to_string(),
            "(0) → Host to Target ⇌ (0) ← Target to Host\n\
            → |     | 0.0 bps / 0.0 B\n\
            ← |     | 0.0 bps / 0.0 B\n"
        );
    }

    #[test]
    fn bar_renderer() {
        let mut bar = BarRenderer::default();
        bar.update(1.0, 1, 1.0);
        assert_eq!(bar.to_string(), "▇");
        bar.update(0.5, 5, 1.0);
        assert_eq!(bar.to_string(), "   ▇▄");
        bar.update(0.0, 5, 0.5);
        assert_eq!(bar.to_string(), "  ▇▇ ");
        bar.update(1.0, 2, 1.0);
        assert_eq!(bar.to_string(), " ▇");
        bar.update(0.00001, 2, 1.0);
        assert_eq!(bar.to_string(), "▇▁");
        bar.update(20.0, 2, 1.0);
        assert_eq!(bar.to_string(), "▁▇");
    }

    #[test]
    fn forward_message_display_single_spec_h2t() {
        let spec = ForwardSpec {
            host: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8080")),
            target: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8081")),
            direction: Direction::HostToTarget,
        };
        let message = ForwardMessage::Forwarding(vec![spec]);
        pretty_assertions::assert_eq!(
            message.to_string(),
            "Forwarding: \n\ttcp:127.0.0.1:8080=>tcp:127.0.0.1:8081"
        );
    }

    #[test]
    fn forward_message_display_single_spec_t2h() {
        let spec = ForwardSpec {
            host: ProtoSpec::Tcp(std_socket_addr!("192.168.1.1:22")),
            target: ProtoSpec::Tcp(std_socket_addr!("10.0.0.1:23")),
            direction: Direction::TargetToHost,
        };
        let message = ForwardMessage::Forwarding(vec![spec]);
        pretty_assertions::assert_eq!(
            message.to_string(),
            "Forwarding: \n\ttcp:192.168.1.1:22<=tcp:10.0.0.1:23"
        );
    }

    #[test]
    fn forward_message_display_multiple_specs() {
        let spec1 = ForwardSpec {
            host: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8080")),
            target: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8081")),
            direction: Direction::HostToTarget,
        };
        let spec2 = ForwardSpec {
            host: ProtoSpec::Tcp(std_socket_addr!("192.168.1.1:22")),
            target: ProtoSpec::Tcp(std_socket_addr!("10.0.0.1:23")),
            direction: Direction::TargetToHost,
        };
        let spec3 = ForwardSpec {
            host: ProtoSpec::Tcp(std_socket_addr!("[::1]:9000")),
            target: ProtoSpec::Tcp(std_socket_addr!("[::1]:9001")),
            direction: Direction::HostToTarget,
        };
        let message = ForwardMessage::Forwarding(vec![spec1, spec2, spec3]);
        pretty_assertions::assert_eq!(
            message.to_string(),
            "Forwarding: \n\ttcp:127.0.0.1:8080=>tcp:127.0.0.1:8081\n\ttcp:192.168.1.1:22<=tcp:10.0.0.1:23\n\ttcp:[::1]:9000=>tcp:[::1]:9001"
        );
    }

    #[fuchsia::test]
    async fn test_forwarder() -> fho::Result<()> {
        let fake_netstack = FakeNetstack::new(fdomain_local::local_client_empty());
        let socket = fake_netstack.new_socket_provider();
        let port_forward = PortForwarder::new(socket);

        let orig_specs = vec![
            ForwardSpec {
                host: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:0")),
                target: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:1414")),
                direction: Direction::HostToTarget,
            },
            ForwardSpec {
                host: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8084")),
                target: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:1515")),
                direction: Direction::TargetToHost,
            },
            ForwardSpec {
                host: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8085")),
                target: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:0")),
                direction: Direction::TargetToHost,
            },
        ];
        let forwarder = Forwarder::new_with_forwarder(&orig_specs, port_forward).await?;
        let specs = forwarder.forwarded_specs();

        let (spec0, spec1, spec2) = assert_matches!(&specs[..], [a,b,c] => (a,b,c));

        // The first port is going to be randomly chosen by the host,
        // dont assert on the full port.
        let ForwardSpec { host: host_0, target: target_0, direction: direction_0 } = &spec0;
        assert_eq!(target_0, &ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:1414")));
        assert_eq!(direction_0, &Direction::HostToTarget);
        let s = assert_matches!(host_0, ProtoSpec::Tcp(s)=> s);
        assert_ne!(s.port(), 0);
        assert_eq!(s.ip(), std_ip_v4!("127.0.0.1"));

        assert_eq!(
            spec1,
            &ForwardSpec {
                host: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8084")),
                target: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:1515")),
                direction: Direction::TargetToHost,
            }
        );

        // The target port is going to be randomly chosen by the target,
        // dont assert on the full port.
        let ForwardSpec { host: host_2, target: target_2, direction: direction_2 } = &spec2;
        assert_eq!(host_2, &ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8085")));
        assert_eq!(direction_2, &Direction::TargetToHost);
        let s = assert_matches!(target_2, ProtoSpec::Tcp(s)=> s);
        assert_ne!(s.port(), 0);
        assert_eq!(s.ip(), std_ip_v4!("127.0.0.1"));
        forwarder.shutdown().await;
        Ok(())
    }

    #[fuchsia::test]
    fn test_writer_schema() -> fho::Result<()> {
        let buffers = TestBuffers::default();
        let mut writer = <ForwardTool as FfxMain>::Writer::new_test(Some(Format::Json), &buffers);

        // Write a few items
        writer.item(&ForwardMessage::Forwarding(vec![ForwardSpec {
            host: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8000")),
            target: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:1515")),
            direction: Direction::TargetToHost,
        }]))?;
        writer.item(&ForwardMessage::Forwarding(vec![ForwardSpec {
            host: ProtoSpec::Tcp(std_socket_addr!("[::1]:8090")),
            target: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:1432")),
            direction: Direction::TargetToHost,
        }]))?;
        writer.item(&ForwardMessage::Forwarding(vec![ForwardSpec {
            host: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8085")),
            target: ProtoSpec::Tcp(std_socket_addr!("[::]:1588")),
            direction: Direction::HostToTarget,
        }]))?;

        let output = buffers.into_stdout_str();

        let messages: Vec<_> = output.split("\n").collect();
        assert_eq!(messages.len(), 4);
        for i in 0..(messages.len() - 1) {
            let message = messages[i];
            eprintln!("{message}");
            let err = format!("schema not valid {message}");
            let json = serde_json::from_str(&message).expect(&err);
            <ForwardTool as FfxMain>::Writer::verify_schema(&json).expect(&err);
        }
        Ok(())
    }

    #[fuchsia::test]
    fn test_message_representation() {
        let repr = r#"{"forwarding":[{"host":{"Tcp":"[::]:8084"},"target":{"Tcp":"127.0.0.1:1515"},"direction":"TargetToHost"}]}
{"forwarding":[{"host":{"Tcp":"[::1]:9090"},"target":{"Tcp":"127.0.0.1:8765"},"direction":"HostToTarget"}]}
{"forwarding":[{"host":{"Tcp":"127.0.0.1:8888"},"target":{"Tcp":"[::]:7890"},"direction":"TargetToHost"}]}"#;
        let items = vec![
            ForwardMessage::Forwarding(vec![ForwardSpec {
                host: ProtoSpec::Tcp(std_socket_addr!("[::]:8084")),
                target: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:1515")),
                direction: Direction::TargetToHost,
            }]),
            ForwardMessage::Forwarding(vec![ForwardSpec {
                host: ProtoSpec::Tcp(std_socket_addr!("[::1]:9090")),
                target: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8765")),
                direction: Direction::HostToTarget,
            }]),
            ForwardMessage::Forwarding(vec![ForwardSpec {
                host: ProtoSpec::Tcp(std_socket_addr!("127.0.0.1:8888")),
                target: ProtoSpec::Tcp(std_socket_addr!("[::]:7890")),
                direction: Direction::TargetToHost,
            }]),
        ];
        pretty_assertions::assert_eq!(repr, items.into_iter().map(|i| json!(i)).join("\n"));
    }
}
