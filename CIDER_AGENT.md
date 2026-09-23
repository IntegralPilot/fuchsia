# Cider Agent Guidelines for Fuchsia (Cog Workspace)

This document contains essential instructions and troubleshooting steps for AI
agents operating within the Fuchsia Cog workspace. For comprehensive details on
Fuchsia development, please always refer to @GEMINI.md

## 1. Running `fx` Commands
* The safest and most reliable way to run `fx` is to invoke it directly using
  its path: `.jiri_root/bin/fx` (or `//.jiri_root/bin/fx` from the repo root).
* Using `scripts/fx` or just `fx` might fail depending on the PATH
  configuration. You may temporarily add `.jiri_root/bin` to your `$PATH`
  during your session if needed.

## 2. Workspace Initialization (Cog)
* If `.jiri_root` is not present or you are encountering fundamental workspace
  configuration issues, the Cog workspace might not be properly initialized.
* The workspace setup script is located at
  `scripts/cog/setup_cog_workspace.py`.
* **Important:** This setup script only needs to be run **once** per workspace.
  You do not need to run it repeatedly.

## 3. Configuring the Build
* Before building or running tests, ensure your build configuration is set up
  correctly.
* See `GEMINI.md` for more details on `fx set`, `fx build`, and building
  specific targets.

## 4. Additional Reference
* For broader guidelines regarding C++/Rust development, writing tests, finding
  FIDL methods, formatting commit messages, and managing Git in a multi-repo
  environment (`jiri`), please consult the `GEMINI.md` file located at the root
  of the Fuchsia directory.
