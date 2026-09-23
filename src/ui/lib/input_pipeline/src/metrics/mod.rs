// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Dispatcher, Incoming, Transport, dispatcher};
use anyhow::{Context as _, Error};
use cobalt_client::traits::AsEventCode;
use derivative::Derivative;
use fidl_next_fuchsia_metrics as metrics;
use log::warn;
use metrics_registry::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Connects to the MetricEventLoggerFactory service to create a
/// MetricEventLoggerProxy for the caller.
fn create_metrics_logger(
    incoming: &Incoming,
) -> Result<
    (
        fidl_next::Client<metrics::MetricEventLogger, Transport>,
        dispatcher::TaskHandle<()>,
        Arc<AtomicBool>,
    ),
    Error,
> {
    let factory_proxy = incoming
        .connect_protocol_next::<metrics::MetricEventLoggerFactory>()
        .context("connecting to metrics")?;
    let factory_proxy = factory_proxy.spawn();

    let (cobalt_proxy, cobalt_server) =
        fidl_next::fuchsia::create_channel::<metrics::MetricEventLogger>();
    let cobalt_proxy = Dispatcher::client_from_zx_channel(cobalt_proxy).spawn();

    let project_spec = metrics::ProjectSpec {
        customer_id: None, // defaults to fuchsia
        project_id: Some(PROJECT_ID),
        ..Default::default()
    };

    let completed = Arc::new(AtomicBool::new(false));
    let completed_clone = completed.clone();
    let task = Dispatcher::spawn_local(async move {
        match factory_proxy.create_metric_event_logger(&project_spec, cobalt_server).await {
            Err(e) => warn!("FIDL failure setting up event logger: {e:?}"),
            Ok(Err(e)) => warn!("CreateMetricEventLogger failure: {e:?}"),
            Ok(Ok(_)) => {}
        }
        completed_clone.store(true, Ordering::SeqCst);
    });

    Ok((cobalt_proxy, task, completed))
}

fn log_on_failure<T: std::fmt::Debug>(result: Result<Result<T, metrics::Error>, fidl_next::Error>) {
    match result {
        Ok(Ok(_)) => (),
        e => warn!("failed to log metrics: {:?}", e),
    };
}

/// A client connection to the Cobalt logging service.
#[derive(Clone, Derivative, Default)]
#[derivative(Debug)]
pub struct MetricsLogger {
    #[derivative(Debug = "ignore")]
    logger: Option<fidl_next::Client<metrics::MetricEventLogger, Transport>>,
    tasks: Arc<Mutex<Vec<(dispatcher::TaskHandle<()>, Arc<AtomicBool>)>>>,
}

impl MetricsLogger {
    pub fn new(incoming: &Incoming) -> Self {
        let tasks = Arc::new(Mutex::new(vec![]));
        let logger = match create_metrics_logger(incoming) {
            Ok((logger, init_task, completed)) => {
                tasks.lock().unwrap().push((init_task, completed));
                Some(logger)
            }
            Err(e) => {
                warn!("Failed to create metrics logger: {e}");
                None
            }
        };
        Self { logger, tasks }
    }

    /// Logs an warning occurrence metric using the Cobalt logger. Does not block execution.
    pub fn log_warn<E: AsEventCode, S: Into<String>>(&self, event_code: E, message: S) {
        log::warn!("{}", message.into());
        self.send_metric(event_code);
    }

    /// Logs an error occurrence metric using the Cobalt logger. Does not block execution.
    pub fn log_error<E: AsEventCode, S: Into<String>>(&self, event_code: E, message: S) {
        log::error!("{}", message.into());
        self.send_metric(event_code);
    }

    // send metric, does not block the execution.
    fn send_metric<E: AsEventCode>(&self, event_code: E) {
        let Some(c) = self.logger.clone() else { return };
        let code = event_code.as_event_code();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_clone = completed.clone();
        let task = Dispatcher::spawn_local(async move {
            log_on_failure(c.log_occurrence(INPUT_PIPELINE_ERROR_METRIC_ID, 1, &[code]).await);
            completed_clone.store(true, Ordering::SeqCst);
        });
        let mut tasks = self.tasks.lock().unwrap();
        tasks.retain(|(_, completed)| !completed.load(Ordering::SeqCst));
        tasks.push((task, completed));
    }
}
