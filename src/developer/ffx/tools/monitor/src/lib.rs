// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
mod args;
use args::{MonitorCommand, SubCommand};

use anyhow::{Context, Result};
use async_trait::async_trait;
use ffx_config::EnvironmentContext;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxMain, FfxTool};
use http_body_util::{BodyExt as _, Full};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
pub type Body = Full<hyper::body::Bytes>;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::fs;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;
use target_formatter::{JsonTarget, JsonTargetFormatter};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio::task::spawn_blocking;

use crate::args::StartCommand;

// Default value of these can be found in //src/developer/ffx/data/config.json
const CONFIG_PID_FILE: &str = "monitor.pid_file";
const CONFIG_PORT_FILE: &str = "monitor.port_file";

const LOCAL_SERVER_IP_ADDRESS: &str = "127.0.0.1";
const LOCAL_SERVER_IP_ADDRESS_ARRAY: [u8; 4] = [127, 0, 0, 1];
const DEFAULT_MONITOR_PORT: u16 = 8080;
const LOG_VERSION: u16 = 3;
const AGGREGATIONS_VERSION: u16 = 2;

#[derive(FfxTool)]
#[target(None)]
pub struct MonitorTool {
    #[command]
    cmd: MonitorCommand,

    context: EnvironmentContext,
}

type Cache = Arc<Mutex<HashMap<String, serde_json::Value>>>;
type LogSender = mpsc::UnboundedSender<LogMessage>;

struct LogContext {
    sender: LogSender,
    log_path: Option<Arc<PathBuf>>,
    aggregations_path: Option<Arc<PathBuf>>,
}

#[derive(Serialize, Debug, Clone)]
struct IntentionalDisconnectRecord {
    nodename: String,
    timestamp: u64,
}

#[derive(Serialize, Debug)]
struct Aggregations {
    version: u16,
    summary: Summary,
    device_aggregations: Vec<DeviceAggregation>,
}

#[derive(Serialize, Debug, Default)]
struct Summary {
    total_devices: usize,
    start_time: u64,
    duration_ms: u64,
    latency: LatencySummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_type: Option<String>,
}

#[derive(Serialize, Debug, Default)]
struct LatencySummary {
    min: u64,
    avg: u64,
    p90: u64,
    p95: u64,
    max: u64,
    count: usize,
}

#[derive(Serialize, Debug)]
struct DeviceAggregation {
    id: String,
    nodename: String,
    target_type: String,
    metrics: DeviceMetricsOutput,
}

#[derive(Serialize, Debug)]
struct DeviceMetricsOutput {
    active_ms: u64,
    inactive_ms: u64,
    disconnected_ms: u64,
    rcs_dropouts: u64,
    target_vanished_count: u64,
    intentional_target_vanished_count: u64,
    unexpected_target_vanished_count: u64,
    intentional_disconnected_ms: u64,
    unexpected_disconnected_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DisconnectType {
    Intentional,
    Unexpected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeviceState {
    Active,
    Inactive,
    Disconnected,
}

#[derive(Debug, Clone)]
struct DeviceMetrics {
    active_ms: u64,
    inactive_ms: u64,
    disconnected_ms: u64,
    rcs_dropouts: u64,
    target_vanished_count: u64,
    intentional_target_vanished_count: u64,
    unexpected_target_vanished_count: u64,
    intentional_disconnected_ms: u64,
    unexpected_disconnected_ms: u64,
    last_seen_state: Option<DeviceState>,
    last_disconnect_type: Option<DisconnectType>,
    target_type: String,
}
#[derive(Serialize, Debug, Clone)]
struct LogEntry {
    targets: Vec<JsonTarget>,
    timestamp: u64,
    latency: u64,
}

#[derive(Default, Clone)]
struct AggregationsState {
    start_time: Option<u64>,
    last_time: u64,
    latencies: Vec<u64>,
    devices: HashMap<String, DeviceMetrics>,
    prev_time: u64,
    /// A set of target nodenames that are expected to disconnect intentionally.
    pending_intentional_disconnects: HashSet<String>,
}

impl AggregationsState {
    fn update(&mut self, entry: &LogEntry) {
        if self.start_time.is_none() {
            self.start_time = Some(entry.timestamp);
            self.prev_time = entry.timestamp;
        }
        self.last_time = entry.timestamp;
        self.latencies.push(entry.latency);

        let delta = entry.timestamp.saturating_sub(self.prev_time);

        for dev_metrics in self.devices.values_mut() {
            match dev_metrics.last_seen_state {
                Some(DeviceState::Active) => dev_metrics.active_ms += delta,
                Some(DeviceState::Inactive) => dev_metrics.inactive_ms += delta,
                Some(DeviceState::Disconnected) => {
                    dev_metrics.disconnected_ms += delta;
                    match dev_metrics.last_disconnect_type {
                        Some(DisconnectType::Intentional) => {
                            dev_metrics.intentional_disconnected_ms += delta
                        }
                        Some(DisconnectType::Unexpected) => {
                            dev_metrics.unexpected_disconnected_ms += delta
                        }
                        None => {}
                    }
                }
                None => {}
            }
        }

        let mut current_nodenames = HashSet::new();
        for target in &entry.targets {
            let target_val = serde_json::to_value(target).unwrap();
            let name = target_val["nodename"].as_str().unwrap_or("<unknown>").to_string();
            current_nodenames.insert(name.clone());

            let is_active = target_val["rcs_state"].as_str().unwrap_or("N") == "Y";

            let dev_metrics = self.devices.entry(name).or_insert_with(|| {
                let target_type =
                    target_val["target_type"].as_str().unwrap_or("Unknown").to_string();
                DeviceMetrics {
                    active_ms: 0,
                    inactive_ms: 0,
                    disconnected_ms: 0,
                    rcs_dropouts: 0,
                    target_vanished_count: 0,
                    intentional_target_vanished_count: 0,
                    unexpected_target_vanished_count: 0,
                    intentional_disconnected_ms: 0,
                    unexpected_disconnected_ms: 0,
                    last_seen_state: None,
                    last_disconnect_type: None,
                    target_type,
                }
            });

            if dev_metrics.last_seen_state == Some(DeviceState::Active) && !is_active {
                dev_metrics.rcs_dropouts += 1;
            }

            dev_metrics.last_seen_state =
                Some(if is_active { DeviceState::Active } else { DeviceState::Inactive });
            dev_metrics.last_disconnect_type = None;
        }

        for (name, dev_metrics) in self.devices.iter_mut() {
            if !current_nodenames.contains(name) {
                match dev_metrics.last_seen_state {
                    Some(DeviceState::Active) | Some(DeviceState::Inactive) => {
                        dev_metrics.target_vanished_count += 1;
                        if self.pending_intentional_disconnects.remove(name) {
                            dev_metrics.intentional_target_vanished_count += 1;
                            dev_metrics.last_disconnect_type = Some(DisconnectType::Intentional);
                        } else {
                            dev_metrics.unexpected_target_vanished_count += 1;
                            dev_metrics.last_disconnect_type = Some(DisconnectType::Unexpected);
                        }
                        dev_metrics.last_seen_state = Some(DeviceState::Disconnected);
                    }
                    _ => {}
                }
            }
        }

        self.prev_time = entry.timestamp;
    }

    fn finalize(&self) -> Aggregations {
        let start_time = self.start_time.unwrap_or(0);
        let duration_ms = self.last_time.saturating_sub(start_time);

        let mut sorted_latencies = self.latencies.clone();
        sorted_latencies.sort_unstable();

        let count = sorted_latencies.len();
        let latency_summary = if count > 0 {
            let sum: u64 = sorted_latencies.iter().sum();
            let p90_idx = ((count as f64) * 0.90).floor() as usize;
            let p95_idx = ((count as f64) * 0.95).floor() as usize;
            let p90_idx = p90_idx.min(count.saturating_sub(1));
            let p95_idx = p95_idx.min(count.saturating_sub(1));
            LatencySummary {
                min: sorted_latencies[0],
                max: sorted_latencies[count - 1],
                avg: sum / (count as u64),
                p90: sorted_latencies[p90_idx],
                p95: sorted_latencies[p95_idx],
                count,
            }
        } else {
            LatencySummary::default()
        };

        let mut device_aggregations = Vec::new();
        let mut sorted_devices: Vec<_> = self.devices.iter().collect();
        sorted_devices.sort_by(|a, b| a.0.cmp(b.0));

        for (i, (nodename, metrics)) in sorted_devices.into_iter().enumerate() {
            device_aggregations.push(DeviceAggregation {
                id: format!("target_{}", i),
                nodename: nodename.clone(),
                target_type: metrics.target_type.clone(),
                metrics: DeviceMetricsOutput {
                    active_ms: metrics.active_ms,
                    inactive_ms: metrics.inactive_ms,
                    disconnected_ms: metrics.disconnected_ms,
                    rcs_dropouts: metrics.rcs_dropouts,
                    target_vanished_count: metrics.target_vanished_count,
                    intentional_target_vanished_count: metrics.intentional_target_vanished_count,
                    unexpected_target_vanished_count: metrics.unexpected_target_vanished_count,
                    intentional_disconnected_ms: metrics.intentional_disconnected_ms,
                    unexpected_disconnected_ms: metrics.unexpected_disconnected_ms,
                },
            });
        }

        Aggregations {
            version: AGGREGATIONS_VERSION,
            summary: Summary {
                total_devices: device_aggregations.len(),
                start_time,
                duration_ms,
                latency: latency_summary,
                device_type: std::env::var("FUCHSIA_DEVICE_TYPE").ok(),
            },
            device_aggregations,
        }
    }
}

enum LogMessage {
    Log(LogEntry),
    Flush(Option<Arc<PathBuf>>, Option<Arc<PathBuf>>, oneshot::Sender<anyhow::Result<()>>),
    IntentionalDisconnect(String, u64),
}

/// Resolves a log file path.
///
/// If the provided `file_path` is absolute, it's used as is.
/// If it's relative, it's joined with the provided `log_dir`.
/// If `file_path` is relative but `log_dir` is not set, an error is returned.
fn get_log_path(
    file_path: &Option<String>,
    log_dir: &Option<String>,
) -> Result<Option<Arc<PathBuf>>> {
    match (file_path, log_dir) {
        (None, _) => Ok(None),
        (Some(p), _) if Path::new(p).is_absolute() => Ok(Some(Arc::new(PathBuf::from(p)))),
        (Some(_), None) => {
            Err(anyhow::anyhow!("log.dir is not set but is required for relative log file paths"))
        }
        (Some(p), Some(dir)) => Ok(Some(Arc::new(PathBuf::from(dir).join(p)))),
    }
}

async fn start_server(
    context: &EnvironmentContext,
    addr: SocketAddr,
    cmd: StartCommand,
    writer: &mut MachineWriter<serde_json::Value>,
    pid_file_path: &str,
) -> anyhow::Result<()> {
    if cmd.aggregations_file.is_some() && cmd.log_file.is_none() {
        return Err(anyhow::anyhow!(
            "--aggregations-file can only be used when --log-file is also specified"
        ));
    }

    let log_dir: Option<String> = context.get("log.dir").ok();
    let log_path = get_log_path(&cmd.log_file, &log_dir)?;
    let aggregations_path = get_log_path(&cmd.aggregations_file, &log_dir)?;

    let log_context = if log_path.is_some() {
        let ensure_parent = |p: &Option<Arc<PathBuf>>| {
            if let Some(path) = p {
                if let Some(parent) = path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
            }
        };
        ensure_parent(&log_path);
        ensure_parent(&aggregations_path);

        let (tx, rx) = mpsc::unbounded_channel::<LogMessage>();
        tokio::spawn(run_log_manager(rx));
        Some(Arc::new(LogContext { sender: tx, log_path, aggregations_path }))
    } else {
        None
    };

    let target_status_cache = Arc::new(Mutex::new(HashMap::new()));
    let cache_for_task = target_status_cache.clone();
    let log_sender_for_task = log_context.as_ref().map(|c| c.sender.clone());
    let context_clone = context.clone();
    tokio::spawn(async move {
        loop {
            let ctx = context_clone.clone();
            let cmd_clone = cmd.clone();
            let start = std::time::Instant::now();
            let res = spawn_blocking(move || {
                fuchsia_async::LocalExecutor::default()
                    .run_singlethreaded(collect_target_status(&ctx, cmd_clone))
            })
            .await;
            let latency_ms = start.elapsed().as_millis() as u64;

            match res {
                Ok(Ok(statuses)) => {
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;

                    let mut cache_lock = cache_for_task.lock().await;
                    let json_value = serde_json::to_value(&statuses).unwrap();
                    cache_lock.insert("targets".to_owned(), json_value);
                    log::debug!("Successfully updated target status cache {:?}", cache_lock);

                    if let Some(sender) = &log_sender_for_task {
                        let _ = sender.send(LogMessage::Log(LogEntry {
                            targets: statuses,
                            timestamp,
                            latency: latency_ms,
                        }));
                    }
                }
                Ok(Err(e)) => {
                    log::error!("Error collecting target status: {:?}", e);
                }
                Err(e) => {
                    log::error!("Task panicked while collecting target status: {:?}", e);
                }
            }
            sleep(Duration::from_secs(1));
        }
    });

    let log_context_signal = log_context.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use futures::FutureExt;
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
            let mut sigint =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).unwrap();
            futures::future::select(
                Box::pin(sigterm.recv().map(|_| ())),
                Box::pin(sigint.recv().map(|_| ())),
            )
            .await;
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await;
        }

        if let Some(ctx) = log_context_signal {
            if let Err(e) = flush_logs(&ctx).await {
                log::error!("Failed to flush logs on stop signal: {:?}", e);
            }
        }
        std::process::exit(0);
    });

    let pid = std::process::id();
    writeln!(writer, "Starting server on http://{} with pid {}", addr, pid)
        .context("writing start message")?;

    let listener = TcpListener::bind(addr).await.context("binding to address")?;

    // Record PID in config file after server successfully started
    if let Some(parent) = Path::new(&pid_file_path).parent() {
        fs::create_dir_all(parent).context("creating pid file directory")?;
    }
    fs::write(&pid_file_path, pid.to_string()).context("writing pid file")?;

    loop {
        let (stream, _) = listener.accept().await.context("accepting connection")?;
        let cache_for_handler = target_status_cache.clone();
        let log_context_for_handler = log_context.clone();

        let builder = hyper_util::server::conn::auto::Builder::new(fuchsia_hyper::Executor);
        tokio::task::spawn(async move {
            if let Err(err) = builder
                .serve_connection(
                    hyper_util::rt::TokioIo::new(stream),
                    service_fn(move |req| {
                        handle_request(
                            req,
                            cache_for_handler.clone(),
                            log_context_for_handler.clone(),
                        )
                    }),
                )
                .await
            {
                log::error!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn collect_target_status(
    context: &EnvironmentContext,
    cmd: StartCommand,
) -> Result<Vec<JsonTarget>> {
    let query = ffx_target::TargetInfoQuery::try_from(cmd.nodename.clone())?;

    let infos =
        ffx_target::list_targets(context, query, !cmd.no_usb, !cmd.no_mdns, !cmd.no_probe).await?;
    let formatter = JsonTargetFormatter::try_from(infos)?;
    Ok(formatter.targets)
}

async fn flush_logs(ctx: &Arc<LogContext>) -> anyhow::Result<()> {
    let (tx, finished_signal) = tokio::sync::oneshot::channel();
    let _ =
        ctx.sender.send(LogMessage::Flush(ctx.log_path.clone(), ctx.aggregations_path.clone(), tx));
    match finished_signal.await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(anyhow::anyhow!("Failed to flush logs: {:?}", e)),
        Err(e) => Err(anyhow::anyhow!("Failed to receive flush confirmation: {:?}", e)),
    }
}

async fn handle_request<B>(
    req: Request<B>,
    cache: Cache,
    log_context: Option<Arc<LogContext>>,
) -> std::result::Result<Response<Body>, Infallible>
where
    B: hyper::body::Body,
    B::Error: std::fmt::Debug + std::fmt::Display,
{
    let mut response = Response::new(Full::new("".into()));
    match req.uri().path() {
        "/intentional_disconnect" => {
            if req.method() == hyper::Method::POST {
                let body = req.into_body().collect().await.map(|b| b.to_bytes());
                match body {
                    Ok(bytes) => {
                        if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                            if let Some(nodename) = json["nodename"].as_str() {
                                if let Some(ctx) = log_context {
                                    let timestamp = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap()
                                        .as_millis()
                                        as u64;
                                    let _ = ctx.sender.send(LogMessage::IntentionalDisconnect(
                                        nodename.to_string(),
                                        timestamp,
                                    ));
                                }
                                *response.body_mut() = "OK".into();
                            } else {
                                *response.status_mut() = StatusCode::BAD_REQUEST;
                                *response.body_mut() = "Missing nodename".into();
                            }
                        } else {
                            *response.status_mut() = StatusCode::BAD_REQUEST;
                            *response.body_mut() = "Invalid JSON".into();
                        }
                    }
                    Err(e) => {
                        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                        *response.body_mut() = format!("Failed to read body: {}", e).into();
                    }
                }
            } else {
                *response.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
            }
        }
        "/status" => {
            let statuses = cache.lock().await;
            match serde_json::to_string(&*statuses) {
                Ok(body) => {
                    *response.body_mut() = body.into();
                    response
                        .headers_mut()
                        .insert(hyper::header::CONTENT_TYPE, "application/json".parse().unwrap());
                }
                Err(e) => {
                    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                    *response.body_mut() = format!("Internal Server Error: {}", e).into();
                }
            }
        }
        "/stop" => {
            if let Some(ctx) = log_context {
                match flush_logs(&ctx).await {
                    Ok(_) => {
                        *response.body_mut() = "OK".into();
                    }
                    Err(e) => {
                        log::error!("{}", e);
                        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                        *response.body_mut() = format!("{}", e).into();
                    }
                }
            } else {
                *response.body_mut() = "OK".into();
            }
        }
        _ => {
            *response.status_mut() = StatusCode::NOT_FOUND;
        }
    };
    Ok(response)
}

#[async_trait(?Send)]
impl FfxMain for MonitorTool {
    type Writer = MachineWriter<serde_json::Value>;

    type Error = ::fho::Error;

    async fn main(self, mut writer: <Self as FfxMain>::Writer) -> fho::Result<()> {
        let pid_file_path: String = self
            .context
            .get(CONFIG_PID_FILE)
            .map_err(|e| fho::Error::from(anyhow::anyhow!("Failed to get pid file path: {}", e)))?;
        let port_file_path: String = self.context.get(CONFIG_PORT_FILE).map_err(|e| {
            fho::Error::from(anyhow::anyhow!("Failed to get port file path: {}", e))
        })?;
        match self.cmd.subcommand {
            SubCommand::Start(cmd) => {
                let port = match cmd.port {
                    Some(port) => port,
                    None => DEFAULT_MONITOR_PORT,
                };

                let addr = SocketAddr::from((LOCAL_SERVER_IP_ADDRESS_ARRAY, port));

                // Record port in config file
                if let Some(parent) = Path::new(&port_file_path).parent() {
                    fs::create_dir_all(parent).context("creating pid file directory")?;
                }
                fs::write(&port_file_path, port.to_string()).context("writing port file")?;
                start_server(&self.context, addr, cmd, &mut writer, &pid_file_path)
                    .await
                    .map_err(fho::Error::from)
            }
            SubCommand::Stop(_) => {
                let port_str = fs::read_to_string(&port_file_path).context("reading port file")?;
                let port: u16 = port_str.trim().parse().context("parsing port")?;

                let url = format!("http://{}:{}/stop", LOCAL_SERVER_IP_ADDRESS, port);
                let client = fuchsia_hyper::new_client();
                let uri = url.parse::<hyper::Uri>().context("parsing uri")?;
                let req = Request::builder()
                    .method(hyper::Method::POST)
                    .uri(uri)
                    .body(Full::new(hyper::body::Bytes::new()))
                    .unwrap();

                if let Err(e) = client.request(req).await {
                    log::warn!("Failed to send stop request to server: {}", e);
                } else {
                    writeln!(writer, "Successfully sent stop request to server")
                        .context("send stop request")?;
                }

                let pid_str = fs::read_to_string(&pid_file_path).context("reading pid file")?;
                let pid: i32 = pid_str.trim().parse().context("parsing pid")?;

                writeln!(writer, "Stopping server with pid {}", pid)
                    .context("writing stop message")?;
                Command::new("kill").arg(pid.to_string()).status().context("killing process")?;
                fs::remove_file(pid_file_path).context("removing pid file")?;
                Ok(())
            }
            SubCommand::Status(_) => {
                let port_str = fs::read_to_string(&port_file_path).context("reading port file")?;
                let port: u16 = port_str.trim().parse().context("parsing port")?;

                let url = format!("http://{}:{}/status", LOCAL_SERVER_IP_ADDRESS, port);
                let client = fuchsia_hyper::new_client();
                let response = client
                    .get(
                        url.parse()
                            .map_err(|e: hyper::http::uri::InvalidUri| anyhow::anyhow!(e))?,
                    )
                    .await
                    .context("sending request")?;

                let body = response
                    .into_body()
                    .collect()
                    .await
                    .context("reading response body")?
                    .to_bytes();
                let json: serde_json::Value =
                    serde_json::from_slice(&body).context("parsing json")?;

                if writer.is_machine() {
                    writer.machine(&json)?;
                } else {
                    let pretty_json =
                        serde_json::to_string_pretty(&json).context("formatting json")?;
                    writeln!(writer, "{}", pretty_json).context("writing response to writer")?;
                }
                Ok(())
            }
            SubCommand::IntentionalDisconnect(cmd) => {
                let port_str = fs::read_to_string(&port_file_path).context("reading port file")?;
                let port: u16 = port_str.trim().parse().context("parsing port")?;

                let url =
                    format!("http://{}:{}/intentional_disconnect", LOCAL_SERVER_IP_ADDRESS, port);
                let client = fuchsia_hyper::new_client();
                let body = serde_json::json!({ "nodename": cmd.nodename }).to_string();
                let req = Request::builder()
                    .method(hyper::Method::POST)
                    .uri(url.parse::<hyper::Uri>().context("parsing uri")?)
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap();

                if let Err(e) = client.request(req).await {
                    log::warn!("Failed to send disconnect request to server: {}", e);
                } else {
                    writeln!(writer, "Successfully sent disconnect request to server")
                        .context("send disconnect request")?;
                }
                Ok(())
            }
        }
    }
}

/// Runs the log manager task which accumulates logs in memory and flushes them to disk upon request.
///
/// This task listens for `LogMessage`s on the provided receiver.
/// - `LogMessage::Log(entry)`: Appends the log entry to the in-memory buffer.
/// - `LogMessage::Flush(path, reply)`: Flushes the accumulated logs to the specified path as a JSON file,
///   overwriting any existing file. The in-memory buffer is cleared after flushing.
async fn run_log_manager(mut log_receiver: mpsc::UnboundedReceiver<LogMessage>) {
    let mut logs: Vec<LogEntry> = Vec::new();
    let mut intentional_disconnect_records: Vec<IntentionalDisconnectRecord> = Vec::new();
    let mut state = AggregationsState::default();
    while let Some(msg) = log_receiver.recv().await {
        match msg {
            LogMessage::Log(entry) => {
                state.update(&entry);
                logs.push(entry);
            }
            LogMessage::IntentionalDisconnect(nodename, timestamp) => {
                intentional_disconnect_records
                    .push(IntentionalDisconnectRecord { nodename: nodename.clone(), timestamp });
                state.pending_intentional_disconnects.insert(nodename);
            }
            LogMessage::Flush(log_path, aggregations_path, reply) => {
                let logs_to_write = logs.clone();
                let intentional_disconnects_to_write = intentional_disconnect_records.clone();
                let current_state = state.clone();
                let res = spawn_blocking(move || {
                    if let Some(path) = log_path {
                        let json = serde_json::json!({
                            "data": logs_to_write,
                            "intentional_disconnects": intentional_disconnects_to_write,
                            "version": LOG_VERSION,
                        });
                        let json_str = serde_json::to_string_pretty(&json)?;
                        fs::write(&*path, json_str)?;
                    }

                    if let Some(path) = aggregations_path {
                        let aggs = current_state.finalize();
                        let json_str = serde_json::to_string_pretty(&aggs)?;
                        fs::write(&*path, json_str)?;
                    }
                    Ok::<(), anyhow::Error>(())
                })
                .await;
                let _ = reply.send(match res {
                    Ok(inner_res) => inner_res,
                    Err(e) => Err(anyhow::anyhow!("JoinError: {}", e)),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_target(nodename: &str, rcs_state: &str) -> JsonTarget {
        serde_json::from_value(serde_json::json!({
            "nodename": nodename,
            "rcs_state": rcs_state,
            "serial": "serial",
            "target_state": "Product",
            "target_type": "smart_display_m3_eng.nelson",
            "addresses": [
                {
                    "type": "Ip",
                    "ip": "1.2.3.4",
                    "ssh_port": 0
                }
            ],
            "is_default": true,
            "is_manual": false,
        }))
        .unwrap()
    }

    fn make_entry(timestamp: u64, latency: u64, targets: Vec<JsonTarget>) -> LogEntry {
        LogEntry { targets, timestamp, latency }
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_log_manager() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let log_path = dir.path().join("log.json");
        let (tx, rx) = mpsc::unbounded_channel();

        fuchsia_async::Task::local(run_log_manager(rx)).detach();

        tx.send(LogMessage::Log(make_entry(
            1000,
            500,
            vec![make_target("fuchsia-1234-5678-abcd", "Y")],
        )))?;
        tx.send(LogMessage::Log(make_entry(
            2000,
            500,
            vec![make_target("fuchsia-1234-5678-abcd", "Y")],
        )))?;

        // Send intentional disconnect request
        tx.send(LogMessage::IntentionalDisconnect("fuchsia-1234-5678-abcd".to_string(), 2500))?;

        let (flush_tx, flush_rx) = oneshot::channel();
        let aggregations_path = dir.path().join("aggregations.json");
        tx.send(LogMessage::Flush(
            Some(Arc::new(log_path.clone())),
            Some(Arc::new(aggregations_path.clone())),
            flush_tx,
        ))?;
        flush_rx.await??;

        // Verify first flush
        let content = fs::read_to_string(&log_path)?;
        let json: serde_json::Value = serde_json::from_str(&content)?;
        assert_eq!(json["version"], LOG_VERSION);
        let data = json["data"].as_array().unwrap();
        assert_eq!(data.len(), 2);

        assert_eq!(data[0]["timestamp"], 1000);
        assert_eq!(data[0]["targets"][0]["nodename"], "fuchsia-1234-5678-abcd");

        assert_eq!(data[1]["timestamp"], 2000);
        assert_eq!(data[1]["targets"][0]["nodename"], "fuchsia-1234-5678-abcd");

        // Verify intentional disconnects record in log.json
        let intentional_disconnects = json["intentional_disconnects"].as_array().unwrap();
        assert_eq!(intentional_disconnects.len(), 1);
        assert_eq!(intentional_disconnects[0]["nodename"], "fuchsia-1234-5678-abcd");
        assert_eq!(intentional_disconnects[0]["timestamp"], 2500);

        // Verify aggregations
        let aggs_content = fs::read_to_string(&aggregations_path)?;
        let aggs_json: serde_json::Value = serde_json::from_str(&aggs_content)?;
        assert_eq!(aggs_json["version"], AGGREGATIONS_VERSION);
        let aggs_summary = aggs_json["summary"].as_object().unwrap();
        assert_eq!(aggs_summary["duration_ms"], 1000);

        let target_aggs = aggs_json["device_aggregations"].as_array().unwrap();
        assert_eq!(target_aggs.len(), 1);
        assert_eq!(target_aggs[0]["nodename"], "fuchsia-1234-5678-abcd");
        assert_eq!(target_aggs[0]["metrics"]["active_ms"], 1000);
        assert_eq!(target_aggs[0]["metrics"]["inactive_ms"], 0);

        // Flush again with new data, verifying overwrite
        tx.send(LogMessage::Log(make_entry(
            3000,
            500,
            vec![make_target("fuchsia-1234-5678-abcd", "Y")],
        )))?;
        let (flush_tx, flush_rx) = oneshot::channel();
        tx.send(LogMessage::Flush(
            Some(Arc::new(log_path.clone())),
            Some(Arc::new(aggregations_path.clone())),
            flush_tx,
        ))?;
        flush_rx.await??;

        let content = fs::read_to_string(&log_path)?;
        let json: serde_json::Value = serde_json::from_str(&content)?;
        assert_eq!(json["version"], LOG_VERSION);
        let data = json["data"].as_array().unwrap();
        assert_eq!(data.len(), 3); // Should have all entries
        assert_eq!(data[2]["timestamp"], 3000);

        let aggs_content = fs::read_to_string(&aggregations_path)?;
        let aggs_json: serde_json::Value = serde_json::from_str(&aggs_content)?;
        assert_eq!(aggs_json["version"], AGGREGATIONS_VERSION);

        Ok(())
    }

    #[test]
    fn test_get_log_path() {
        let default_dir = Some("/default/dir".to_string());

        // Absolute path
        let abs_path = "/tmp/log.json";
        let res = get_log_path(&Some(abs_path.to_string()), &default_dir).unwrap();
        assert_eq!(res.unwrap().as_path(), Path::new(abs_path));

        // Relative path with log_dir
        let res = get_log_path(&Some("log.json".to_string()), &default_dir).unwrap();
        assert_eq!(res.unwrap().as_path(), Path::new("/default/dir/log.json"));

        // Relative path without log_dir should fail
        let res = get_log_path(&Some("log.json".to_string()), &None);
        assert!(res.is_err());

        // None path
        let res = get_log_path(&None, &default_dir).unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn test_compute_aggregations_reappearing_device() {
        let mut state = AggregationsState::default();
        state.update(&make_entry(0, 100, vec![make_target("A", "Y")]));
        state.update(&make_entry(1000, 100, vec![]));
        state.update(&make_entry(2000, 100, vec![make_target("A", "Y")]));

        let result = state.finalize();
        let dev_a = &result.device_aggregations[0];
        assert_eq!(dev_a.nodename, "A");
        assert_eq!(dev_a.metrics.target_vanished_count, 1);
        assert_eq!(dev_a.metrics.disconnected_ms, 1000);
        assert_eq!(dev_a.metrics.active_ms, 1000);
    }

    #[test]
    fn test_compute_aggregations_intentional_disconnect() {
        let mut state = AggregationsState::default();
        let mut intentional_disconnect_records = Vec::new();

        // Time 0: A is active
        state.update(&make_entry(0, 100, vec![make_target("A", "Y")]));

        // Time 1000: A drops unexpectedly
        state.update(&make_entry(1000, 100, vec![]));

        // Time 1500: A still dropped (Unexpected persistence)
        state.update(&make_entry(1500, 100, vec![]));

        // Time 2000: A recovers
        state.update(&make_entry(2000, 100, vec![make_target("A", "N")]));

        // Time 2500: Intentional disconnect requested (Record 1)
        intentional_disconnect_records
            .push(IntentionalDisconnectRecord { nodename: "A".to_string(), timestamp: 2500 });
        state.pending_intentional_disconnects.insert("A".to_string());

        // Time 2600: Redundant Intentional disconnect requested (Record 2)
        intentional_disconnect_records
            .push(IntentionalDisconnectRecord { nodename: "A".to_string(), timestamp: 2600 });
        state.pending_intentional_disconnects.insert("A".to_string());

        // Time 3000: A drops intentionally
        state.update(&make_entry(3000, 100, vec![]));

        // Time 3500: A still dropped (Intentional persistence)
        state.update(&make_entry(3500, 100, vec![]));

        // Time 4000: A recovers
        state.update(&make_entry(4000, 100, vec![make_target("A", "Y")]));

        // Time 5000: A drops unexpectedly
        state.update(&make_entry(5000, 100, vec![]));

        // Time 6000: A recovers
        state.update(&make_entry(6000, 100, vec![make_target("A", "Y")]));

        // Time 6500: Intentional disconnect requested (Record 3)
        intentional_disconnect_records
            .push(IntentionalDisconnectRecord { nodename: "A".to_string(), timestamp: 6500 });
        state.pending_intentional_disconnects.insert("A".to_string());

        // Time 7000: A drops intentionally
        state.update(&make_entry(7000, 100, vec![]));

        // Time 8000: A recovers
        state.update(&make_entry(8000, 100, vec![make_target("A", "N")]));

        // Time 9000: A drops unexpectedly
        state.update(&make_entry(9000, 100, vec![]));

        // Time 10000: End
        state.update(&make_entry(10000, 100, vec![]));

        let result = state.finalize();
        let dev_a = &result.device_aggregations[0];
        assert_eq!(dev_a.nodename, "A");
        assert_eq!(dev_a.metrics.target_vanished_count, 5);
        assert_eq!(dev_a.metrics.intentional_target_vanished_count, 2);
        assert_eq!(dev_a.metrics.unexpected_target_vanished_count, 3);

        // Expected Disconnected Durations:
        // 1000-2000: Unexpected (1000ms)
        // 3000-4000: Intentional (1000ms)
        // 5000-6000: Unexpected (1000ms)
        // 7000-8000: Intentional (1000ms)
        // 9000-10000: Unexpected (1000ms)
        assert_eq!(dev_a.metrics.disconnected_ms, 5000);
        assert_eq!(dev_a.metrics.unexpected_disconnected_ms, 3000);
        assert_eq!(dev_a.metrics.intentional_disconnected_ms, 2000);
        assert_eq!(
            dev_a.metrics.disconnected_ms,
            dev_a.metrics.unexpected_disconnected_ms + dev_a.metrics.intentional_disconnected_ms
        );

        // Verify intentional disconnect records (3 total, including redundant ones)
        assert_eq!(intentional_disconnect_records.len(), 3);
        assert_eq!(intentional_disconnect_records[0].timestamp, 2500);
        assert_eq!(intentional_disconnect_records[1].timestamp, 2600);
        assert_eq!(intentional_disconnect_records[2].timestamp, 6500);
    }

    #[fuchsia_async::run_singlethreaded(test)]
    async fn test_handle_request() -> anyhow::Result<()> {
        let cache = Arc::new(Mutex::new(HashMap::new()));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let log_context =
            Some(Arc::new(LogContext { sender: tx, log_path: None, aggregations_path: None }));

        // Test /status
        {
            let mut cache_lock = cache.lock().await;
            cache_lock.insert("targets".to_string(), serde_json::json!([]));
            drop(cache_lock);

            let req = Request::builder().uri("/status").body(Body::default()).unwrap();
            let resp = handle_request(req, cache.clone(), log_context.clone()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert_eq!(
                resp.headers().get(hyper::header::CONTENT_TYPE).unwrap(),
                "application/json"
            );
        }

        // Test /intentional_disconnect
        {
            let body = serde_json::json!({ "nodename": "test-device" }).to_string();
            let req = Request::builder()
                .method(hyper::Method::POST)
                .uri("/intentional_disconnect")
                .body(Body::from(body))
                .unwrap();
            let resp = handle_request(req, cache.clone(), log_context.clone()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let msg = rx.recv().await.unwrap();
            if let LogMessage::IntentionalDisconnect(nodename, _) = msg {
                assert_eq!(nodename, "test-device");
            } else {
                panic!("Expected IntentionalDisconnect message");
            }
        }

        // Test /intentional_disconnect - Missing nodename
        {
            let body = serde_json::json!({ "wrong_key": "test-device" }).to_string();
            let req = Request::builder()
                .method(hyper::Method::POST)
                .uri("/intentional_disconnect")
                .body(Body::from(body))
                .unwrap();
            let resp = handle_request(req, cache.clone(), log_context.clone()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }

        // Test /intentional_disconnect - Invalid JSON
        {
            let req = Request::builder()
                .method(hyper::Method::POST)
                .uri("/intentional_disconnect")
                .body(Body::from("not json"))
                .unwrap();
            let resp = handle_request(req, cache.clone(), log_context.clone()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        }

        // Test /intentional_disconnect - Wrong method
        {
            let req = Request::builder()
                .method(hyper::Method::GET)
                .uri("/intentional_disconnect")
                .body(Body::default())
                .unwrap();
            let resp = handle_request(req, cache.clone(), log_context.clone()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        }

        // Test /stop
        {
            let req = Request::builder()
                .method(hyper::Method::POST)
                .uri("/stop")
                .body(Body::default())
                .unwrap();
            let log_context_clone = log_context.clone();
            let cache_clone = cache.clone();

            let (resp, _) =
                futures::future::join(handle_request(req, cache_clone, log_context_clone), async {
                    match rx.recv().await {
                        Some(LogMessage::Flush(_, _, reply)) => {
                            reply.send(Ok(())).unwrap();
                        }
                        _ => panic!("Expected Flush message"),
                    }
                })
                .await;

            assert_eq!(resp.unwrap().status(), StatusCode::OK);
        }

        // Test /intentional_disconnect - Failed to read body
        {
            struct ErrorBody;
            impl hyper::body::Body for ErrorBody {
                type Data = hyper::body::Bytes;
                type Error = std::io::Error;
                fn poll_frame(
                    self: std::pin::Pin<&mut Self>,
                    _cx: &mut std::task::Context<'_>,
                ) -> std::task::Poll<Option<Result<hyper::body::Frame<Self::Data>, Self::Error>>>
                {
                    std::task::Poll::Ready(Some(Err(std::io::Error::other("Failed to read body"))))
                }
            }
            let req = Request::builder()
                .method(hyper::Method::POST)
                .uri("/intentional_disconnect")
                .body(ErrorBody)
                .unwrap();
            let resp = handle_request(req, cache.clone(), log_context.clone()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
            let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
            assert!(std::str::from_utf8(&body_bytes).unwrap().contains("Failed to read body"));
        }

        // Test Not Found
        {
            let req = Request::builder().uri("/unknown").body(Body::default()).unwrap();
            let resp = handle_request(req, cache.clone(), log_context.clone()).await.unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        }

        // Test with log_context = None
        {
            let req = Request::builder().uri("/status").body(Body::default()).unwrap();
            let resp = handle_request(req, cache.clone(), None).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);

            let body = serde_json::json!({ "nodename": "test-device" }).to_string();
            let req = Request::builder()
                .method(hyper::Method::POST)
                .uri("/intentional_disconnect")
                .body(Body::from(body))
                .unwrap();
            let resp = handle_request(req, cache.clone(), None).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);

            let req = Request::builder()
                .method(hyper::Method::POST)
                .uri("/stop")
                .body(Body::default())
                .unwrap();
            let resp = handle_request(req, cache.clone(), None).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        Ok(())
    }

    #[test]
    fn test_compute_aggregations() {
        let mut state = AggregationsState::default();
        state.update(&make_entry(0, 100, vec![make_target("A", "Y"), make_target("B", "Y")]));
        state.update(&make_entry(1000, 150, vec![make_target("A", "Y"), make_target("B", "N")]));
        state.update(&make_entry(2000, 120, vec![make_target("A", "N")]));
        state.update(&make_entry(3000, 110, vec![make_target("A", "Y"), make_target("C", "Y")]));

        let result = state.finalize();
        assert_eq!(result.version, AGGREGATIONS_VERSION);

        assert_eq!(result.summary.start_time, 0);
        assert_eq!(result.summary.duration_ms, 3000);
        assert_eq!(result.summary.total_devices, 3);

        let latency = &result.summary.latency;
        assert_eq!(latency.count, 4);
        assert_eq!(latency.min, 100);
        assert_eq!(latency.max, 150);
        assert_eq!(latency.avg, (100 + 150 + 120 + 110) / 4);
        assert_eq!(latency.p90, 150);
        assert_eq!(latency.p95, 150);

        let dev_a = &result.device_aggregations[0];
        assert_eq!(dev_a.nodename, "A");
        assert_eq!(dev_a.metrics.active_ms, 2000);
        assert_eq!(dev_a.metrics.inactive_ms, 1000);
        assert_eq!(dev_a.metrics.rcs_dropouts, 1);
        assert_eq!(dev_a.metrics.target_vanished_count, 0);
        assert_eq!(dev_a.metrics.disconnected_ms, 0);

        let dev_b = &result.device_aggregations[1];
        assert_eq!(dev_b.nodename, "B");
        assert_eq!(dev_b.metrics.active_ms, 1000);
        assert_eq!(dev_b.metrics.inactive_ms, 1000);
        assert_eq!(dev_b.metrics.rcs_dropouts, 1);
        assert_eq!(dev_b.metrics.target_vanished_count, 1);
        assert_eq!(dev_b.metrics.disconnected_ms, 1000);
        assert_eq!(
            dev_b.metrics.disconnected_ms,
            dev_b.metrics.unexpected_disconnected_ms + dev_b.metrics.intentional_disconnected_ms
        );

        let dev_c = &result.device_aggregations[2];
        assert_eq!(dev_c.nodename, "C");
        assert_eq!(dev_c.metrics.active_ms, 0);
        assert_eq!(dev_c.metrics.rcs_dropouts, 0);
        assert_eq!(
            dev_c.metrics.disconnected_ms,
            dev_c.metrics.unexpected_disconnected_ms + dev_c.metrics.intentional_disconnected_ms
        );
    }

    #[test]
    fn test_compute_aggregations_empty() {
        let state = AggregationsState::default();
        let result = state.finalize();
        assert_eq!(result.version, AGGREGATIONS_VERSION);
        assert_eq!(result.summary.total_devices, 0);
        assert_eq!(result.summary.duration_ms, 0);
        assert_eq!(result.device_aggregations.len(), 0);
    }

    #[test]
    fn test_compute_aggregations_p90_p95() {
        let mut state = AggregationsState::default();
        for i in 1..=100 {
            state.update(&make_entry(i * 1000, i, vec![]));
        }

        let result = state.finalize();
        let latency = &result.summary.latency;
        assert_eq!(latency.count, 100);
        assert_eq!(latency.min, 1);
        assert_eq!(latency.max, 100);
        assert_eq!(latency.p90, 91);
        assert_eq!(latency.p95, 96);
    }
}
