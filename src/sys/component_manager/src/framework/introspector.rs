// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::{Arc, LazyLock};

use crate::model::component::ComponentInstance;
use crate::sandbox_util::take_handle_as_stream;
use anyhow::Context;
use cm_rust::Availability;
use cm_types::Name;
use fidl_fuchsia_component as fcomponent;
use futures::future::BoxFuture;
use futures::{FutureExt, TryStreamExt};
use log::warn;
use moniker::{ExtendedMoniker, Moniker};
use routing::error::RoutingError;
use routing::policy::PolicyError;

use crate::model::component::WeakComponentInstance;
use crate::model::routing::report_routing_failure;
use crate::model::token::InstanceToken;

static INTROSPECTOR_SERVICE: LazyLock<Name> =
    LazyLock::new(|| "fuchsia.component.Introspector".parse().unwrap());

pub fn serve(
    server_end: zx::Channel,
    target: WeakComponentInstance,
    source: WeakComponentInstance,
) -> BoxFuture<'static, Result<(), anyhow::Error>> {
    async move {
        if let Err(err) = check_access_permissions(&target, &source) {
            if let Ok(target) = target.upgrade() {
                report_routing_failure(
                    "protocol `fuchsia.component.Introspector`",
                    Some(Availability::Required),
                    &target,
                    &err,
                )
                .await;
            }
            return Err(err.into());
        }
        let mut stream = take_handle_as_stream::<fcomponent::IntrospectorMarker>(server_end);
        let source = source.upgrade()?;
        while let Some(request) = stream.try_next().await? {
            let method_name = request.method_name();
            handle_request(request, &source)
                .await
                .with_context(|| format!("Error handling Introspector method {method_name}"))?;
        }
        Ok(())
    }
    .boxed()
}

async fn handle_request(
    request: fcomponent::IntrospectorRequest,
    scope: &Arc<ComponentInstance>,
) -> Result<(), fidl::Error> {
    match request {
        fcomponent::IntrospectorRequest::GetMoniker { component_instance, responder } => {
            let token = InstanceToken::from(component_instance);
            let Some(Ok(moniker)) = scope
                .context
                .instance_registry()
                .get(&token)
                .map(|moniker| moniker.strip_prefix(&scope.moniker))
            else {
                return responder.send(Err(fcomponent::Error::InstanceNotFound));
            };
            return responder.send(Ok(moniker.as_ref()));
        }
        fcomponent::IntrospectorRequest::_UnknownMethod {
            ordinal,
            control_handle: _,
            method_type,
            ..
        } => {
            warn!(ordinal:%; "Unknown {method_type:?} Introspector method");
            Ok(())
        }
    }
}

fn check_access_permissions(
    target: &WeakComponentInstance,
    scope: &WeakComponentInstance,
) -> Result<(), RoutingError> {
    static MEMORY_MONITOR: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/core/memory_monitor2").unwrap());
    static DRIVER_MANAGER: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/bootstrap/driver_manager").unwrap());
    /// Moniker for integration tests.
    static RECEIVER: LazyLock<Moniker> = LazyLock::new(|| Moniker::parse_str("/receiver").unwrap());
    static ELF_TEST_RUNNER: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/core/testing/elf_test_runner").unwrap());
    static FUZZ_TEST_RUNNER: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/core/testing/fuzz_test_runner").unwrap());
    static GO_TEST_RUNNER: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/core/testing/go_test_runner").unwrap());
    static GTEST_TEST_RUNNER: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/core/testing/gunit_runner").unwrap());
    static GUNIT_TEST_RUNNER: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/core/testing/gunit_runner").unwrap());
    static RUST_TEST_RUNNER: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/core/testing/rust_test_runner").unwrap());
    static STARNIX_TEST_RUNNER: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/core/testing/starnix_test_runner").unwrap());
    static ZXTEST_TEST_RUNNER: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/core/testing/zxtest_runner").unwrap());
    static TEST_REALMS: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/core/testing").unwrap());
    static TEST_MANAGER_REALMS: LazyLock<Moniker> =
        LazyLock::new(|| Moniker::parse_str("/core/test_manager").unwrap());
    static STARNIX_TESTS: LazyLock<Name> = LazyLock::new(|| "starnix-tests".parse().unwrap());
    // TODO(https://fxbug.dev/318904493): Temporary workaround to prevent other components from
    // using `Introspector` while improvements to framework capability allowlists are under way.
    //
    // In production, the capability is minted at `/`, then offered to `/core/memory_monitor`.
    //
    // In the `introspector-integration-test`, the capability is minted at some test specific
    // realm, then exposed from `/`.
    //
    // In starnix tests, the capability is minted at some realm inside
    // `/core/testing/starnix-tests`, then used by some realm inside
    // `/core/testing/starnix-tests`.
    //
    // In driver test realm tests, the capability is minted at some realm inside
    // a test realm, then used by the driver manager in the same test realm.
    //
    // All other cases are disallowed.
    let is_starnix_test_realm = |moniker: &Moniker| {
        moniker.path().len() > TEST_REALMS.path().len()
            && moniker.has_prefix(&TEST_REALMS)
            && moniker.path()[TEST_REALMS.path().len()].collection() == Some(&*STARNIX_TESTS)
    };
    let is_test_realm = |moniker: &Moniker| {
        (moniker.path().len() > TEST_REALMS.path().len()
            && moniker.has_prefix(&TEST_REALMS)
            && moniker.path()[TEST_REALMS.path().len()].collection().is_some())
            || (moniker.path().len() > TEST_MANAGER_REALMS.path().len()
                && moniker.has_prefix(&TEST_MANAGER_REALMS)
                && moniker.path()[TEST_MANAGER_REALMS.path().len()].collection().is_some())
    };
    if target.moniker != *MEMORY_MONITOR
        && target.moniker != *DRIVER_MANAGER
        && target.moniker != *RECEIVER
        && target.moniker != *ELF_TEST_RUNNER
        && target.moniker != *FUZZ_TEST_RUNNER
        && target.moniker != *GO_TEST_RUNNER
        && target.moniker != *GTEST_TEST_RUNNER
        && target.moniker != *GUNIT_TEST_RUNNER
        && target.moniker != *RUST_TEST_RUNNER
        && target.moniker != *STARNIX_TEST_RUNNER
        && target.moniker != *ZXTEST_TEST_RUNNER
        && !target.moniker.is_root()
        && !(is_starnix_test_realm(&target.moniker) && is_starnix_test_realm(&scope.moniker))
        && !(is_test_realm(&target.moniker)
            && is_test_realm(&scope.moniker)
            && target.moniker.has_prefix(&scope.moniker)
            && target.moniker.leaf().and_then(|l| Some(l.name().into())) == Some("driver_manager"))
    {
        return Err(RoutingError::from(PolicyError::CapabilityUseDisallowed {
            cap: INTROSPECTOR_SERVICE.to_string(),
            source_moniker: ExtendedMoniker::ComponentInstance(scope.moniker.clone()),
            target_moniker: target.moniker.clone(),
        }));
    }
    Ok(())
}
