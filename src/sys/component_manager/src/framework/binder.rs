// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::{StartReason, WeakComponentInstance};
use crate::model::routing::report_routing_failure;
use crate::model::start::Start;
use cm_rust::Availability;
use cm_types::Name;
use errors::ModelError;
use futures::FutureExt;
use futures::future::BoxFuture;
use log::warn;
use moniker::Moniker;
use std::sync::LazyLock;

static BINDER_SERVICE: LazyLock<Name> =
    LazyLock::new(|| "fuchsia.component.Binder".parse().unwrap());

pub fn serve(
    server_end: zx::Channel,
    target: WeakComponentInstance,
    source: WeakComponentInstance,
) -> BoxFuture<'static, Result<(), anyhow::Error>> {
    async move {
        let res = serve_inner(server_end, target.moniker.clone(), source).await;
        if let Err(err) = res {
            let ret = anyhow::format_err!("{:?}", err);
            report_routing_failure_to_target(target, err).await;
            return Err(ret);
        }
        Ok(())
    }
    .boxed()
}

pub async fn serve_inner(
    server_end: zx::Channel,
    target_moniker: Moniker,
    source: WeakComponentInstance,
) -> Result<(), ModelError> {
    let source = source.upgrade()?;
    source
        .ensure_started(&StartReason::AccessCapability {
            target: target_moniker,
            name: BINDER_SERVICE.clone(),
        })
        .await?;
    source.scope_to_runtime(server_end).await;
    Ok(())
}

async fn report_routing_failure_to_target(target: WeakComponentInstance, err: ModelError) {
    match target.upgrade().map_err(|e| ModelError::from(e)) {
        Ok(target) => {
            report_routing_failure(
                "protocol `fuchsia.component.Binder`",
                Some(Availability::Required),
                &target,
                &err,
            )
            .await;
        }
        Err(err) => {
            warn!(moniker:% = target.moniker, error:% = err; "failed to upgrade reference");
        }
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::builtin_environment::BuiltinEnvironment;
    use crate::model::testing::test_helpers::*;
    use ::routing::component_instance::ComponentInstanceInterface;
    use assert_matches::assert_matches;
    use cm_rust::ComponentDecl;
    use cm_rust_testing::*;
    use fidl::client::Client;
    use fidl::encoding::DefaultFuchsiaResourceDialect;
    use fidl::handle::AsyncChannel;
    use fidl_fuchsia_component as fcomponent;
    use futures::StreamExt;
    use futures::lock::Mutex;
    use hooks::EventType;
    use std::sync::Arc;

    struct BinderCapabilityTestFixture {
        builtin_environment: Arc<Mutex<BuiltinEnvironment>>,
    }

    impl BinderCapabilityTestFixture {
        async fn new(components: Vec<(&'static str, ComponentDecl)>) -> Self {
            let TestModelResult { builtin_environment, .. } =
                TestEnvironmentBuilder::new().set_components(components).build().await;

            BinderCapabilityTestFixture { builtin_environment }
        }

        async fn new_event_stream(&self, events: Vec<EventType>) -> fcomponent::EventStreamProxy {
            let builtin_environment_guard = self.builtin_environment.lock().await;
            new_event_stream(&*builtin_environment_guard, events).await
        }

        async fn open_binder(
            &self,
            source: Moniker,
            target: Moniker,
        ) -> (zx::Channel, Result<(), anyhow::Error>) {
            let builtin_environment = self.builtin_environment.lock().await;
            let source = builtin_environment
                .model
                .root()
                .find_and_maybe_resolve(&source)
                .await
                .expect("failed to look up source moniker");
            let target = builtin_environment
                .model
                .root()
                .find_and_maybe_resolve(&target)
                .await
                .expect("failed to look up target moniker");
            let (client_end, server_end) = zx::Channel::create();
            let res = serve(server_end, target.as_weak(), source.as_weak()).await;
            (client_end, res)
        }
    }

    #[fuchsia::test]
    async fn component_starts_on_open() {
        let fixture = BinderCapabilityTestFixture::new(vec![
            (
                "root",
                ComponentDeclBuilder::new().child_default("source").child_default("target").build(),
            ),
            ("source", component_decl_with_test_runner()),
            ("target", component_decl_with_test_runner()),
        ])
        .await;
        let event_stream =
            fixture.new_event_stream(vec![EventType::Resolved, EventType::Started]).await;
        let moniker: Moniker = ["source"].try_into().unwrap();
        let (_client_end, binder_res) =
            fixture.open_binder(moniker.clone(), ["target"].try_into().unwrap()).await;
        binder_res.expect("failed to bind");

        let events = get_n_events(&event_stream, 4).await;
        assert_event_type_and_moniker(&events[0], fcomponent::EventType::Resolved, Moniker::root());
        assert_event_type_and_moniker(&events[1], fcomponent::EventType::Resolved, &moniker);
        assert_event_type_and_moniker(&events[2], fcomponent::EventType::Resolved, "target");
        assert_event_type_and_moniker(&events[3], fcomponent::EventType::Started, &moniker);
    }

    // TODO(https://fxbug.dev/42073225): Figure out a way to test this behavior.
    #[ignore]
    #[fuchsia::test]
    async fn channel_is_closed_if_component_does_not_exist() {
        let fixture = BinderCapabilityTestFixture::new(vec![(
            "root",
            ComponentDeclBuilder::new()
                .child_default("target")
                .child_default("unresolvable")
                .build(),
        )])
        .await;
        let moniker: Moniker = ["foo"].try_into().unwrap();
        let (client_end, binder_res) = fixture.open_binder(moniker.clone(), Moniker::root()).await;
        binder_res.expect_err("should have failed to bind");

        let client_end = AsyncChannel::from_channel(client_end);
        let client = Client::<DefaultFuchsiaResourceDialect>::new(client_end, "binder_service");
        let mut event_receiver = client.take_event_receiver();
        assert_matches!(
            event_receiver.next().await,
            Some(Err(fidl::Error::ClientChannelClosed {
                epitaph,
                protocol_name: "binder_service",
                ..
            })) if epitaph == zx::Status::NOT_FOUND
        );
        assert_matches!(event_receiver.next().await, None);
    }
}
