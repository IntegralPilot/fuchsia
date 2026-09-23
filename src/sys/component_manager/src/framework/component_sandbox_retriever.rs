// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::WeakComponentInstance;
use ::routing::bedrock::sandbox_construction::{ComponentSandbox, ProgramInput};
use ::routing::bedrock::structured_dict::ComponentInput;
use ::routing::component_instance::ComponentInstanceInterface;
use anyhow::{Error, format_err};
use capability_source::{BuiltinSource, CapabilitySource, InternalCapability};
use cm_types::Name;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_component_internal as finternal;
use fidl_fuchsia_component_runtime::RouteRequest;
use futures::future::BoxFuture;
use futures::{FutureExt, StreamExt};
use runtime_capabilities::{Dictionary, WeakInstanceToken};
use std::sync::Arc;

pub fn serve(
    chan: zx::Channel,
    _target: WeakComponentInstance,
    source: WeakComponentInstance,
) -> BoxFuture<'static, Result<(), Error>> {
    async move {
        let mut stream =
            ServerEnd::<finternal::ComponentSandboxRetrieverMarker>::new(chan).into_stream();
        while let Some(Ok(request)) = stream.next().await {
            match request {
                finternal::ComponentSandboxRetrieverRequest::GetMySandbox { responder } => {
                    let source = source
                        .upgrade()
                        .map_err(|e| format_err!("failed to upgrade component: {:?}", e))?;
                    let remote_capabilities = source.context.remote_capabilities().clone();
                    let ComponentSandbox {
                        component_input,
                        component_output,
                        program_input,
                        program_output_dict,
                        capability_sourced_capabilities_dict,
                        declared_dictionaries,
                        child_inputs,
                        collection_inputs,
                        ..
                    } = source
                        .lock_resolved_state()
                        .await
                        .map_err(|e| format_err!("failed to resolve component: {:?}", e))?
                        .sandbox
                        .clone();
                    if !is_dispatcher_runner(&program_input, source.as_weak().into()).await {
                        // This API is explerimental and making it widely available has security
                        // implications. To allow us to get a bit of mileage on it to determine the
                        // correct shape for it, we currently only allow connections to this
                        // protocol if the client is a built-in component.
                        return Err(format_err!("only accessible from built-in components"));
                    }
                    let to_event_pair = |dictionary: Arc<Dictionary>| {
                        let (e1, e2) = zx::EventPair::create();
                        remote_capabilities.store(e1, dictionary).expect("we used a valid handle");
                        e2
                    };
                    let child_input_to_fidl =
                        |(name, input): (Name, ComponentInput)| finternal::ChildInput {
                            child_name: name.to_string(),
                            child_input: to_event_pair(input.into()),
                        };
                    responder.send(finternal::ComponentSandbox {
                        component_input: Some(to_event_pair(component_input.into())),
                        component_output: Some(to_event_pair(component_output.into())),
                        program_input: Some(to_event_pair(program_input.into())),
                        program_output: Some(to_event_pair(program_output_dict)),
                        capability_sourced: Some(to_event_pair(
                            capability_sourced_capabilities_dict,
                        )),
                        declared_dictionaries: Some(to_event_pair(declared_dictionaries)),
                        child_inputs: Some(
                            child_inputs.enumerate().map(child_input_to_fidl).collect(),
                        ),
                        collection_inputs: Some(
                            collection_inputs.enumerate().map(child_input_to_fidl).collect(),
                        ),
                        ..Default::default()
                    })?;
                }
                ord => {
                    return Err(format_err!("unrecognized ordinal: {:?}", ord));
                }
            }
        }
        Ok(())
    }
    .boxed()
}

async fn is_dispatcher_runner(
    program_input: &ProgramInput,
    target: Arc<WeakInstanceToken>,
) -> bool {
    let Some(runner_router) = program_input.runner() else {
        return false;
    };
    let Ok(source) = runner_router.route_debug(RouteRequest::default(), target).await else {
        return false;
    };
    let CapabilitySource::Builtin(BuiltinSource { capability: InternalCapability::Runner(name) }) =
        source
    else {
        return false;
    };
    name == Name::new("builtin_dispatcher").unwrap() || name == Name::new("test_runner").unwrap()
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::testing::test_helpers::*;
    use crate::model::testing::test_hook::*;
    use ::routing::component_instance::ComponentInstanceInterface;
    use cm_config::RuntimeConfig;
    use cm_rust::{ComponentDecl, ExposeSource};
    use cm_rust_testing::*;
    use fuchsia_async as fasync;
    use std::sync::Arc;

    async fn get_sandbox(
        components: Vec<(&'static str, ComponentDecl)>,
    ) -> Option<finternal::ComponentSandbox> {
        let config = RuntimeConfig { list_children_batch_size: 2, ..Default::default() };
        let hook = Arc::new(TestHook::new());
        let test = TestEnvironmentBuilder::new()
            .set_runtime_config(config)
            .set_components(components)
            .set_front_hooks(hook.hooks())
            .build()
            .await;

        // Look up and start component.
        let component = test.model.root().clone();

        let (proxy, server_end) =
            fidl::endpoints::create_proxy::<finternal::ComponentSandboxRetrieverMarker>();
        let _serving_task = fasync::Task::spawn(async move {
            if let Err(e) =
                serve(server_end.into_channel(), component.as_weak(), component.as_weak()).await
            {
                log::warn!("failure in serve function: {e:?}");
            }
        });
        proxy.get_my_sandbox().await.ok()
    }

    #[fuchsia::test]
    async fn builtin_runner() {
        // ComponentDeclBuilder::new() automatically sets our runner to `test_runner`, which is a
        // built-in runner
        let components = vec![("root", ComponentDeclBuilder::new().build())];
        assert!(get_sandbox(components).await.is_some());
    }

    #[fuchsia::test]
    async fn invalid_runner() {
        let components = vec![(
            "root",
            ComponentDeclBuilder::new_empty_component().program_runner("nonexistent").build(),
        )];
        assert!(get_sandbox(components).await.is_none());
    }

    #[fuchsia::test]
    async fn non_builtin_runner() {
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new_empty_component()
                    .child_default("child")
                    .use_(UseBuilder::runner().name("example_runner").source_static_child("child"))
                    .build(),
            ),
            (
                "child",
                ComponentDeclBuilder::new()
                    .capability(CapabilityBuilder::runner().name("example_runner"))
                    .expose(
                        ExposeBuilder::runner().name("example_runner").source(ExposeSource::Self_),
                    )
                    .build(),
            ),
        ];
        assert!(get_sandbox(components).await.is_none());
    }

    #[fuchsia::test]
    async fn no_runner() {
        let components = vec![("root", ComponentDeclBuilder::new_empty_component().build())];
        assert!(get_sandbox(components).await.is_none());
    }
}
