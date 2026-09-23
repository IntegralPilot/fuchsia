// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_debug::lifecycle::{
    ActionError, CreateError, DestroyError, ResolveError, StartError,
};
use errors::ffx_error;
use moniker::Moniker;

static LIFECYCLE_ERROR_HELP: &'static str =
    "To learn more, see https://fuchsia.dev/go/components/run-errors";
static FIND_LIST_OR_SHOW: &'static str =
    "Use `ffx component list` or `ffx component show` to find the correct instance.";
static FFX_INCOMPATIBILITY: &'static str =
    "This is most likely due to an incompatibility between ffx and the target.";
static CHECK_TARGET_LOGS: &'static str =
    "Check target logs (`ffx log`) for error details printed by component_manager.";

/// Format an ActionError into an error message that is suitable for `ffx component`.
pub fn format_action_error(moniker: &Moniker, err: ActionError) -> errors::FfxError {
    match err {
        ActionError::InstanceNotFound => ffx_error!(
            "\nError: The instance {} does not exist.\n{}\n{}\n",
            moniker,
            FIND_LIST_OR_SHOW,
            LIFECYCLE_ERROR_HELP
        ),
        ActionError::InstanceNotResolved => ffx_error!(
            "\nError: The instance {moniker} has not been resolved.\n{}\n{}\n",
            CHECK_TARGET_LOGS,
            LIFECYCLE_ERROR_HELP
        ),
        ActionError::BadMoniker => ffx_error!(
            "\nError: Component manager cannot parse the moniker `{}`. {}\n",
            moniker,
            FFX_INCOMPATIBILITY
        ),
        ActionError::Internal => ffx_error!(
            "\nError: Component manager encountered an internal error.\n{}\n{}\n",
            CHECK_TARGET_LOGS,
            LIFECYCLE_ERROR_HELP
        ),
        ActionError::UnknownError => ffx_error!(
            "\nError: Component manager returned an unknown error.\n{}\n{}\n{}\n",
            FFX_INCOMPATIBILITY,
            CHECK_TARGET_LOGS,
            LIFECYCLE_ERROR_HELP
        ),
        ActionError::Fidl(e) => ffx_error!(
            "\nError: FIDL error communicating with LifecycleController ({:?}).\n{}\n{}\n",
            e,
            CHECK_TARGET_LOGS,
            LIFECYCLE_ERROR_HELP
        ),
    }
}

/// Format a CreateError into an error message that is suitable for `ffx component`.
pub fn format_create_error(
    moniker: &Moniker,
    parent: &Moniker,
    collection: &str,
    err: CreateError,
) -> errors::FfxError {
    match err {
        CreateError::InstanceAlreadyExists => ffx_error!(
            "\nError: {} already exists.\nUse `ffx component show` to get information about the instance.\n{}\n",
            moniker,
            LIFECYCLE_ERROR_HELP
        ),
        CreateError::CollectionNotFound => ffx_error!(
            "\nError: The parent {} does not have a collection `{}`.\nCheck the manifest of {} for a collection with this name.\n",
            parent,
            collection,
            parent
        ),
        CreateError::BadChildDecl => ffx_error!(
            "\nError: Component manager cannot parse the child decl created by ffx. {}\n",
            FFX_INCOMPATIBILITY
        ),
        CreateError::ActionError(e) => format_action_error(parent, e),
    }
}

/// Format a DestroyError into an error message that is suitable for `ffx component`.
pub fn format_destroy_error(moniker: &Moniker, err: DestroyError) -> errors::FfxError {
    match err {
        DestroyError::BadChildRef => ffx_error!(
            "\nError: Component manager cannot parse the child reference created by ffx. {}\n",
            FFX_INCOMPATIBILITY
        ),
        DestroyError::ActionError(e) => format_action_error(moniker, e),
    }
}

/// Format a ResolveError into an error message that is suitable for `ffx component`.
pub fn format_resolve_error(moniker: &Moniker, err: ResolveError) -> errors::FfxError {
    match err {
        ResolveError::PackageNotFound => ffx_error!(
            "\nError: The package associated with the instance {} could not be found.\nEnsure that your package server is running and the package is added to it.\n",
            moniker
        ),
        ResolveError::ManifestNotFound => ffx_error!(
            "\nError: The manifest associated with the instance {} could not be found.\nEnsure that your package contains the manifest.\n",
            moniker
        ),
        ResolveError::ActionError(e) => format_action_error(moniker, e),
    }
}

/// Format a StartError into an error message that is suitable for `ffx component`.
pub fn format_start_error(moniker: &Moniker, err: StartError) -> errors::FfxError {
    match err {
        StartError::PackageNotFound => ffx_error!(
            "\nError: The package associated with the instance {} could not be found.\nEnsure that your package server is running and the package is added to it.\n",
            moniker
        ),
        StartError::ManifestNotFound => ffx_error!(
            "\nError: The manifest associated with the instance {} could not be found.\nEnsure that your package contains the manifest.\n",
            moniker
        ),
        StartError::ActionError(e) => format_action_error(moniker, e),
    }
}
