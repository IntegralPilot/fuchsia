# Copyright 2017 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# shellcheck disable=SC2148
# No shebang in this file is intentional

# Return the top-directory of a given Bazel workspace used by the platform
# build. A TOPDIR contains several files and directories like workspace/
# or output_base/
fx-bazel-top-dir () {
  # LINT.IfChange(bazel_topdir_config_file)
  local INPUT_FILE="${FUCHSIA_DIR}/build/bazel/config/bazel_top_dir"
  # LINT.ThenChange(//build/bazel/bazel_workspace.gni:bazel_topdir_config_file)
  local TOPDIR
  TOPDIR=$(<"${INPUT_FILE}")
  echo "${FUCHSIA_BUILD_DIR}/${TOPDIR}"
}

# Return path to Bazel workspace.
fx-get-bazel-workspace () {
  printf %s/workspace "$(fx-bazel-top-dir)"
}

# Return the path to the Bazel wrapper script.
fx-get-bazel () {
   printf %s/bazel "$(fx-bazel-top-dir)"
}

# Regenerate Bazel workspace and launcher script if needed.
# Note that this also regenerates the Ninja build plan if necessary.
fx-update-bazel-workspace () {
  # Ignore this when running unit tests.
  if [[ -n "${DISABLE_FX_UPDATE_BAZEL_WORKSPACE_FOR_TESTS}" ]]; then
    return 0
  fi
  # First, refresh Ninja build plan if needed.
  local check_script="${FUCHSIA_DIR}/build/bazel/scripts/check_regenerator_inputs.py"
  if ! "${PREBUILT_PYTHON3}" -S "${check_script}" --quiet "${FUCHSIA_BUILD_DIR}"; then
    echo "fx-bazel: Regenerating workspace due to input file changes!"
    fx-command-run gen
  fi
}

# Run bazel command in the Fuchsia workspace, after ensuring it is up-to-date.
fx-bazel () {
  fx-update-bazel-workspace
  # Building with Bazel now requires a `--config=`
  # option to avoid a cryptic error related to C++ toolchain resolution failure.
  # Parse the command arguments to print a warning if it is missing.
  local args=("$@")
  local bazel_command="${args[0]}"
  case "${bazel_command}" in
    # The following commands are configuration-sensitive, and will require
    # A platform --config option to avoid toolchain resolution errors.
    aquery|build|cquery|info|run|test)
      local opt
      local has_platform_config
      for opt in "${args[@]:1}"; do
          case "$opt" in
            --config=host|--config=linux*|--config=fuchsia*)
                has_platform_config=true
                if [[ "$opt" == "--config=fuchsia" ]]; then
                  fx-warn "'--config=fuchsia' is deprecated, use '--config=fuchsia_platform' or '--config=fuchsia_sdk' as appropriate."
                fi
                ;;
          esac
      done
      if [[ -z "$has_platform_config" ]]; then
          fx-error "Use '--config=fuchsia_platform', '--config=fuchsia_sdk', or '--config=host' when invoking Bazel ${bazel_command} commands!"
          return 1
      fi
      ;;
  esac

  fx-run-bazel "" "$(fx-get-bazel)" "$@"
}
