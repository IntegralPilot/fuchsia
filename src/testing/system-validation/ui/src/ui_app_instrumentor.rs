// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use argh::FromArgs;
use fuchsia_component::client::connect_to_protocol;
use log::info;
use system_validation_lib::app_monitor::AppMonitor;
use system_validation_lib::screencapture::take_screenshot;
use system_validation_lib::single_session_trace::SingleSessionTrace;
use {
    fidl_fuchsia_session_scene as scene, fidl_fuchsia_ui_app as ui_app, fuchsia_async as fasync,
    fuchsia_scenic as scenic,
};

/// Args to control how to run system validation test.
/// Specify them through *_system_validation.cml
#[derive(FromArgs, PartialEq, Debug)]
struct RunTestCmd {
    /// trace configurations, input is a comma delimited string
    /// example: --trace-config "system_metrics,input,gfx"
    #[argh(option, default = "vec![]", from_str_fn(to_vector))]
    trace_config: Vec<String>,

    /// number of seconds to run example app, default is 5 seconds
    #[argh(
        option,
        default = "fasync::MonotonicDuration::from_seconds(5)",
        from_str_fn(to_duration)
    )]
    run_duration_sec: fasync::MonotonicDuration,
}

// Note: Maybe we want to make this an argument.
const SAMPLE_APP_MONIKER: &str = "./sample-app";

#[fuchsia::main]
async fn main() {
    let args: RunTestCmd = argh::from_env();
    // Hook up to scene_manager
    let scene_manager = connect_to_protocol::<scene::ManagerMarker>()
        .expect("failed to connect to fuchsia.scene.Manager");
    let view_provider = connect_to_protocol::<ui_app::ViewProviderMarker>()
        .expect("failed to connect to ViewProvider");
    let mut link_token_pair = scenic::flatland::ViewCreationTokenPair::new().unwrap();
    let app_monitor = AppMonitor::new(SAMPLE_APP_MONIKER.to_string());

    // Use view provider to initiate creation of the view which will be connected to the
    // viewport that we create below.
    view_provider
        .create_view2(ui_app::CreateView2Args {
            view_creation_token: Some(link_token_pair.view_creation_token),
            ..Default::default()
        })
        .expect("Cannot invoke create_view2");
    let _root_view_created =
        scene_manager.present_root_view(&mut link_token_pair.viewport_creation_token);
    app_monitor.wait_for_start_event().await;
    app_monitor.add_monitor_for_stop_event();

    // Collect trace.
    let trace = SingleSessionTrace::new();
    let collect_trace = !args.trace_config.is_empty();
    if collect_trace {
        info!("Collecting trace: {:?}", args.trace_config);
        trace.initialize(args.trace_config).await.unwrap();
        trace.start().await.unwrap();
    }
    // Let the UI App run for [run_duration_sec], then the test_runner will shutdown the sample_app
    let duration_sec = args.run_duration_sec;
    info!("Running sample app for {:?} sec", duration_sec);
    fasync::Timer::new(duration_sec).await;

    if collect_trace {
        trace.stop().await.unwrap();
        trace.terminate().await.unwrap();
    }

    info!("Taking screenshot.");
    take_screenshot().await;

    info!("Checking that sample-app did not crash.");
    if app_monitor.has_seen_stop_event() {
        panic!("Sample app unexpectedly stopped");
    }
}

fn to_duration(duration_sec: &str) -> Result<fasync::MonotonicDuration, String> {
    Ok(fasync::MonotonicDuration::from_seconds(duration_sec.parse::<i64>().unwrap()))
}

fn to_vector(configs: &str) -> Result<Vec<String>, String> {
    Ok(configs.split(',').map(|v| v.trim().to_string()).collect())
}
