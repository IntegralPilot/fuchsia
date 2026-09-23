// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
mod args;

use anyhow::{Context, Result};
use args::{ProfilerCommand, ProfilerSubCommand};
use async_fs::File;
use core::fmt;
use errors::{ffx_bail, ffx_error};
use fdomain_client::fidl::Proxy;
use fdomain_fuchsia_cpu_profiler as profiler;
use fdomain_fuchsia_cpu_profiler::SessionResult;
use fdomain_fuchsia_test_manager as test_manager;
use ffx_config::EnvironmentContext;
use ffx_writer::{MachineWriter, ToolIO as _};
use fho::{FfxMain, FfxTool, bug, deferred, return_user_error, user_error};
use log::info;
use schemars::JsonSchema;
use serde::Serialize;
use std::io::{BufRead, IsTerminal, stdin};
use std::time::Duration;
use target_holders::moniker;
use tempfile::Builder;
use termion::{color, style};

#[derive(Serialize, JsonSchema)]
pub struct ShowCpuProfilerCmd {
    pub samples_collected: Option<u64>,
    pub median_sample_time: Option<u64>,
    pub mean_sample_time: Option<u64>,
    pub max_sample_time: Option<u64>,
    pub min_sample_time: Option<u64>,
    pub missing_process_mappings: Option<Vec<u64>>,
}

impl fmt::Display for ShowCpuProfilerCmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Session Stats: \n")?;
        if let Some(num_samples) = self.samples_collected {
            write!(f, "    Number of samples collected: {}\n", num_samples)?;
        }
        if let Some(median_sample_time) = self.median_sample_time {
            write!(f, "    Median sample time: {}us\n", median_sample_time)?;
        }
        if let Some(mean_sample_time) = self.mean_sample_time {
            write!(f, "    Mean sample time: {}us\n", mean_sample_time)?;
        }
        if let Some(max_sample_time) = self.max_sample_time {
            write!(f, "    Max sample time: {}us\n", max_sample_time)?;
        }
        if let Some(min_sample_time) = self.min_sample_time {
            write!(f, "    Min sample time: {}us\n", min_sample_time)?;
        }
        if let Some(ref pids) = self.missing_process_mappings {
            write!(f, "    Processes missing mappings: {:?}\n", pids)?;
        }
        Ok(())
    }
}

type Writer = MachineWriter<ShowCpuProfilerCmd>;
#[derive(FfxTool)]
pub struct ProfilerTool {
    #[with(deferred(moniker("/core/cpu_profiler")))]
    controller: fho::Deferred<profiler::SessionProxy>,
    #[with(deferred(moniker("/core/cpu_profiler/profiler_session_manager")))]
    session_manager: fho::Deferred<profiler::SessionManagerProxy>,
    #[command]
    cmd: ProfilerCommand,
    context: EnvironmentContext,
}

#[async_trait::async_trait(?Send)]
impl FfxMain for ProfilerTool {
    type Writer = Writer;

    type Error = ::fho::Error;

    async fn main(self, writer: Self::Writer) -> fho::Result<()> {
        info!(cmd:? = self.cmd; "Running profiler... ");
        self.profiler(writer).await
    }
}

fn gather_targets(opts: &args::Attach) -> Result<profiler::TargetConfig> {
    if let Some(moniker) = &opts.moniker {
        if !opts.pids.is_empty()
            || !opts.tids.is_empty()
            || !opts.job_ids.is_empty()
            || opts.system_wide
        {
            ffx_bail!(
                "Targeting both a component and specific jobs/processes/threads is not supported"
            )
        }
        let component_config = profiler::AttachConfig::AttachToComponentMoniker(moniker.clone());
        Ok(profiler::TargetConfig::Component(component_config))
    } else if let Some(url) = &opts.url {
        if !opts.pids.is_empty()
            || !opts.tids.is_empty()
            || !opts.job_ids.is_empty()
            || opts.system_wide
        {
            ffx_bail!(
                "Targeting both a component and specific jobs/processes/threads is not supported"
            )
        }
        let component_config = profiler::AttachConfig::AttachToComponentUrl(url.clone());
        Ok(profiler::TargetConfig::Component(component_config))
    } else {
        let mut tasks: Vec<_> = opts
            .job_ids
            .iter()
            .map(|&id| profiler::Task::Job(id))
            .chain(opts.pids.iter().map(|&id| profiler::Task::Process(id)))
            .chain(opts.tids.iter().map(|&id| profiler::Task::Thread(id)))
            .collect();
        if opts.system_wide {
            tasks.push(profiler::Task::SystemWide(profiler::SystemWide {}));
        }
        if tasks.is_empty() {
            ffx_bail!("No targets were specified")
        }
        Ok(profiler::TargetConfig::Tasks(tasks))
    }
}

fn make_boot_target_config(
    target_config: &profiler::TargetConfig,
) -> Result<profiler::OnBootTargetConfig> {
    match target_config {
        profiler::TargetConfig::Component(profiler::AttachConfig::AttachToComponentMoniker(
            moniker,
        )) => Ok(profiler::OnBootTargetConfig::Component(
            profiler::OnBootAttachConfig::AttachToComponentMoniker(moniker.clone()),
        )),
        profiler::TargetConfig::Component(profiler::AttachConfig::AttachToComponentUrl(url)) => {
            Ok(profiler::OnBootTargetConfig::Component(
                profiler::OnBootAttachConfig::AttachToComponentUrl(url.clone()),
            ))
        }
        profiler::TargetConfig::Tasks(tasks) => {
            Ok(profiler::OnBootTargetConfig::Tasks(tasks.clone()))
        }
        _ => ffx_bail!("On-boot profiling requires a component URL or moniker, or tasks"),
    }
}

#[derive(Debug, PartialEq)]
struct SessionOpts {
    symbolize: bool,
    buffer_size_mb: Option<u64>,
    print_stats: bool,
    pprof_conversion: bool,
    output: String,
    duration: Option<u64>,
    color_output: bool,
}

fn check_background_args(
    duration: Option<u64>,
    output: Option<&str>,
    print_stats: bool,
    symbolize: bool,
    pprof_conversion: bool,
    color_output: bool,
) -> fho::Result<()> {
    if duration.is_some() {
        ffx_bail!("Cannot specify a duration when starting a background profiling session.");
    }

    // Check for non-default values for the other arguments.
    if output.is_some()
        || print_stats
        || !symbolize
        || !pprof_conversion
        || color_output != std::io::stdout().is_terminal()
    {
        let message: &str = "The options --output, --print-stats, --symbolize, --pprof-conversion, \
            and --color-output should be specified when calling `ffx profiler stop` when using \
            a background profiling session.";
        ffx_bail!("{message}");
    }
    Ok(())
}

fn default_sampling_config(
    sample_period_us: u64,
    unwind_strategy: args::UnwindStrategy,
) -> profiler::SamplingConfig {
    let strategy = match unwind_strategy {
        args::UnwindStrategy::Dwarf => profiler::CallgraphStrategy::Dwarf,
        args::UnwindStrategy::FramePointer => profiler::CallgraphStrategy::FramePointer,
    };

    profiler::SamplingConfig {
        period: Some(sample_period_us * 1000),
        timebase: Some(profiler::Counter::PlatformIndependent(profiler::CounterId::Nanoseconds)),
        sample: Some(profiler::Sample {
            callgraph: Some(profiler::CallgraphConfig {
                strategy: Some(strategy),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn create_session_opts(
    symbolize: bool,
    buffer_size_mb: Option<u64>,
    print_stats: bool,
    output: Option<String>,
    duration: Option<u64>,
    pprof_conversion: bool,
    color_output: bool,
) -> SessionOpts {
    let extension = if symbolize { "pb" } else { "fxt" };
    let final_output = match output {
        Some(o) => o,
        None => format!("profile.{}", extension),
    };
    SessionOpts {
        symbolize,
        buffer_size_mb,
        print_stats,
        output: final_output,
        duration,
        pprof_conversion,
        color_output,
    }
}

fn print_stop_stats(
    writer: &mut Writer,
    stats: &ShowCpuProfilerCmd,
    color_output: bool,
    print_stats: bool,
) -> fho::Result<()> {
    if let Some(pids) = &stats.missing_process_mappings {
        if !pids.is_empty() {
            writeln!(
                writer.stderr(),
                "{}[WARNING] Failed to get symbols for some processes: {:?}\n\
                This can occur when processes exit before the profiler is able to read their modules.{}",
                if color_output {
                    format!("{}", color::Fg(color::Red))
                } else {
                    String::from("")
                },
                pids,
                if color_output { format!("{}", style::Reset) } else { String::from("") },
            ).map_err(|e| anyhow::anyhow!(e))?;
        }
    }
    if print_stats {
        writer.machine(stats)?;
        writer.line(format!("\n{stats}"))?;
    }
    Ok(())
}

pub enum ProfilerSessionManagerProxy {
    Controller(profiler::SessionProxy),
    SessionManager(profiler::SessionManagerProxy),
}

impl From<profiler::SessionProxy> for ProfilerSessionManagerProxy {
    fn from(proxy: profiler::SessionProxy) -> Self {
        Self::Controller(proxy)
    }
}

impl From<profiler::SessionManagerProxy> for ProfilerSessionManagerProxy {
    fn from(proxy: profiler::SessionManagerProxy) -> Self {
        Self::SessionManager(proxy)
    }
}

impl ProfilerSessionManagerProxy {
    pub fn domain(&self) -> std::sync::Arc<fdomain_client::Client> {
        match self {
            Self::Controller(c) => c.domain(),
            Self::SessionManager(mgr) => mgr.domain(),
        }
    }

    pub async fn configure(
        &self,
        req: profiler::SessionConfigureRequest,
    ) -> Result<Result<(), profiler::SessionConfigureError>, fidl::Error> {
        match self {
            Self::Controller(c) => c.configure(req).await,
            Self::SessionManager(mgr) => mgr.configure(req).await,
        }
    }

    pub async fn start(
        &self,
        req: &profiler::SessionStartRequest,
    ) -> Result<Result<(), profiler::SessionStartError>, fidl::Error> {
        match self {
            Self::Controller(c) => c.start(req).await,
            Self::SessionManager(mgr) => mgr.start(req).await,
        }
    }

    pub async fn stop(&self) -> Result<profiler::SessionResult, fidl::Error> {
        match self {
            Self::Controller(c) => c.stop().await,
            Self::SessionManager(mgr) => mgr.stop().await,
        }
    }

    pub async fn reset(&self) -> Result<(), fidl::Error> {
        match self {
            Self::Controller(c) => c.reset().await,
            Self::SessionManager(mgr) => mgr.reset().await,
        }
    }

    pub async fn start_session(
        &self,
        req: profiler::SessionManagerStartSessionRequest,
    ) -> Result<
        Result<profiler::SessionManagerStartSessionResponse, profiler::ManagerError>,
        fidl::Error,
    > {
        match self {
            Self::Controller(_) => Ok(Err(profiler::ManagerError::Start)),
            Self::SessionManager(mgr) => mgr.start_session(req).await,
        }
    }

    pub async fn stop_session(
        &self,
        req: profiler::SessionManagerStopSessionRequest,
    ) -> Result<Result<profiler::SessionResult, profiler::ManagerError>, fidl::Error> {
        match self {
            Self::Controller(_) => Ok(Err(profiler::ManagerError::Stop)),
            Self::SessionManager(mgr) => mgr.stop_session(req).await,
        }
    }

    pub async fn status(
        &self,
    ) -> Result<Result<profiler::SessionManagerStatusResponse, profiler::ManagerError>, fidl::Error>
    {
        match self {
            Self::Controller(_) => Ok(Err(profiler::ManagerError::NoSuchTask)),
            Self::SessionManager(mgr) => mgr.status().await,
        }
    }

    pub async fn abort_session(
        &self,
        req: &profiler::SessionManagerAbortSessionRequest,
    ) -> Result<Result<(), profiler::ManagerError>, fidl::Error> {
        match self {
            Self::Controller(_) => Ok(Err(profiler::ManagerError::Stop)),
            Self::SessionManager(mgr) => mgr.abort_session(req).await,
        }
    }

    pub async fn start_session_on_boot(
        &self,
        req: &profiler::SessionManagerStartSessionOnBootRequest,
    ) -> Result<Result<(), profiler::ManagerError>, fidl::Error> {
        match self {
            Self::Controller(_) => Ok(Err(profiler::ManagerError::Start)),
            Self::SessionManager(mgr) => mgr.start_session_on_boot(req).await,
        }
    }
}

async fn finalize_profile_session(
    proxy: &ProfilerSessionManagerProxy,
    writer: &mut Writer,
    options: &SessionOpts,
    copy_task: Option<fuchsia_async::Task<std::io::Result<u64>>>,
    unsymbolized_path: &std::path::PathBuf,
    context: &EnvironmentContext,
    is_background: bool,
) -> Result<()> {
    info!("Stopping profiler...");
    let mut copy_task = copy_task;

    let stats = if is_background {
        let (client, server) = proxy.domain().create_stream_socket();

        let req = profiler::SessionManagerStopSessionRequest {
            output: Some(server),
            ..Default::default()
        };

        let mut output = async_fs::File::create(&unsymbolized_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create output file: {:?}", e))?;

        copy_task = Some(fuchsia_async::Task::local(async move {
            futures::io::copy(client, &mut output).await
        }));

        match proxy.stop_session(req).await? {
            Ok(stats) => SessionResult {
                samples_collected: stats.samples_collected,
                mean_sample_time: stats.mean_sample_time,
                median_sample_time: stats.median_sample_time,
                min_sample_time: stats.min_sample_time,
                max_sample_time: stats.max_sample_time,
                missing_process_mappings: stats.missing_process_mappings,
                ..Default::default()
            },
            Err(e) => anyhow::bail!("Failed to stop session: {:?}", e),
        }
    } else {
        proxy.stop().await?
    };

    let out_stats = ShowCpuProfilerCmd {
        samples_collected: stats.samples_collected,
        median_sample_time: stats.median_sample_time,
        mean_sample_time: stats.mean_sample_time,
        max_sample_time: stats.max_sample_time,
        min_sample_time: stats.min_sample_time,
        missing_process_mappings: stats.missing_process_mappings,
    };

    print_stop_stats(writer, &out_stats, options.color_output, options.print_stats)?;

    info!("Profiler stopped, waiting for copy to complete...");
    if let Some(task) = copy_task {
        task.await.map_err(|e| anyhow::anyhow!(e))?;
    }

    info!("Copy from profiler completed, resetting profiler...");
    proxy.reset().await?;

    let unsymbolized_samples =
        ffx_profiler::symbolize::create_unsymbolized_samples(unsymbolized_path)?;

    if !options.symbolize {
        std::fs::write(unsymbolized_path, format!("{unsymbolized_samples:#?}\n"))?;
        return Ok(());
    }

    if let Ok(symbolized_record) = unsymbolized_samples.process_unsymbolized_samples(
        &options.output,
        options.pprof_conversion,
        context,
    ) {
        ffx_profiler::pprof::samples_to_pprof(symbolized_record, options.output.as_str().into())?;
        return Ok(());
    } else {
        anyhow::bail!("Failed to symbolize profile");
    }
}

async fn run_session(
    context: &EnvironmentContext,
    session_proxy: &ProfilerSessionManagerProxy,
    mut writer: Writer,
    config: profiler::Config,
    opts: SessionOpts,
) -> fho::Result<()> {
    info!(config:? = config, opts:? = opts; "Running profiler session...");
    let (client, server) = session_proxy.domain().create_stream_socket();

    session_proxy
        .configure(profiler::SessionConfigureRequest {
            output: Some(server),
            config: Some(config),
            ..Default::default()
        })
        .await
        .map_err(|e| bug!("{e}"))?
        .map_err(|e| user_error!("Failed to start: {:?}", e))?;
    info!("Profiler session is configured.");

    let tmp_dir = Builder::new()
        .prefix("fuchsia_cpu_profiler_")
        .tempdir()
        .map_err(|e| user_error!("Failed to create temporary directory: {e}"))?;
    let unsymbolized_path = if opts.symbolize {
        tmp_dir.path().join("unsymbolized.txt")
    } else {
        std::path::PathBuf::from(&opts.output)
    };

    let mut output = File::create(&unsymbolized_path)
        .await
        .map_err(|e| user_error!("Failed to create output file: {e}"))?;
    let copy_task =
        fuchsia_async::Task::local(async move { futures::io::copy(client, &mut output).await });

    info!("Starting profiler...");
    session_proxy
        .start(&profiler::SessionStartRequest {
            buffer_results: Some(true),
            buffer_size_mb: opts.buffer_size_mb,
            ..Default::default()
        })
        .await
        .map_err(|e| bug!(e))?
        .map_err(|e| ffx_error!("Failed to start: {:?}", e))?;
    info!("Profiler started.");

    if let &Some(duration) = &opts.duration {
        writer.line(format!("Waiting for {} seconds...", duration))?;
        fuchsia_async::Timer::new(Duration::from_secs(duration)).await;
    } else {
        writer.line("Press <enter> to stop profiling...")?;
        blocking::unblock(|| {
            let _ = stdin().lock().read_line(&mut String::new());
        })
        .await;
    }

    finalize_profile_session(
        session_proxy,
        &mut writer,
        &opts,
        Some(copy_task),
        &unsymbolized_path,
        context,
        false,
    )
    .await?;

    Ok(())
}

async fn download_android_symbols(opts: args::DownloadAndroidSymbols) -> Result<()> {
    info!("Fetching android symbols for build {} target {}", opts.bid, opts.target);

    let target_prefix = opts.target.split('-').next().unwrap_or(&opts.target);
    let glob = format!("{}-symbols-*.zip", target_prefix);

    // Create a temporary directory to download the zip into
    let tmp_dir = Builder::new().prefix("android_symbols_").tempdir()?;
    let out_dir = tmp_dir.path().join("symbols");
    std::fs::create_dir_all(&out_dir)?;

    // Invoke fetch_artifact to get the symbol archive
    let mut command = std::process::Command::new("/google/data/ro/projects/android/fetch_artifact");
    command
        .arg("--bid")
        .arg(&opts.bid)
        .arg("--target")
        .arg(&opts.target)
        .arg("--use_shared_quota")
        .arg(&glob)
        .arg(&out_dir);

    println!("Running: {:?}", command);
    let status = command.status()?;

    if !status.success() {
        anyhow::bail!("fetch_artifact failed: {}", status);
    }

    // Now extract the zip(s) into a sub-directory
    let extract_dir = tmp_dir.path().join("extracted");
    std::fs::create_dir_all(&extract_dir)?;

    println!("Extracting symbols...");
    for entry in std::fs::read_dir(&out_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("zip") {
            println!("Unzipping {}", path.display());
            let extract_status = std::process::Command::new("unzip")
                .arg("-q")
                .arg(&path)
                .arg("-d")
                .arg(&extract_dir)
                .status()?;

            if !extract_status.success() {
                anyhow::bail!("unzip failed: {}", extract_status);
            }
        }
    }

    // Determine the user's .build-id directory
    let home = std::env::var("HOME").context("Failed to get HOME env var")?;
    let build_id_dir = std::path::PathBuf::from(home).join(".fuchsia/debug/build-id");
    std::fs::create_dir_all(&build_id_dir)?;

    println!("Populating .build-id cache...");

    // Traverse extracted directory to find all .so files
    let mut dirs_to_visit = vec![extract_dir];
    while let Some(dir) = dirs_to_visit.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs_to_visit.push(path);
                continue;
            }
            if !path.is_file() || path.extension().and_then(|s| s.to_str()) != Some("so") {
                continue;
            }

            // Get Build ID using readelf
            let Ok(output) = std::process::Command::new("readelf").arg("-n").arg(&path).output()
            else {
                continue;
            };

            if !output.status.success() {
                continue;
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let Some(line) = stdout.lines().find(|l| l.contains("Build ID:")) else {
                continue;
            };

            let Some(build_id) = line.split_whitespace().last() else {
                continue;
            };

            if build_id.len() <= 2 {
                continue;
            }

            let (xx, yy) = build_id.split_at(2);
            let target_dir = build_id_dir.join(xx);
            if std::fs::create_dir_all(&target_dir).is_ok() {
                let target_file = target_dir.join(format!("{}.debug", yy));
                let _ = std::fs::copy(&path, &target_file);
            }
        }
    }

    println!("Successfully downloaded and cached Android symbols for bid {}!", opts.bid);

    Ok(())
}

impl ProfilerTool {
    async fn get_session_proxy(self) -> Result<ProfilerSessionManagerProxy> {
        match self.session_manager.await {
            Ok(p) => Ok(p.into()),
            Err(e) => {
                log::warn!(
                    "Cannot connect to SessionManagerProxy, falling back to basic profiler controller: {e:?}."
                );
                Ok(self.controller.await?.into())
            }
        }
    }

    pub async fn profiler(self, mut writer: Writer) -> fho::Result<()> {
        let context = self.context.clone();
        let sub_command = self.cmd.sub_cmd.clone();
        let session_proxy = self.get_session_proxy().await?;

        let (targets, config, session_opts, background) = match sub_command {
            ProfilerSubCommand::Attach(opts) => {
                let target = gather_targets(&opts)?;
                if opts.on_boot {
                    let boot_target = make_boot_target_config(&target)?;
                    let config = profiler::OnBootConfig {
                        configs: Some(vec![default_sampling_config(
                            opts.sample_period_us,
                            opts.unwind_strategy.clone(),
                        )]),
                        target: Some(boot_target),
                        ..Default::default()
                    };

                    if let ProfilerSessionManagerProxy::SessionManager(mgr) = &session_proxy {
                        let req = profiler::SessionManagerStartSessionOnBootRequest {
                            config: Some(config),
                            ..Default::default()
                        };
                        match mgr.start_session_on_boot(&req).await.map_err(|e| bug!(e))? {
                            Ok(_) => {
                                writer.line("On-boot profiling session configured. \
                                             Profiles will be collected when the target component starts.")?;
                            }
                            Err(e) => {
                                ffx_bail!("Failed to start on-boot session: {:?}", e);
                            }
                        }
                        return Ok(());
                    } else {
                        ffx_bail!(
                            "On-boot profiling requires a SessionManager connection, but none could be established."
                        );
                    }
                }

                if opts.background {
                    check_background_args(
                        opts.duration,
                        opts.output.as_deref(),
                        opts.print_stats,
                        opts.symbolize,
                        opts.pprof_conversion,
                        opts.color_output,
                    )?;
                }
                let config = default_sampling_config(opts.sample_period_us, opts.unwind_strategy);
                let session_opts = create_session_opts(
                    opts.symbolize,
                    opts.buffer_size_mb,
                    opts.print_stats,
                    opts.output,
                    opts.duration,
                    opts.pprof_conversion,
                    opts.color_output,
                );
                (target, config, session_opts, opts.background)
            }
            ProfilerSubCommand::Launch(opts) => {
                if opts.background {
                    check_background_args(
                        opts.duration,
                        opts.output.as_deref(),
                        opts.print_stats,
                        opts.symbolize,
                        opts.pprof_conversion,
                        opts.color_output,
                    )?;
                }
                let component_config = if opts.test {
                    profiler::AttachConfig::LaunchTest(profiler::LaunchTest {
                        url: Some(opts.url.clone()),
                        options: Some(test_manager::RunSuiteOptions {
                            test_case_filters: Some(opts.test_filters),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })
                } else {
                    profiler::AttachConfig::LaunchComponent(profiler::LaunchComponent {
                        url: Some(opts.url.clone()),
                        moniker: opts.moniker.clone(),
                        ..Default::default()
                    })
                };
                let target = profiler::TargetConfig::Component(component_config);
                let config = default_sampling_config(opts.sample_period_us, opts.unwind_strategy);
                let session_opts = create_session_opts(
                    opts.symbolize,
                    opts.buffer_size_mb,
                    opts.print_stats,
                    opts.output,
                    opts.duration,
                    opts.pprof_conversion,
                    opts.color_output,
                );
                (target, config, session_opts, opts.background)
            }
            ProfilerSubCommand::Symbolize(opts) => {
                let unsymbolized_samples =
                    ffx_profiler::symbolize::create_unsymbolized_samples(&opts.input)
                        .map_err(|e| bug!("Failed to create unsymbolized samples: {:?}", e))?;
                match unsymbolized_samples.process_unsymbolized_samples(
                    &opts.output,
                    opts.pprof_conversion,
                    &context,
                ) {
                    Ok(symbolized_record) => {
                        return ffx_profiler::pprof::samples_to_pprof(
                            symbolized_record,
                            opts.output.into(),
                        )
                        .map_err(|e| user_error!("Failed to convert to pprof: {:?}", e));
                    }
                    Err(e) => return_user_error!("Failed to symbolize profile: {:?}", e),
                }
            }
            ProfilerSubCommand::DownloadAndroidSymbols(opts) => {
                return download_android_symbols(opts)
                    .await
                    .map_err(|e| user_error!("Failed to download Android symbols: {e:?}"));
            }
            ProfilerSubCommand::Stop(stop_options) => {
                if let ProfilerSessionManagerProxy::SessionManager(mgr) = &session_proxy {
                    if stop_options.abort {
                        let req = profiler::SessionManagerAbortSessionRequest::default();
                        info!("Aborting background session");

                        match mgr
                            .abort_session(&req)
                            .await
                            .map_err(|e| bug!("Failed to abort session: {:?}", e))?
                        {
                            Ok(_) => writer.line("Session aborted successfully.")?,
                            Err(e) => ffx_bail!("Failed to abort background session: {:?}", e),
                        }
                        return Ok(());
                    }

                    let extension = if stop_options.symbolize { "pb" } else { "fxt" };
                    let final_output = match stop_options.output {
                        Some(o) => o,
                        None => format!("profile.{}", extension),
                    };

                    let session_opts = SessionOpts {
                        symbolize: stop_options.symbolize,
                        buffer_size_mb: None,
                        print_stats: stop_options.print_stats,
                        pprof_conversion: stop_options.pprof_conversion,
                        output: final_output,
                        duration: None,
                        color_output: stop_options.color_output,
                    };

                    info!("Stopping background session");

                    // The temp dir needs to stay in scope until symbolization is completed.
                    let tmp_dir = Builder::new()
                        .prefix("fuchsia_cpu_profiler_")
                        .tempdir()
                        .map_err(|e| bug!("Failed to create temp dir: {:?}", e))?;

                    let unsymbolized_path = if stop_options.symbolize {
                        tmp_dir.path().join("unsymbolized.txt")
                    } else {
                        std::path::PathBuf::from(&session_opts.output)
                    };

                    finalize_profile_session(
                        &session_proxy,
                        &mut writer,
                        &session_opts,
                        None,
                        &unsymbolized_path,
                        &context,
                        true,
                    )
                    .await
                    .map_err(|e| user_error!("Failed to finalize session: {:?}", e))?;

                    writer.line(format!("Wrote profile to {}", session_opts.output))?;
                } else {
                    ffx_bail!("Connecting to SessionManager is required for background profiling.");
                }
                return Ok(());
            }
            ProfilerSubCommand::Status(_) => {
                if let ProfilerSessionManagerProxy::SessionManager(mgr) = session_proxy {
                    let response = mgr
                        .status()
                        .await
                        .map_err(|e| bug!("Failed to get background session status: {:?}", e))?;
                    match response {
                        Ok(resp) => {
                            if let Some(sessions) = resp.sessions {
                                if sessions.is_empty() {
                                    writer.line("No background profiling sessions active.")?;
                                } else {
                                    writer.line("Active background sessions:")?;
                                    for session in sessions {
                                        writer.line(format!(
                                            "  - task_id: {}",
                                            session.task_id.unwrap_or(0)
                                        ))?;
                                    }
                                }
                            } else {
                                writer.line("No background profiling sessions active.")?;
                            }
                        }
                        Err(e) => {
                            ffx_bail!("Failed to get background session status: {:?}", e);
                        }
                    }
                } else {
                    ffx_bail!("Connecting to SessionManager is required for background profiling.");
                }
                return Ok(());
            }
        };
        let config = profiler::Config {
            configs: Some(vec![config]),
            target: Some(targets),
            ..Default::default()
        };

        if background {
            if let ProfilerSessionManagerProxy::SessionManager(mgr) = &session_proxy {
                info!(config:? = config; "Starting background profiler session...");

                let req = profiler::SessionManagerStartSessionRequest {
                    config: Some(config),
                    ..Default::default()
                };
                let response = mgr.start_session(req).await.map_err(|e| bug!(e))?;
                match response {
                    Ok(resp) => {
                        writer.line(format!(
                            "Background session started. task_id: {}",
                            resp.task_id.unwrap_or(0)
                        ))?;
                    }
                    Err(e) => {
                        ffx_bail!("Failed to start background session: {:?}", e);
                    }
                }
                Ok(())
            } else {
                ffx_bail!(
                    "Background profiling requires a SessionManager connection, but none could be established."
                );
            }
        } else {
            run_session(&context, &session_proxy, writer, config, session_opts).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_gather_targets() {
        let args = args::Attach {
            pids: vec![1, 2, 3],
            tids: vec![4, 5, 6],
            job_ids: vec![7, 8, 9],
            url: None,
            buffer_size_mb: Some(8 as u64),
            moniker: None,
            duration: None,
            output: Some(String::from("output_file.pb")),
            ..Default::default()
        };
        let target = gather_targets(&args);
        match target {
            Ok(profiler::TargetConfig::Tasks(vec)) => assert!(vec.len() == 9),
            _ => assert!(false),
        }

        let empty_args = args::Attach {
            pids: vec![],
            tids: vec![],
            job_ids: vec![],
            moniker: None,
            url: None,
            buffer_size_mb: None,
            duration: None,
            output: Some(String::from("output_file.pb")),
            ..Default::default()
        };

        let empty_targets = gather_targets(&empty_args);
        assert!(empty_targets.is_err());

        let invalid_args1 = args::Attach {
            pids: vec![1],
            tids: vec![],
            job_ids: vec![],
            moniker: Some(String::from("core/test")),
            buffer_size_mb: Some(8 as u64),
            url: None,
            duration: None,
            output: Some(String::from("output_file.pb")),
            ..Default::default()
        };
        let invalid_args2 = args::Attach {
            pids: vec![],
            tids: vec![1],
            job_ids: vec![],
            moniker: Some(String::from("core/test")),
            url: None,
            buffer_size_mb: Some(8 as u64),
            duration: None,
            output: Some(String::from("output_file.pb")),
            ..Default::default()
        };
        let invalid_args3 = args::Attach {
            pids: vec![],
            tids: vec![],
            job_ids: vec![1],
            moniker: Some(String::from("core/test")),
            buffer_size_mb: Some(8 as u64),
            url: None,
            duration: None,
            output: Some(String::from("output_file.pb")),
            ..Default::default()
        };
        let invalid_targets1 = gather_targets(&invalid_args1);
        assert!(invalid_targets1.is_err());
        let invalid_targets2 = gather_targets(&invalid_args2);
        assert!(invalid_targets2.is_err());
        let invalid_targets3 = gather_targets(&invalid_args3);
        assert!(invalid_targets3.is_err());
    }

    #[test]
    fn test_make_boot_target_config() {
        let target_config_moniker = profiler::TargetConfig::Component(
            profiler::AttachConfig::AttachToComponentMoniker(String::from("core/test")),
        );
        let target_moniker = make_boot_target_config(&target_config_moniker);
        assert!(target_moniker.is_ok());
        if let Ok(profiler::OnBootTargetConfig::Component(
            profiler::OnBootAttachConfig::AttachToComponentMoniker(m),
        )) = target_moniker
        {
            assert_eq!(m, "core/test");
        } else {
            assert!(false, "Expected ComponentMoniker");
        }

        let target_config_url =
            profiler::TargetConfig::Component(profiler::AttachConfig::AttachToComponentUrl(
                String::from("fuchsia-pkg://fuchsia.com/my-pkg#meta/my-component.cm"),
            ));
        let target_url = make_boot_target_config(&target_config_url);
        assert!(target_url.is_ok());
        if let Ok(profiler::OnBootTargetConfig::Component(
            profiler::OnBootAttachConfig::AttachToComponentUrl(u),
        )) = target_url
        {
            assert_eq!(u, "fuchsia-pkg://fuchsia.com/my-pkg#meta/my-component.cm");
        } else {
            assert!(false, "Expected ComponentUrl");
        }

        let target_config_tasks = profiler::TargetConfig::Tasks(vec![profiler::Task::Process(123)]);
        let empty_targets = make_boot_target_config(&target_config_tasks);
        assert!(empty_targets.is_ok());
        if let Ok(profiler::OnBootTargetConfig::Tasks(t)) = empty_targets {
            assert_eq!(t.len(), 1);
            if let profiler::Task::Process(pid) = t[0] {
                assert_eq!(pid, 123);
            } else {
                assert!(false, "Expected Task::Process(123)");
            }
        } else {
            assert!(false, "Expected Tasks");
        }
    }

    #[test]
    fn test_check_background_args_valid() {
        let result =
            check_background_args(None, None, false, true, true, std::io::stdout().is_terminal());
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_background_args_with_duration() {
        let result = check_background_args(
            Some(10),
            None,
            false,
            true,
            true,
            std::io::stdout().is_terminal(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_check_background_args_invalid_output() {
        let result = check_background_args(
            None,
            Some("custom_profile"),
            false,
            true,
            true,
            std::io::stdout().is_terminal(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_check_background_args_print_stats() {
        let result =
            check_background_args(None, None, true, true, true, std::io::stdout().is_terminal());
        assert!(result.is_err());
    }

    #[test]
    fn test_check_background_args_no_symbolize() {
        let result =
            check_background_args(None, None, false, false, true, std::io::stdout().is_terminal());
        assert!(result.is_err());
    }

    #[test]
    fn test_check_background_args_no_pprof() {
        let result =
            check_background_args(None, None, false, true, false, std::io::stdout().is_terminal());
        assert!(result.is_err());
    }

    #[test]
    fn test_check_background_args_invalid_color() {
        let result =
            check_background_args(None, None, false, true, true, !std::io::stdout().is_terminal());
        assert!(result.is_err());
    }

    #[test]
    fn test_create_session_opts_extension() {
        // Test that default is used if output is None
        let opts = create_session_opts(true, None, false, None, None, true, true);
        assert_eq!(opts.output, "profile.pb");

        let opts = create_session_opts(false, None, false, None, None, true, true);
        assert_eq!(opts.output, "profile.fxt");

        // Test that output is literal if Some is provided
        let opts =
            create_session_opts(true, None, false, Some("profile".to_string()), None, true, true);
        assert_eq!(opts.output, "profile");

        let opts = create_session_opts(
            true,
            None,
            false,
            Some("profile.pb".to_string()),
            None,
            true,
            true,
        );
        assert_eq!(opts.output, "profile.pb");
    }

    #[fuchsia::test]
    async fn test_stop_no_active_session() {
        use fdomain_fuchsia_cpu_profiler::{ManagerError, SessionManagerRequest};
        use ffx_writer::TestBuffers;
        use target_holders::fake_proxy;
        let client = fdomain_local::local_client_empty();

        let session_manager = fake_proxy(std::sync::Arc::clone(&client), move |req| match req {
            SessionManagerRequest::StopSession { payload: _, responder } => {
                responder.send(Err(ManagerError::NoSuchTask)).unwrap();
            }
            SessionManagerRequest::AbortSession { payload: _, responder } => {
                responder.send(Err(ManagerError::NoSuchTask)).unwrap();
            }
            _ => panic!("Unexpected request: {:?}", req),
        });

        let tool = ProfilerTool {
            controller: fho::Deferred::from_output(Err(fho::Error::Unexpected(anyhow::anyhow!(
                "controller should not be accessed"
            )))),
            session_manager: fho::Deferred::from_output(Ok(session_manager)),
            cmd: ProfilerCommand {
                sub_cmd: ProfilerSubCommand::Stop(args::Stop {
                    output: None,
                    abort: false,
                    symbolize: false,
                    pprof_conversion: false,
                    color_output: false,
                    print_stats: false,
                }),
            },
            context: ffx_config::EnvironmentContext::default(),
        };

        let buffers = TestBuffers::default();
        let writer = <ProfilerTool as FfxMain>::Writer::new_test(None, &buffers);

        let result = tool.profiler(writer).await;

        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(err_str.contains("Failed to finalize session"));
        assert!(err_str.contains("NoSuchTask"));
    }
}
