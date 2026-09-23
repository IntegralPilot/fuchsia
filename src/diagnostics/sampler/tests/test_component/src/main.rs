// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_samplertestcontroller::*;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::component;
use fuchsia_sync::Mutex;
use futures::{FutureExt, StreamExt, TryStreamExt};
use std::collections::HashMap;
use std::sync::Arc;

const OBSERVED_ID: &str = "1111222233334444111111111111111111111111111111111111111111111111";
const IGNORED_ID: &str = "2222222233334444111111111111111111111111111111111111111111112222";

#[fuchsia::main]
async fn main() {
    let _inspect_server_task = inspect_runtime::publish(
        component::inspector(),
        inspect_runtime::PublishOptions::default(),
    );

    let mut fs = ServiceFs::new();
    log::info!("Started SamplerTestController");
    fs.dir("svc").add_fidl_service(serve_sampler_test_controller);

    fs.take_and_serve_directory_handle().unwrap();

    fs.collect::<()>().await
}

struct TestState {
    integer_property_map: HashMap<u16, (String, i64)>,
    optional_integer: Option<i64>,
    sample_count_callback_opt: Option<SamplerTestControllerWaitForSampleResponder>,
}

static INSPECT_NAME_FOR_OPTIONAL: &str = "optional";
impl TestState {
    pub fn new() -> Arc<Mutex<TestState>> {
        let mut integer_property_map: HashMap<u16, (String, i64)> = HashMap::new();
        integer_property_map.insert(1, ("counter".to_string(), 0));
        integer_property_map.insert(2, ("integer_1".to_string(), 10));
        integer_property_map.insert(3, ("integer_2".to_string(), 20));
        integer_property_map.insert(4, ("integer_3".to_string(), 30));
        integer_property_map.insert(5, (OBSERVED_ID.to_string(), 42));
        integer_property_map.insert(6, (IGNORED_ID.to_string(), 17));
        Arc::new(Mutex::new(TestState {
            integer_property_map,
            optional_integer: None,
            sample_count_callback_opt: None,
        }))
    }
}

fn add_lazy_sampled_node(
    parent: &fuchsia_inspect::Node,
    state: Arc<Mutex<TestState>>,
) -> fuchsia_inspect::LazyNode {
    parent.create_lazy_child("samples", move || {
        let state = state.clone();
        async move {
            let mut unwrapped = state.lock();

            if let Some(responder) = unwrapped.sample_count_callback_opt.take() {
                responder.send(Ok(())).unwrap();
            }

            let samples = fuchsia_inspect::Inspector::default();
            for (_, (name, int_val)) in unwrapped.integer_property_map.clone() {
                let () = samples.root().record_int(name, int_val);
            }
            if let Some(value) = unwrapped.optional_integer {
                let () = samples.root().record_int(INSPECT_NAME_FOR_OPTIONAL, value);
            }
            Ok(samples)
        }
        .boxed()
    })
}

fn serve_sampler_test_controller(mut stream: SamplerTestControllerRequestStream) {
    fasync::Task::spawn(async move {
        let state = TestState::new();
        let inspector = component::inspector();
        let _lazy_state_node = add_lazy_sampled_node(inspector.root(), state.clone());
        while let Some(req) = stream.try_next().await.unwrap() {
            match req {
                SamplerTestControllerRequest::IncrementInt { property_id, responder } => {
                    let mut unwrapped = state.lock();
                    unwrapped.integer_property_map.get_mut(&property_id).unwrap().1 += 1;
                    responder.send().unwrap();
                }
                SamplerTestControllerRequest::SetOptional { value, responder } => {
                    let mut unwrapped = state.lock();
                    unwrapped.optional_integer = Some(value);
                    responder.send().unwrap();
                }
                SamplerTestControllerRequest::RemoveOptional { responder } => {
                    let mut unwrapped = state.lock();
                    unwrapped.optional_integer = None;
                    responder.send().unwrap();
                }
                SamplerTestControllerRequest::WaitForSample { responder } => {
                    let mut unwrapped = state.lock();

                    if unwrapped.sample_count_callback_opt.is_some() {
                        responder.send(Err(SamplingError::MultipleSampleCallbacksError)).unwrap();
                        continue;
                    }

                    unwrapped.sample_count_callback_opt = Some(responder);
                }
            }
        }
    })
    .detach();
}
