// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::lifecycle::{ActionError, CreateError, DestroyError, ResolveError, StartError};
use anyhow::{format_err, Error};
use cm_types::BorrowedName;
use moniker::Moniker;

static LIFECYCLE_ERROR_HELP: &'static str =
    "To learn more, see https://fuchsia.dev/go/components/run-errors";
static FIND_LIST_OR_SHOW: &'static str =
    "Use the `list` or `show` subcommand to find the correct instance.";
static TOOL_INCOMPATIBILITY: &'static str =
    "This is most likely due to an incompatibility between this tool and the target.";
static CHECK_TARGET_LOGS: &'static str =
    "Check target logs for error details printed by component_manager.";

/// Format an ActionError into an error message that is suitable for a CLI tool.
pub fn format_action_error(moniker: &Moniker, err: ActionError) -> Error {
    match err {
        ActionError::InstanceNotFound => format_err!(
            "\nError: The instance {} does not exist.\n{}\n{}\n",
            moniker,
            FIND_LIST_OR_SHOW,
            LIFECYCLE_ERROR_HELP
        ),
        ActionError::InstanceNotResolved => format_err!(
            "\nError: The instance {moniker} has not been resolved.\n{}\n{}\n",
            CHECK_TARGET_LOGS,
            LIFECYCLE_ERROR_HELP
        ),
        ActionError::BadMoniker => format_err!(
            "\nError: Component manager cannot parse the moniker `{}`. {}\n",
            moniker,
            TOOL_INCOMPATIBILITY
        ),
        ActionError::Internal => format_err!(
            "\nError: Component manager encountered an internal error.\n{}\n{}\n",
            CHECK_TARGET_LOGS,
            LIFECYCLE_ERROR_HELP
        ),
        ActionError::UnknownError => format_err!(
            "\nError: Component manager returned an unknown error.\n{}\n{}\n{}\n",
            TOOL_INCOMPATIBILITY,
            CHECK_TARGET_LOGS,
            LIFECYCLE_ERROR_HELP
        ),
        ActionError::Fidl(e) => format_err!(
            "\nError: FIDL error communicating with LifecycleController ({:?}).\n{}\n{}\n",
            e,
            CHECK_TARGET_LOGS,
            LIFECYCLE_ERROR_HELP
        ),
    }
}

/// Format a CreateError into an error message that is suitable for a CLI tool.
pub fn format_create_error(
    moniker: &Moniker,
    parent: &Moniker,
    collection: &BorrowedName,
    err: CreateError,
) -> Error {
    match err {
        CreateError::InstanceAlreadyExists => format_err!("\nError: {} already exists.\nUse the `show` subcommand to get information about the instance.\n{}\n", moniker, LIFECYCLE_ERROR_HELP),
        CreateError::CollectionNotFound => format_err!("\nError: The parent {} does not have a collection `{}`.\nCheck the manifest of {} for a collection with this name.\n", parent, collection, parent),
        CreateError::BadChildDecl => format_err!("\nError: Component manager cannot parse the child decl. {}\n", TOOL_INCOMPATIBILITY),
        CreateError::ActionError(e) => format_action_error(parent, e)
    }
}

/// Format a DestroyError into an error message that is suitable for a CLI tool.
pub fn format_destroy_error(moniker: &Moniker, err: DestroyError) -> Error {
    match err {
        DestroyError::BadChildRef => format_err!(
            "\nError: Component manager cannot parse the child reference. {}\n",
            TOOL_INCOMPATIBILITY
        ),
        DestroyError::ActionError(e) => format_action_error(moniker, e),
    }
}

/// Format a ResolveError into an error message that is suitable for a CLI tool.
pub fn format_resolve_error(moniker: &Moniker, err: ResolveError) -> Error {
    match err {
        ResolveError::PackageNotFound => format_err!("\nError: The package associated with the instance {} could not be found.\nEnsure that your package server is running and the package is added to it.\n", moniker),
        ResolveError::ManifestNotFound => format_err!("\nError: The manifest associated with the instance {} could not be found.\nEnsure that your package contains the manifest.\n", moniker),
        ResolveError::ActionError(e) => format_action_error(moniker, e)
    }
}

/// Format a StartError into an error message that is suitable for a CLI tool.
pub fn format_start_error(moniker: &Moniker, err: StartError) -> Error {
    match err {
        StartError::PackageNotFound => format_err!("\nError: The package associated with the instance {} could not be found.\nEnsure that your package server is running and the package is added to it.\n", moniker),
        StartError::ManifestNotFound => format_err!("\nError: The manifest associated with the instance {} could not be found.\nEnsure that your package contains the manifest.\n", moniker),
        StartError::ActionError(e) => format_action_error(moniker, e)
    }
}
