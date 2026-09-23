# Resources for Cog Workspaces

This directory contains resources used to configure a Fuchsia "cog" workspace. In a cog environment, these files are typically symlinked into the root of a Fuchsia checkout (`$FUCHSIA_DIR`) in CartFS.

*   **`args.gn`**: Provides a default set of GN build arguments tailored for cog workspaces. When a user runs `fx set`, these arguments configure the build, ensuring a consistent development environment. This file is intended to be symlinked from `local/args.gn` within the CartFS filesystem.
