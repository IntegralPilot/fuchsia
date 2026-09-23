#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# fuchsia-rsproxy-wrap.sh invokes rsclient's rsproxy-wrap.sh, but
# with fuchsia-specific configurations and features.
# This is only intended for use when the directly wrapped command is
# ninja or ninja-like.

set -euo pipefail

readonly SCRIPT_NAME="$(basename "${BASH_SOURCE[0]}")"
readonly SCRIPT_DIR="$(dirname "${BASH_SOURCE[0]}")"

# Get the HOST_PLATFORM for the prebuilt path.
# Sourcing platform.sh requires FUCHSIA_DIR to be set.
readonly FUCHSIA_DIR="$(readlink -f "$SCRIPT_DIR/../..")"
source "${FUCHSIA_DIR}/tools/devshell/lib/platform.sh"

readonly check_loas_script="${FUCHSIA_DIR}/build/auth/check_loas_restrictions.sh"

# rsclient install path is set in manifests/prebuilts
readonly PREBUILT_RSCLIENT_DIR="${FUCHSIA_DIR}/prebuilt/rsclient/$HOST_PLATFORM"
readonly proxy_wrap="$PREBUILT_RSCLIENT_DIR/bin/rsproxy-wrap.sh"
readonly rsproxy="$PREBUILT_RSCLIENT_DIR/bin/rsproxy"

# Use re-client's credentials helper tool to exchange LOAS for OAuth2 tokens.
readonly credshelper="${PREBUILT_RECLIENT_DIR}/credshelper"

# default options:
if command -v gcert >/dev/null 2>&1; then
  # Detect LOAS type if it is not already passed in.
  loas_type=auto
else
  # Assume this in an infra environment, and do not attempt to
  # use any credential helpers.
  loas_type=skip
fi
use_gce_machine_credentials=false
verbose=0

function die() {
  echo "[$SCRIPT_NAME]: Error: $*" >&2
  exit 1
}

function debug_msg() {
  [[ "$verbose" == 0 ]] || {
    echo "[$SCRIPT_NAME]: $*"
  }
}

function usage() {
  cat <<EOF
usage: $0 [options] -- command ...

options:
  -h | --help: print help and exit
  --loas-type TYPE: {skip,auto,restricted,unrestricted}, default [$loas_type]
    'skip' will bypass any preflight authentication checks
    'auto' will attempt to detect as restricted or unrestricted.
  --use-machine-credentials: use GCE machine-credentials (bypasses LOAS/OAuth checks, takes absolute precedence)
  --log-dir DIR: rsproxy log dir
  -v | --verbose: print debug messages

  Unrecognized options before -- will be forwarded to rsproxy.

environment variables:
  FX_INTERNAL_RESULTSTORE_NINJA:
      Set to 1 to enable ResultStore, 0 to disable.
      This is usually set by build/scripts/main_build.py.
      Default is disabled if unset.

  RS_rs_instance, RS_cas_instance:
      Configure the ResultStore and CAS target database instance names.
      Critical for bypassing static configs in sandboxed GCE builds.
      These variables directly drive rsproxy.

  RS_rs_service, RS_cas_service:
      Override the gRPC service endpoints for ResultStore and CAS.
      Supports unix:// socket paths for proxy routing.
      These variables directly drive rsproxy.
EOF
}

# Parse options up to --, and treat the rest as the wrapped command.
override_proxy_options=()
got_ddash=0
log_dir=
prev_opt=
for opt  # "$@"
do
  # handle --option arg
  if [[ -n "$prev_opt" ]]
  then
    eval "$prev_opt"=\$opt
    prev_opt=
    shift
    continue
  fi

  # Extract optarg from --opt=optarg
  optarg=
  case "$opt" in
    -*=*) optarg="${opt#*=}" ;;  # remove-prefix, shortest-match
  esac

  case "$opt" in
    -h | --help) usage; exit ;;
    --loas-type=*) loas_type="$optarg" ;;
    --loas-type) prev_opt=loas_type ;;
    --log-dir=*) log_dir="$optarg" ;;
    --log-dir) prev_opt=log_dir ;;
    --use-machine-credentials) use_gce_machine_credentials=true ;;
    -v | --verbose) verbose=1 ;;

    --) got_ddash=1; shift; break ;;

    # Forward unknown options to rsproxy.
    *) override_proxy_options+=( "$opt" ) ;;
  esac
  shift
done

[[ -z "$prev_opt" ]] || {
  die "Missing --${prev_opt} argument."
}

wrapped_command=("$@")

[[ "$got_ddash" == 1 ]] || {
  die "Missing -- before the wrapped command."
}
[[ "${#wrapped_command[@]}" -ge 1 ]] || {
  die "The wrapped command must not be empty."
}

# LINT.IfChange(resultstore_ninja_env_vars)
# Enable ResultStore if one of the following is true:
#   1. FX_INTERNAL_RESULTSTORE_NINJA is explicitly set to 1.
#   2. RS_rs_service is set (legacy/transition fallback for infra/recipes).
#
# TODO(https://fxbug.dev/537038381): recipe should pass flag to main_build.py
# to control ResultStore.
enable_resultstore=0
if [[ "${FX_INTERNAL_RESULTSTORE_NINJA:-0}" == "1" || -n "${RS_rs_service:-}" ]]; then
  enable_resultstore=1
fi

if [[ "$enable_resultstore" == 0 ]]; then
  debug_msg "ResultStore disabled.  Running original command without rsproxy."
  exec "${wrapped_command[@]}"
fi
# LINT.ThenChange(//build/scripts/main_build.py:resultstore_ninja_env_vars)

rsproxy_options=(
  --project_id=fuchsia
)

# rsproxy configuration:
#
### 'fx build'
# Select config based on LOAS type.
# FX_BUILD_LOAS_TYPE is set by 'fx build' to either "restricted" or
# "unrestricted", and influences authentication method.
#
# If loas_type was set by a command-line option (e.g. 'skip' for TUI),
# it must take precedence. This is essential for the TUI because it uses
# an insecure local connection; if we use a credentialed LOAS type,
# gRPC will refuse to send credentials over the insecure transport,
# causing a deadlock.
if [[ "$use_gce_machine_credentials" == "true" ]]; then
  loas_type="skip"
fi

if [[ "$loas_type" == "auto" ]]; then
  loas_type="${FX_BUILD_LOAS_TYPE:-"auto"}"
fi
[[ "$loas_type" != "auto" ]] || {
  # Detect "restricted" or "unrestricted"
  loas_type="$("$check_loas_script" | tail -n 1)" || {
    die "Unable to infer LOAS certificate type"
  }
}
debug_msg "using LOAS type: $loas_type"
case "$loas_type" in
  unrestricted)
    readonly CFG="$SCRIPT_DIR/fuchsia-resultstore-gcertauth.cfg"
    rsproxy_options+=(
      --cfg "$CFG"
      --credentials_helper "${credshelper}"
    )
    ;;
  restricted)
    readonly CFG="$SCRIPT_DIR/fuchsia-resultstore.cfg"
    rsproxy_options+=(
      --cfg "$CFG"
    )
    ;;
  skip) : ;;

  *)
    die "Unhandled LOAS type: $loas_type"
    ;;
esac

### infra builds
# Infra builds do not use .cfg files from the source tree;
# they set various RS_* environment variables to override
# the corresponding flags, e.g.:
# LINT.IfChange(rs_instance_env_vars)
#   * RS_rs_instance
#   * RS_cas_instance
# LINT.ThenChange(//build/scripts/main_build.py:rs_instance_env_vars)
#   * RS_rs_service
#   * RS_cas_service

# When rs_service points to a unix socket, TLS assumes a server name of
# "localhost", for which certs are invalid.  Fix this by using the
# real name of the service.  Same for cas_service.
# TODO: pass these from recipes as RS_* environment variables.
# LINT.IfChange(rs_service_env_vars)
case "${RS_rs_service:-NOT_SET}" in
  unix://*)
    rsproxy_options+=( --rs_tls_server_name="resultstore.googleapis.com")
    ;;
esac
case "${RS_cas_service:-NOT_SET}" in
  unix://*)
    rsproxy_options+=( --cas_tls_server_name="remotebuildexecution.googleapis.com")
    ;;
esac
# LINT.ThenChange(//build/scripts/main_build.py:rs_service_env_vars)

# Scan wrapped command arguments for a build directory override (-C) to
# organize log directories for nested sub-builds.
#
# NOTE: The fragile command-line scanning for Ninja telemetry outputs
# (--chrome_trace, --action_metrics_output, --dirty_sources_list) has been
# removed. That responsibility has shifted to the caller (e.g. main_build.py),
# which explicitly registers those paths using the --post-build-uploads options.
subbuild_dir=
prev_opt=""
for opt in "${wrapped_command[@]}"
do
  # handle --option arg
  if [[ -n "$prev_opt" ]]
  then
    eval "$prev_opt"=\$opt
    prev_opt=
    continue
  fi

  case "$opt" in
    # ninja options
    -C) prev_opt=subbuild_dir ;;
  esac
done

if [[ -n "$subbuild_dir" ]]; then
  readonly subbuild_base="${subbuild_dir##*/}"  # basename
else
  # For non-ninja commands, subbuild_dir is not expected.
  # Default to something generic for log directory structure.
  readonly subbuild_base="top"
fi

proxy_env=()
if [[ "$use_gce_machine_credentials" == "true" ]]; then
  proxy_env+=(
    RS_use_application_default_credentials=false
    RS_use_gce_credentials=true
    RS_experimental_credentials_helper=""
  )
fi

proxy_wrap_options=(
  --rsproxy "$rsproxy"
)

# Handle log dir.
if [[ -n "$log_dir" ]]
then
  proxy_wrap_options+=( --log-dir "$log_dir" )
  # This will be used as a parent log dir in sub-builds.
  proxy_env+=( RS_log_dir="$log_dir" )
elif [[ "${RS_log_dir:-NOT_SET}" != "NOT_SET" ]]
then
  # Preserve sub-invocation directory structure using the basename of the
  # sub-build dir.
  # Override the environment variable, which take precedence over the flag.
  readonly subbuild_log_dir="$RS_log_dir/$subbuild_base"
  mkdir -p "$subbuild_log_dir"
  proxy_wrap_options+=( --log-dir "$subbuild_log_dir" )
  proxy_env+=( RS_log_dir="$subbuild_log_dir" )
  # This will be visible to the wrapped command as well.
fi
# Otherwise, fallback to using some temp dir.

[[ "${GCE_METADATA_HOST:-NOT_SET}" == "NOT_SET" ]] || {
  # Workaround: avoid DNS lookup of "localhost"
  proxy_env+=( GCE_METADATA_HOST="${GCE_METADATA_HOST/localhost/127.0.0.1}" )
}


# Ensure that the prebuilt python3 is in the PATH (needed in infra environment).
# rsproxy-wrap.sh uses python3 as an alternative means for mkfifo and sleep.
readonly py3_bindir="${PREBUILT_PYTHON3%/*}"  # dirname
export PATH="$py3_bindir:$PATH"

full_cmd=(
  env
  "${proxy_env[@]}"
  "${proxy_wrap}"
  "${proxy_wrap_options[@]}"
  --rsproxy_options
  "${rsproxy_options[@]}"
  "${override_proxy_options[@]}"
  --
  "${wrapped_command[@]}"
)

[[ "$verbose" == 0 ]] || {
  echo "[$SCRIPT_NAME] ---- env start ----"
  # Uncomment the following to enable extreme gRPC traffic logging from rsproxy.
  # Doing so will print every single request trace to the console (which can flood build logs).
  # export SH_WRAPPER_TEST_DEBUG=1
  env
  echo "[$SCRIPT_NAME] ---- env end ----"
}

debug_msg "full command: ${full_cmd[*]}"
exec "${full_cmd[@]}"
