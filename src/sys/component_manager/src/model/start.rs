// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::sync::Arc;

use async_trait::async_trait;

use super::component::{
    ComponentInstance, IncomingCapabilities, StartReason, WeakComponentInstance,
};
use crate::framework::controller;
use errors::{ActionError, StartActionError};

/// The interface to start a component.
///
/// If some code only needs to start a component, it can make for better
/// modularity to give them a `dyn Start` of sorts, as opposed to the whole
/// component instance object.
#[async_trait]
pub trait Start {
    /// Start the component without providing additional capabilities such as
    /// numbered handles or extra namespace entries. Idempotent.
    async fn ensure_started<'a>(&'a self, reason: &'a StartReason) -> Result<(), ActionError> {
        self.ensure_started_etc(reason, None, Default::default()).await
    }

    /// Start the component and possibly provide additional capabilities and
    /// controller. These extra resources are discarded if the component is
    /// already started, or if some start request raced with this request.
    async fn ensure_started_etc<'a>(
        &'a self,
        reason: &'a StartReason,
        execution_controller_task: Option<controller::ExecutionControllerTask>,
        incoming: IncomingCapabilities,
    ) -> Result<(), ActionError>;
}

#[async_trait]
impl Start for Arc<ComponentInstance> {
    async fn ensure_started_etc<'a>(
        &'a self,
        reason: &'a StartReason,
        execution_controller_task: Option<controller::ExecutionControllerTask>,
        incoming: IncomingCapabilities,
    ) -> Result<(), ActionError> {
        ComponentInstance::start(self, reason, execution_controller_task, incoming).await
    }
}

#[async_trait]
impl Start for WeakComponentInstance {
    async fn ensure_started_etc<'a>(
        &'a self,
        reason: &'a StartReason,
        execution_controller_task: Option<controller::ExecutionControllerTask>,
        incoming: IncomingCapabilities,
    ) -> Result<(), ActionError> {
        let Ok(component) = self.upgrade() else {
            return Err(ActionError::from(StartActionError::InstanceDestroyed {
                moniker: self.moniker.clone(),
            }));
        };
        component.ensure_started_etc(reason, execution_controller_task, incoming).await
    }
}
