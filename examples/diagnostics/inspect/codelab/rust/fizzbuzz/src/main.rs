// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error};
use fidl_fuchsia_examples_inspect::{FizzBuzzRequest, FizzBuzzRequestStream};
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::{self as inspect, HistogramProperty, NumericProperty, component};

use futures::{StreamExt, TryStreamExt};
use log::{error, info};
use std::sync::Arc;

struct FizzBuzzServerMetrics {
    incoming_connection_count: inspect::UintProperty,
    closed_connection_count: inspect::UintProperty,
    request_count: inspect::UintProperty,
    request_time_histogram: inspect::UintExponentialHistogramProperty,
}

impl FizzBuzzServerMetrics {
    fn new() -> Self {
        let node = component::inspector().root().create_child("fizzbuzz_service");
        let metrics = Self {
            incoming_connection_count: node.create_uint("incoming_connection_count", 0),
            closed_connection_count: node.create_uint("closed_connection_count", 0),
            request_count: node.create_uint("request_count", 0),
            request_time_histogram: node.create_uint_exponential_histogram(
                "request_time_histogram_us",
                inspect::ExponentialHistogramParams {
                    floor: 1,
                    initial_step: 1,
                    step_multiplier: 2,
                    buckets: 16,
                },
            ),
        };
        component::inspector().root().record(node);
        metrics
    }
}

struct FizzBuzzServer {
    metrics: Arc<FizzBuzzServerMetrics>,
}

impl FizzBuzzServer {
    fn new(metrics: Arc<FizzBuzzServerMetrics>) -> Self {
        Self { metrics }
    }

    fn spawn(self, stream: FizzBuzzRequestStream) {
        fasync::Task::local(async move {
            self.metrics.incoming_connection_count.add(1);
            self.handle_request_stream(stream).await.unwrap_or_else(|e| {
                error!(e:?; "Error handling fizzbuzz request stream");
            });
            self.metrics.closed_connection_count.add(1);
        })
        .detach();
    }

    async fn handle_request_stream(&self, mut stream: FizzBuzzRequestStream) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await.context("serve fizzbuzz")? {
            let FizzBuzzRequest::Execute { count, responder } = request;
            self.metrics.request_count.add(1);
            let start_time = zx::MonotonicInstant::get();
            responder.send(&fizzbuzz(count)).context("send execute response")?;
            let stop_time = zx::MonotonicInstant::get();
            let time_micros = (stop_time - start_time).into_micros() as u64;
            self.metrics.request_time_histogram.insert(time_micros);
        }
        Ok(())
    }
}

fn fizzbuzz(n: u32) -> String {
    (1..=n)
        .into_iter()
        .map(|i| match (i % 3, i % 5) {
            (0, 0) => "FizzBuzz".to_string(),
            (0, _) => "Fizz".to_string(),
            (_, 0) => "Buzz".to_string(),
            (_, _) => format!("{}", i),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[fuchsia::main(logging_tags = ["inspect_rust_codelab", "fizzbuzz"])]
async fn main() -> Result<(), Error> {
    let mut fs = ServiceFs::new();

    info!("starting up...");

    let metrics = Arc::new(FizzBuzzServerMetrics::new());

    fs.dir("svc")
        .add_fidl_service(move |stream| FizzBuzzServer::new(metrics.clone()).spawn(stream));

    let _inspect_server_task = inspect_runtime::publish(
        component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );

    fs.take_and_serve_directory_handle()?;
    fs.collect::<()>().await;
    Ok(())
}
