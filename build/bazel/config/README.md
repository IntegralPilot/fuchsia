This directory contains configuration information that is read from several
scripts ot locations.

- `bazel_top_dir`:

  The `BAZEL_TOPDIR` for the main workspace, relative to the Ninja output
  directory. See `//build/bazel/README.md` for details.

  Format:  A single line of text for the path.
  Used by: `//build/bazel/bazel_workspace.gni`
  Used by: `//tools/devshell/lib/bazel_utils.sh`
  Used by: `//build/bazel/scripts/parse-workspace-event-log.py`
  Used by: `//build/bazel/scripts/workspace_utils.py`
  Used by: `//tools/integration/fint/build.go`

- bazel_args.gni:

  GNI file computing the Bazel command-line arguments to use to match
  the current GN build configuration, with regards to optimization mode,
  sanitizer mode, verbosity and remote builds.

- logging.gni:

  GNI file declaring a GN build variable that impacts Bazel invocations.

- no_downloads_allowed.config:

  A Bazel --experimental_downloader_config file referenced from the
  top-level .bazelrc file to disable all downloads when running repository
  rules.

- BUILD.gn:

  A GN build file that defines a `generated_file()` target dumping
  the current toolchain's bazel build arguments to a JSON file.
