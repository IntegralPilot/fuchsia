# Cog Workspace Setup

This directory contains scripts for setting up and managing a
[cog](go/cog)-based workspace for Fuchsia development. Cog (Code on Git) is a
system that allows developers to work with Git-on-Borg repositories using a
virtual filesystem.

The primary script in this directory is `setup-cog-workspace`.

## `setup-cog-workspace`

This script initializes a new cog workspace.

### Usage

To use this script, you must run the script from within your cog workspace
directory. The script will automatically detect the workspace root based on the
current directory, which must match the pattern
`/google/cog/cloud/<user>/<workspace_name>`.

Example:

```bash
cogd my-workspace
scripts/cog/setup_cog_workspace.py
```
In the near future, we expect for this script to be automatically run.

### Testing

To run the tests for this script, run the following command:

```bash
fx add-host-test //scripts/cog:tests
```

You can then run the tests with:

```bash
fx test //scripts/cog
```

### Why is this not in `fx`?

This script must be run before we can use `fx` because it sets up `jiri_root` which
is used by `fx` to find the Fuchsia source tree. Eventually, this script might be
integrated into `fx` but at this time it must be run separately.

### Workspace Linking and Metadata

To ensure that a Cog workspace is correctly linked to its persistent storage in
Cartfs, we use a unique `workspace_id`. This ID is sourced from
`.citc/workspace_id` within the Cog workspace.

When a workspace is linked to a Cartfs directory, a `.cog.json` metadata file is
created in that directory. This file contains:
- `workspace_name`: The name of the Cog workspace.
- `repo_name`: The name of the repository (e.g., `fuchsia`).
- `workspace_id`: The unique ID for the workspace instance.

The `workspace_id` is used to validate the link. If you attempt to use a
previously existing Cartfs directory with the same workspace name but a
different `workspace_id`, the script will consider the link invalid and suggest
creating a new directory.

### Cartfs Directory Structure

When this script runs, it will create (or link to) a directory in the Cartfs
mount point. The suggested directory name is `<workspace_name>-<workspace_id>`.

Inside the Cartfs directory:
- `.cog.json`: Metadata for linking validation.
- `fuchsia`: The fuchsia source directory.
- `fuchsia/prebuilt/`: A subdirectory for persistent prebuilts.
- `integration`: The integration repository.
- `.integration_commit_hash`: The last known integration hash.
- `out/`: A subdirectory for build artifacts.

The Cog workspace then points to these directories via symlinks (e.g.,
`cartfs-dir` symlink in the workspace root).
