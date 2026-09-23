// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![cfg(not(feature = "fdomain"))]

use crate::{
    Change, CommitError, ControllerCreationError, ControllerId, PushChangesError,
    RegisterEbpfProgramError, handle_change_validation_result, handle_commit_result,
};
use fidl::marker::SourceBreaking;
use flex_fuchsia_ebpf as febpf;
use flex_fuchsia_net_filter as fnet_filter;

/// A controller for filtering state with blocking methods.
pub struct Controller {
    controller: fnet_filter::NamespaceControllerSynchronousProxy,
    // The client provides an ID when creating a new controller, but the server
    // may need to assign a different ID to avoid conflicts; either way, the
    // server informs the client of the final `ControllerId` on creation.
    id: ControllerId,
    // Changes that have been pushed to the server but not yet committed. This
    // allows the `Controller` to report more informative errors by correlating
    // error codes with particular changes.
    pending_changes: Vec<Change>,
}

impl Controller {
    /// Creates a new `Controller`.
    ///
    /// Note that the provided `ControllerId` may need to be modified server-
    /// side to avoid collisions; to obtain the final ID assigned to the
    /// `Controller`, use the `id` method.
    pub fn new(
        control: &fnet_filter::ControlSynchronousProxy,
        ControllerId(id): &ControllerId,
        deadline: zx::MonotonicInstant,
    ) -> Result<Self, ControllerCreationError> {
        let (controller, server_end) = fidl::endpoints::create_sync_proxy();
        control.open_controller(id, server_end).map_err(ControllerCreationError::OpenController)?;

        let fnet_filter::NamespaceControllerEvent::OnIdAssigned { id } =
            controller.wait_for_event(deadline).map_err(ControllerCreationError::IdAssignment)?;
        Ok(Self { controller, id: ControllerId(id), pending_changes: Vec::new() })
    }

    pub fn id(&self) -> &ControllerId {
        &self.id
    }

    pub fn register_ebpf_program(
        &mut self,
        handle: febpf::ProgramHandle,
        program: febpf::VerifiedProgram,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), RegisterEbpfProgramError> {
        self.controller
            .register_ebpf_program(handle, program, deadline)
            .map_err(RegisterEbpfProgramError::CallMethod)?
            .map_err(RegisterEbpfProgramError::from)
    }

    pub fn push_changes(
        &mut self,
        changes: Vec<Change>,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), PushChangesError> {
        let fidl_changes = changes.iter().cloned().map(Into::into).collect::<Vec<_>>();
        let result = self
            .controller
            .push_changes(&fidl_changes, deadline)
            .map_err(PushChangesError::CallMethod)?;
        handle_change_validation_result(result, &changes)?;
        // Maintain a client-side copy of the pending changes we've pushed to
        // the server in order to provide better error messages if a commit
        // fails.
        self.pending_changes.extend(changes);
        Ok(())
    }

    pub fn commit_with_options(
        &mut self,
        options: fnet_filter::CommitOptions,
        deadline: zx::MonotonicInstant,
    ) -> Result<(), CommitError> {
        let committed_changes = std::mem::take(&mut self.pending_changes);
        let result = self.controller.commit(options, deadline).map_err(CommitError::CallMethod)?;
        handle_commit_result(result, committed_changes)
    }

    pub fn commit(&mut self, deadline: zx::MonotonicInstant) -> Result<(), CommitError> {
        self.commit_with_options(fnet_filter::CommitOptions::default(), deadline)
    }

    pub fn commit_idempotent(&mut self, deadline: zx::MonotonicInstant) -> Result<(), CommitError> {
        self.commit_with_options(
            fnet_filter::CommitOptions {
                idempotent: Some(true),
                __source_breaking: SourceBreaking,
            },
            deadline,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{
        handle_commit, handle_open_controller, handle_push_changes, pretend_invalid_resource,
        test_resource, test_resource_id, unknown_resource_id,
    };
    use crate::{ChangeCommitError, ChangeValidationError};
    use assert_matches::assert_matches;

    #[fuchsia::test(threads = 2)]
    async fn controller_push_changes_reports_invalid_change() {
        let (control_sync, request_stream) =
            fidl::endpoints::create_sync_proxy_and_stream::<fnet_filter::ControlMarker>();

        let run_controller = fuchsia_async::Task::spawn(async {
            let mut stream = handle_open_controller(request_stream).await;
            handle_push_changes(
                &mut stream,
                fnet_filter::ChangeValidationResult::ErrorOnChange(vec![
                    fnet_filter::ChangeValidationError::Ok,
                    fnet_filter::ChangeValidationError::InvalidPortMatcher,
                    fnet_filter::ChangeValidationError::NotReached,
                ]),
            )
            .await;
        });

        let mut controller = Controller::new(
            &control_sync,
            &ControllerId(String::from("test")),
            zx::MonotonicInstant::INFINITE,
        )
        .expect("create controller");
        let result = controller.push_changes(
            vec![
                Change::Create(test_resource()),
                // We fake the server response to say this is invalid even
                // though it really isn't.
                Change::Create(pretend_invalid_resource()),
                Change::Remove(test_resource_id()),
            ],
            zx::MonotonicInstant::INFINITE,
        );

        assert_matches!(
            result,
            Err(PushChangesError::ErrorOnChange(errors)) if errors == vec![(
                Change::Create(pretend_invalid_resource()),
                ChangeValidationError::InvalidPortMatcher
            )]
        );

        run_controller.await;
    }

    #[fuchsia::test(threads = 2)]
    async fn controller_commit_reports_invalid_change() {
        let (control_sync, request_stream) =
            fidl::endpoints::create_sync_proxy_and_stream::<fnet_filter::ControlMarker>();

        let run_controller = fuchsia_async::Task::spawn(async {
            let mut stream = handle_open_controller(request_stream).await;
            handle_push_changes(
                &mut stream,
                fnet_filter::ChangeValidationResult::Ok(fnet_filter::Empty {}),
            )
            .await;
            handle_commit(
                &mut stream,
                fnet_filter::CommitResult::ErrorOnChange(vec![
                    fnet_filter::CommitError::Ok,
                    fnet_filter::CommitError::NamespaceNotFound,
                    fnet_filter::CommitError::Ok,
                ]),
            )
            .await;
        });

        let mut controller = Controller::new(
            &control_sync,
            &ControllerId(String::from("test")),
            zx::MonotonicInstant::INFINITE,
        )
        .expect("create controller");
        controller
            .push_changes(
                vec![
                    Change::Create(test_resource()),
                    Change::Remove(unknown_resource_id()),
                    Change::Remove(test_resource_id()),
                ],
                zx::MonotonicInstant::INFINITE,
            )
            .expect("push changes");

        let result = controller.commit(zx::MonotonicInstant::INFINITE);
        assert_matches!(
            result,
            Err(CommitError::ErrorOnChange(errors)) if errors == vec![(
                Change::Remove(unknown_resource_id()),
                ChangeCommitError::NamespaceNotFound,
            )]
        );

        run_controller.await;
    }
}
