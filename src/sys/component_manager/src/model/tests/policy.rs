// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::{ComponentInstance, WeakComponentInstance, WeakExtendedInstance};
use crate::model::context::ModelContext;
use anyhow::Error;
use async_trait::async_trait;
use fidl_fuchsia_component_decl as fdecl;
use hooks::Hooks;
use moniker::Moniker;
use routing::bedrock::structured_dict::ComponentInput;
use routing_test_helpers::instantiate_global_policy_checker_tests;
use routing_test_helpers::policy::GlobalPolicyCheckerTest;
use std::sync::Arc;

// Tests `GlobalPolicyChecker` methods for `ComponentInstance`s.
#[derive(Default)]
#[allow(dead_code)]
struct GlobalPolicyCheckerTestForCm {}

#[async_trait]
impl GlobalPolicyCheckerTest<ComponentInstance> for GlobalPolicyCheckerTestForCm {
    async fn make_component(&self, moniker: Moniker) -> Arc<ComponentInstance> {
        ComponentInstance::new(
            ComponentInput::default(),
            moniker,
            0,
            "test:///bar".parse().unwrap(),
            fdecl::StartupMode::Lazy,
            fdecl::OnTerminate::None,
            None,
            Arc::new(ModelContext::new_for_test()),
            WeakExtendedInstance::Component(WeakComponentInstance::default()),
            Arc::new(Hooks::new()),
            false,
        )
        .await
    }
}

instantiate_global_policy_checker_tests!(GlobalPolicyCheckerTestForCm);
