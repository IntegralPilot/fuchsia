// Copyright 2024 The Fuchsia Authors
//
// Licensed under a BSD-style license <LICENSE-BSD>, Apache License, Version 2.0
// <LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0>, or the MIT
// license <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your option.
// This file may not be copied, modified, or distributed except according to
// those terms.

use {
    anyhow::Error,
    app_set::{AppMetadata, MinimalAppSet},
    argh::FromArgs,
    futures::{FutureExt as _, StreamExt, lock::Mutex, stream::FuturesUnordered},
    http_request::MinimalHttpRequest,
    log::{error, info},
    metrics::MinimalMetricsReporter,
    omaha_client::{
        common::App,
        configuration::{Config, Updater},
        cup_ecdsa::StandardCupv2Handler,
        protocol::{Cohort, request::OS},
        state_machine::StateMachineBuilder,
        time::StandardTimeSource,
    },
    std::rc::Rc,
};

mod app_set;
mod http_request;
mod installer;
mod metrics;
mod policy;
mod storage;
mod timer;

/// App ID of the application for which an update is checked / requested.
/// The omaha service must know this app ID for this example program to work.
const APP_ID: &str = "appid_01";

#[derive(FromArgs)]
/// Arguments for the omaha-client hello world example
struct Args {
    #[argh(
        option,
        short = 'u',
        description = "URL of the omaha service to connect to, e.g. 'http://[::]:1234'. This service must accept connections."
    )]
    url: String,
}

#[tokio::main]
async fn main() {
    info!("Starting omaha client...");

    if let Err(e) = main_inner().await {
        error!("Error running omaha-client: {:#}", e);
        std::process::exit(1);
    }

    info!("Shutting down omaha client...");
}

/// This demo code sets up a minimal set of structures to get the omaha-client lib's
/// state machine up and running, and then logs the state machine events, including
/// the results of requests to the update sever to stdout.
/// It interacts with the actual service used to keep Fuchsia-based smart displays
/// updated.
async fn main_inner() -> Result<(), Error> {
    let args: Args = argh::from_env();

    // HTTP
    let http = MinimalHttpRequest::new();

    // Example platform config
    let platform_config = Config {
        updater: Updater {
            // The following values are from a hypothetical Fuchsia device.
            name: "FuchsiaUpdater".to_string(),
            version: [0, 0, 1, 0].into(),
        },
        os: OS {
            // The following values are from a hypothetical Fuchsia device.
            platform: "Fuchsia".to_string(),
            version: "14.20230831.4.72".to_string(),
            service_pack: "".to_string(),
            arch: "aarch64".to_string(),
        },
        service_url: args.url,
        omaha_public_keys: None,
    };

    // The cup handler is required for the state machine, but does not require explicit
    // initialization, the standard handler will do just fine.
    let cup_handler: Option<StandardCupv2Handler> =
        platform_config.omaha_public_keys.as_ref().map(StandardCupv2Handler::new);

    // Example app and app_set we use for this hello world program.
    // The following app corresponds to the system image for the Nest Hub 1st gen ("Astro")
    // for all devices currently tracking the stable channel.
    // app.id is unique to the package we are checking for (in this example the system OTA for Astro).
    // app.version is the version we tell the server we currently have. Based on this it will decide
    //      what version the installer should upgrade to, and provide the data accordingly.
    // app.cohort tells the server what channel the device is on. This allows the server to decide if
    //      a particular device should get updates or not. For instance, if a staged rollout is
    //      planned, the server might count how many devices it has told to update and will stop
    //      handing out "should update" messages to the rest of the cohort once a certain number
    //      has been reached.
    // Things to try here:
    // Setting .version([14, 20230831, 4, 72]) will return an update to a newer image on the stable
    //      channel. So, when watching the stream of events printed to stdout while
    //      running this application, something like this will be in the event log:
    //      ...
    //      update_check: Some(
    //        UpdateCheck {
    //          status: Ok,
    //          info: None,
    //          urls: Some(
    //            URLs {
    //              url: [
    //                URL {
    //                  codebase: "fuchsia-pkg://<update URL>/",
    //                },
    //              ],
    //            },
    //          ),
    //          manifest: Some(
    //            Manifest {
    //              `version: "<some newer version than 14.x.y.z>",
    //
    let app = App::builder()
        .id(APP_ID)
        .version([14, 20230831, 4, 72])
        .cohort(Cohort::from_hint("stable-channel"))
        .build();

    let app_set = Rc::new(Mutex::new(MinimalAppSet::new(
        app,
        AppMetadata { appid_source: app_set::AppIdSource::DefaultEmpty },
    )));

    // Installer
    // Using the minimal installer in this example since we don't install
    // anything on an actual device. It always returns a successful install
    // to the state machine.
    let installer = installer::MinimalInstaller { should_fail: false };

    // Metrics
    // Using the minimal reporter for this example. It only logs metrics via log::info.
    let metrics_reporter = MinimalMetricsReporter;

    // Storage
    // The state machine expects the caller to provide key/value store implementations to
    // hold data for it. In this demo example we don't store anything, hence
    // the implementation is just a stub.
    let stash = storage::MinimalStorage;
    let stash_ref = Rc::new(Mutex::new(stash));

    // Policy
    // The policy is the client-side implementation of the conditions under which it performs
    // updates, checks for new updates, etc. For this example we use a simple policy which
    // will check frequently, and is always ready to perform an update.
    // Things to try here: Add functionality to the policy, e.g. adjust the update interval
    // based on some criteria.
    let policy_engine =
        policy::MinimalPolicyEngine::new(StandardTimeSource, metrics_reporter.clone());

    let futures = FuturesUnordered::new();

    // Start the omaha-client lib state machine
    let (_state_machine_control, state_machine) = StateMachineBuilder::new(
        policy_engine,
        http,
        installer,
        timer::MinimalTimer,
        metrics_reporter,
        Rc::clone(&stash_ref),
        platform_config.clone(),
        Rc::clone(&app_set),
        cup_handler,
    )
    .start()
    .await;

    // Catch and print the events from the state machine
    futures.push(
        async move {
            futures::pin_mut!(state_machine);

            while let Some(event) = state_machine.next().await {
                println!("Event: {:#?}", event);
            }
        }
        .boxed_local(),
    );

    futures.collect::<()>().await;

    Ok(())
}
