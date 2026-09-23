// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/410430313): Figure out how to manage clippy configs for targets that are
// building in both GN and Bazel.
#![warn(clippy::all)]

use fuchsia_async::TimeoutExt;
use futures::future::BoxFuture;
use futures::lock::Mutex;
use futures::prelude::*;
use http_body_util::BodyExt;
use hyper::{Request, Response};
type Body = http_body_util::Full<hyper::body::Bytes>;
use omaha_client::app_set::VecAppSet;
use omaha_client::common::App;
use omaha_client::configuration::{Config, Updater};
use omaha_client::cup_ecdsa::{PublicKeyAndId, PublicKeyId, PublicKeys, StandardCupv2Handler};
use omaha_client::http_request::{Error, HttpRequest};
use omaha_client::installer::stub::StubInstaller;
use omaha_client::metrics::StubMetricsReporter;
use omaha_client::policy::StubPolicyEngine;
use omaha_client::protocol::Cohort;
use omaha_client::protocol::request::OS;
use omaha_client::state_machine::{
    StateMachineBuilder, StateMachineEvent, UpdateCheckError, update_check,
};
use omaha_client::storage::MemStorage;
use omaha_client::time::StandardTimeSource;
use omaha_client::time::timers::StubTimer;
use std::error;
use std::rc::Rc;
use std::time::Duration;

pub struct FuchsiaHyperHttpRequest {
    client: fuchsia_hyper::HttpsClient,
}

impl HttpRequest for FuchsiaHyperHttpRequest {
    fn request(&mut self, req: Request<Body>) -> BoxFuture<'_, Result<Response<Vec<u8>>, Error>> {
        collect_from_future(self.client.request(req))
            .on_timeout(Duration::from_secs(10), || Err(Error::new_timeout()))
            .boxed()
    }
}

async fn collect_from_future(
    response_future: impl Future<
        Output = Result<Response<hyper::body::Incoming>, hyper_util::client::legacy::Error>,
    >,
) -> Result<Response<Vec<u8>>, Error> {
    let response = response_future.await.map_err(Error::new_transport)?;
    let (parts, body) = response.into_parts();
    let bytes = body.collect().await.map_err(Error::new_transport)?.to_bytes();
    Ok(Response::from_parts(parts, bytes.to_vec()))
}

impl FuchsiaHyperHttpRequest {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        FuchsiaHyperHttpRequest { client: fuchsia_hyper::new_https_client() }
    }
}

#[derive(argh::FromArgs)]
#[argh(description = "Fake Omaha client")]
struct FakeOmahaClientArgs {
    #[argh(option, description = "omaha server URL")]
    server: String,
    #[argh(option, description = "public key ID (integer)")]
    key_id: PublicKeyId,
    #[argh(option, description = "public key (ECDSA), .pem format")]
    key: String,
    #[argh(option, description = "omaha app ID")]
    app_id: String,
    #[argh(option, description = "omaha channel")]
    channel: String,
}

fn main() -> Result<(), Box<dyn error::Error>> {
    let args: FakeOmahaClientArgs = argh::from_env();
    main_inner(args)
}

fn main_inner(args: FakeOmahaClientArgs) -> Result<(), Box<dyn error::Error>> {
    let omaha_public_keys = PublicKeys {
        latest: PublicKeyAndId { id: args.key_id, key: args.key.parse()? },
        historical: vec![],
    };
    let config = Config {
        updater: Updater { name: "updater".to_string(), version: [1, 2, 3, 4].into() },
        os: OS {
            platform: "platform".to_string(),
            version: "0.1.2.3".to_string(),
            service_pack: "sp".to_string(),
            arch: "test_arch".to_string(),
        },
        service_url: args.server.to_string(),
        omaha_public_keys: Some(omaha_public_keys.clone()),
    };
    let app_set = VecAppSet::new(vec![
        App::builder()
            .id(args.app_id)
            .version([20200101, 0, 0, 0])
            .cohort(Cohort::new(&args.channel))
            .build(),
    ]);

    let state_machine = StateMachineBuilder::new(
        /*policy_engine=*/ StubPolicyEngine::new(StandardTimeSource),
        /*http=*/ FuchsiaHyperHttpRequest::new(),
        /*installer=*/ StubInstaller { should_fail: false },
        /*timer=*/ StubTimer {},
        /*metrics_reporter=*/ StubMetricsReporter {},
        /*storage=*/ Rc::new(Mutex::new(MemStorage::new())),
        /*config=*/ config,
        /*app_set=*/ Rc::new(Mutex::new(app_set)),
        /*cup_handler=*/ Some(StandardCupv2Handler::new(&omaha_public_keys)),
    );

    let stream: Vec<StateMachineEvent> = fuchsia_async::LocalExecutorBuilder::new()
        .build()
        .run_singlethreaded(async { state_machine.oneshot_check().await.collect().await });
    let mut result: Vec<Result<update_check::Response, UpdateCheckError>> = stream
        .into_iter()
        .filter_map(|p| match p {
            StateMachineEvent::UpdateCheckResult(val) => Some(val),
            _ => None,
        })
        .collect();

    let _ = result.pop().unwrap()?;
    Ok(())
}
