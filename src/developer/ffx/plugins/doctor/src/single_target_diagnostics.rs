// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::doctor_ledger::{LedgerMode, LedgerNodeGuard, LedgerOutcome};
use discovery::TargetHandle;
use ffx_config::EnvironmentContext;
use ffx_diagnostics::{NotificationType, Notifier};
use ffx_diagnostics_checks::run_diagnostics_with_handle;
use fidl_fuchsia_developer_ffx::TargetInfo;
use std::io::Write;

pub(crate) struct LedgerNotifier<'a, 'b, W: Write> {
    ledger: &'a mut LedgerNodeGuard<'b, W>,
}

impl<'a, 'b, W: Write> LedgerNotifier<'a, 'b, W> {
    pub(crate) fn new(ledger: &'a mut LedgerNodeGuard<'b, W>) -> Self {
        Self { ledger }
    }
}

impl<W: Write> Notifier for LedgerNotifier<'_, '_, W> {
    fn update_status(
        &mut self,
        ty: NotificationType,
        status: impl Into<String>,
    ) -> anyhow::Result<()> {
        let ledger_outcome = match ty {
            NotificationType::Info => LedgerOutcome::Info,
            NotificationType::Success => LedgerOutcome::Success,
            NotificationType::Warning => LedgerOutcome::Warning,
            NotificationType::Error => LedgerOutcome::Failure,
        };
        let node = self.ledger.add_node(status.into().as_str(), LedgerMode::Automatic);
        node.set_outcome(ledger_outcome);
        Ok(())
    }
}

pub(crate) async fn run_single_target_diagnostics<W: Write>(
    env_context: &EnvironmentContext,
    target_info: TargetInfo,
    ledger: &mut LedgerNodeGuard<'_, W>,
    product_timeout: std::time::Duration,
) -> anyhow::Result<()> {
    let handle: TargetHandle = TargetHandle::try_from(target_info)?;
    let mut notifier = LedgerNotifier::new(ledger);
    run_diagnostics_with_handle(env_context, handle, &mut notifier, product_timeout)
        .await
        .map_err(Into::into)
}
