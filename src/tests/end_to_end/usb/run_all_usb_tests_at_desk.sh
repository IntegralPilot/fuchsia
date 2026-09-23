#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
#
# Runs the USB/ADB end-to-end test suites against a device on your desk.
#
# See //src/tests/end_to_end/usb/ for the tests themselves.

set -euo pipefail

# Resolve the Fuchsia checkout from this script's own location rather than
# from the current working directory. `git rev-parse --show-toplevel` is
# wrong here: Fuchsia is a jiri multi-repo checkout, so running from inside
# a sub-project such as //vendor/google would resolve to that sub-project.
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
FUCHSIA_DIR="$(cd -- "${SCRIPT_DIR}/../../../.." && pwd)"
FX="${FUCHSIA_DIR}/scripts/fx"

if [[ ! -x "${FX}" ]]; then
  echo "ERROR: could not locate the Fuchsia checkout root." >&2
  echo "  Derived FUCHSIA_DIR=${FUCHSIA_DIR}" >&2
  echo "  but ${FX} is not executable." >&2
  echo "  This script expects to live at //src/tests/end_to_end/usb/." >&2
  exit 1
fi

# Always invoke fx by path. `fx` is only on PATH after sourcing
# scripts/fx-env.sh, which not everyone has in their shell rc.
cd "${FUCHSIA_DIR}"

# ---------------------------------------------------------------------------
# Test inventory.
#
# These are the device-facing end-to-end targets (python_mobly_test and
# python_perf_test) under //src/tests/end_to_end/usb. Keep them in sync with
# the BUILD.gn files there.
#
# Deliberately excluded: lib/testusb:testusb_test and
# lib/zero_function:zero_function_lib_test. Those are python_host_test
# library unit tests that need no device; run them with `fx test --host`.
# ---------------------------------------------------------------------------

# //src/tests/end_to_end/usb/functional/...
FUNCTIONAL_TESTS=(
  usb_enumeration_test
  usb_fastboot_e2e_test
  zero_function_test
)

# //src/tests/end_to_end/usb/stress/...
# Excludes usb_disconnect_test, which needs a hardware USB power hub; that
# one is opt-in via --physical-disconnect.
STRESS_TESTS=(
  usb_virtual_disconnect_test
  cdc_stress_test
  usb_fastboot_loop_test
  adb_root_unroot_stress_test
  adb_fastboot_stress_test
  adb_random_stress_test
  zero_function_stress_test
)

# Requires a hardware USB power hub (see --help).
PHYSICAL_TESTS=(
  usb_disconnect_test
)

# //src/tests/end_to_end/usb/performance/...
PERFORMANCE_TESTS=(
  adb_throughput_test
  adb_throughput_large_test
)

function show_help() {
  cat <<'EOF'
Usage: run_all_usb_tests_at_desk.sh [options] [-- <extra fx test args>]

Test selection:
  -f, --functional           Functional tests
  -s, --stress               Stress tests (excluding physical disconnect)
      --physical-disconnect  Physical USB disconnect test (needs power hub)
  -p, --performance          Performance/throughput tests
  -a, --all                  Everything above, including physical disconnect
      --test NAME            Run a specific test by name (repeatable)

Single-test shorthands:
      --virtual-disconnect   Same as --test usb_virtual_disconnect_test
      --adb-perf             Same as --test adb_throughput_test

Other options:
  -t, --device NAME          Target device (runs `fx -t NAME test`)
  -l, --list                 List the known tests
  -n, --dry-run              Resolve and check the tests, but do not run them
  -h, --help                 Show this message

Anything after `--` is forwarded verbatim to the real `fx test` invocation,
for example:
  run_all_usb_tests_at_desk.sh -f -- --test-filter SomeCase

Default when no selection flag is given:
  Functional + stress tests.

  Performance tests are excluded by default because they publish benchmark
  metrics and take substantially longer. The physical disconnect test is
  excluded by default because it needs extra hardware. Pass -p, --all, or
  --physical-disconnect to opt in.

Note on --device:
  Specifies the target device name to pass to `fx -t NAME test`.

Note on --physical-disconnect:
  Requires a hardware USB power hub. The default setup uses 'dmc' (a
  Fuchsia infra tool) which abstracts the power hub hardware, typically a
  Cambrionix in infra labs. For local runs with different hardware you must
  configure 'usb_power_hub_hw' and 'usb_power_hub_impl' in Mobly.
EOF
}

function list_tests() {
  cat <<'EOF'
Functional (-f):
  usb_enumeration_test
  usb_fastboot_e2e_test
  zero_function_test
Stress (-s):
  usb_virtual_disconnect_test
  cdc_stress_test
  usb_fastboot_loop_test
  adb_root_unroot_stress_test
  adb_fastboot_stress_test
  adb_random_stress_test
  zero_function_stress_test
Physical disconnect (--physical-disconnect, needs USB power hub):
  usb_disconnect_test
Performance (-p):
  adb_throughput_test
  adb_throughput_large_test
EOF
}

# Rejects a missing value, and a value that looks like the next flag, so
# that `-t -n` reports an error instead of sending "-n" to fx as a device.
function require_value() {
  local flag="$1"
  local count="$2"
  local value="${3-}"
  if [[ "${count}" -lt 2 || "${value}" == -* ]]; then
    echo "ERROR: ${flag} requires a value" >&2
    exit 1
  fi
}

RUN_FUNCTIONAL=false
RUN_STRESS=false
RUN_PHYSICAL=false
RUN_PERFORMANCE=false
EXPLICIT_SELECTION=false
DEVICE=""
DRY_RUN=false
EXPLICIT_TESTS=()
EXTRA_FX_ARGS=()

while [[ $# -gt 0 ]]; do
  case $1 in
    -a|--all)
      RUN_FUNCTIONAL=true
      RUN_STRESS=true
      RUN_PHYSICAL=true
      RUN_PERFORMANCE=true
      EXPLICIT_SELECTION=true
      shift
      ;;
    -f|--functional)
      RUN_FUNCTIONAL=true
      EXPLICIT_SELECTION=true
      shift
      ;;
    -s|--stress)
      RUN_STRESS=true
      EXPLICIT_SELECTION=true
      shift
      ;;
    --virtual-disconnect)
      EXPLICIT_TESTS+=("usb_virtual_disconnect_test")
      EXPLICIT_SELECTION=true
      shift
      ;;
    --physical-disconnect)
      RUN_PHYSICAL=true
      EXPLICIT_SELECTION=true
      shift
      ;;
    -p|--performance)
      RUN_PERFORMANCE=true
      EXPLICIT_SELECTION=true
      shift
      ;;
    --adb-perf)
      EXPLICIT_TESTS+=("adb_throughput_test")
      EXPLICIT_SELECTION=true
      shift
      ;;
    --cdc-perf)
      # Guard rather than "unknown option": earlier unlanded copies of this
      # script offered --cdc-perf, so point those users somewhere useful.
      echo "ERROR: --cdc-perf is not supported." >&2
      echo "  It referred to 'cdc_ethernet_throughput_test', which does" >&2
      echo "  not exist in the tree. CDC coverage lives in cdc_stress_test" >&2
      echo "  (-s); the throughput benchmarks are adb_throughput_test and" >&2
      echo "  adb_throughput_large_test (-p)." >&2
      exit 1
      ;;
    --test)
      require_value "$1" "$#" "${2-}"
      EXPLICIT_TESTS+=("$2")
      EXPLICIT_SELECTION=true
      shift 2
      ;;
    -t|--device)
      require_value "$1" "$#" "${2-}"
      DEVICE="$2"
      shift 2
      ;;
    -l|--list)
      list_tests
      exit 0
      ;;
    -n|--dry-run)
      DRY_RUN=true
      shift
      ;;
    -h|--help)
      show_help
      exit 0
      ;;
    --)
      shift
      EXTRA_FX_ARGS=("$@")
      break
      ;;
    *)
      echo "Unknown option: $1" >&2
      show_help >&2
      exit 1
      ;;
  esac
done

if [[ "${EXPLICIT_SELECTION}" == false ]]; then
  RUN_FUNCTIONAL=true
  RUN_STRESS=true
fi

TESTS_TO_RUN=()
if [[ "${RUN_FUNCTIONAL}" == true ]]; then
  TESTS_TO_RUN+=("${FUNCTIONAL_TESTS[@]}")
fi
if [[ "${RUN_STRESS}" == true ]]; then
  TESTS_TO_RUN+=("${STRESS_TESTS[@]}")
fi
if [[ "${RUN_PHYSICAL}" == true ]]; then
  echo "WARNING: the physical disconnect test needs a USB power hub." >&2
  echo "         Uses 'dmc' by default (infra), or Mobly config locally." >&2
  TESTS_TO_RUN+=("${PHYSICAL_TESTS[@]}")
fi
if [[ "${RUN_PERFORMANCE}" == true ]]; then
  TESTS_TO_RUN+=("${PERFORMANCE_TESTS[@]}")
fi
if [[ ${#EXPLICIT_TESTS[@]} -gt 0 ]]; then
  TESTS_TO_RUN+=("${EXPLICIT_TESTS[@]}")
fi

if [[ ${#TESTS_TO_RUN[@]} -eq 0 ]]; then
  echo "No tests selected to run."
  exit 0
fi

# De-duplicate while preserving order, so that e.g.
# `-s --virtual-disconnect` does not run the same test twice.
DEDUPED=()
for candidate in "${TESTS_TO_RUN[@]}"; do
  duplicate=false
  for seen in ${DEDUPED[@]+"${DEDUPED[@]}"}; do
    if [[ "${seen}" == "${candidate}" ]]; then
      duplicate=true
      break
    fi
  done
  if [[ "${duplicate}" == false ]]; then
    DEDUPED+=("${candidate}")
  fi
done
TESTS_TO_RUN=("${DEDUPED[@]}")

FX_GLOBAL_ARGS=()
if [[ -n "${DEVICE}" ]]; then
  FX_GLOBAL_ARGS+=(-t "${DEVICE}")
fi

# ---------------------------------------------------------------------------
# Build-configuration gate.
#
# `fx test --dry` resolves the selections against tests.json without running
# anything. If a selection is missing from the build, fx emits a block per
# unmatched name:
#
#     <name> (100.00% similar)
#         fx add-test //path/to:target
#
# which we turn into actionable advice. Any other failure is reported
# verbatim: we must never fall through to the real run after a failed gate.
# ---------------------------------------------------------------------------
echo "Checking build configuration..."
if dry_run_out=$("${FX}" ${FX_GLOBAL_ARGS[@]+"${FX_GLOBAL_ARGS[@]}"} \
    test --dry --e2e "${TESTS_TO_RUN[@]}" 2>&1); then
  gate_ok=true
else
  gate_ok=false
fi

# Strip ANSI styling before matching. fx disables color when stdout is not
# a TTY, which is the case here, but do not depend on that. Use $'\033'
# rather than \x1b, which is a GNU sed extension.
clean_out=$(printf '%s\n' "${dry_run_out}" \
  | sed -e $'s/\033\\[[0-9;]*m//g')

# `fx test --dry` returns exit status 0 as long as at least one test argument
# matches, even when other arguments in the list do not match. Check the
# output explicitly so partial matches still trigger the missing-test gate.
if printf '%s\n' "${clean_out}" \
    | grep -qF "Could not find any tests to run"; then
  gate_ok=false
fi

if [[ "${gate_ok}" == false ]]; then

  # Collect "<name>\t<suggestion>" for every 100.00% block. Matching the
  # whole line and taking field 1 avoids a substring of one test name
  # matching a different, longer suggested name.
  parsed=$(printf '%s\n' "${clean_out}" | awk '
    /^[[:space:]]*[^[:space:]]+ \(100\.00% similar\)[[:space:]]*$/ {
      name = $1
      suggestion = ""
      if ((getline line) > 0) {
        sub(/^[[:space:]]+/, "", line)
        sub(/[[:space:]]+$/, "", line)
        suggestion = line
      }
      print name "\t" suggestion
    }')

  MISSING_TESTS=()
  SUGGESTED_COMMANDS=()
  for candidate in "${TESTS_TO_RUN[@]}"; do
    # -x for a whole-line match, -- so a name starting with "-" is not
    # parsed as an option.
    if ! printf '%s\n' "${parsed}" | cut -f1 \
        | grep -qxF -- "${candidate}"; then
      continue
    fi
    MISSING_TESTS+=("${candidate}")
    suggestion=$(printf '%s\n' "${parsed}" \
      | awk -F'\t' -v n="${candidate}" '$1 == n { print $2; exit }')
    # The line after a match is not always a command: for a target already
    # in the build it reads "Build includes: fuchsia-pkg://...". Only
    # forward it if it really is something the user can run.
    if [[ "${suggestion}" == fx\ * ]]; then
      SUGGESTED_COMMANDS+=("${suggestion}")
    fi
  done

  if [[ ${#MISSING_TESTS[@]} -gt 0 ]]; then
    echo "ERROR: these tests are not in your build configuration:" >&2
    printf '  - %s\n' "${MISSING_TESTS[@]}" >&2
    echo "" >&2
    if [[ ${#SUGGESTED_COMMANDS[@]} -gt 0 ]]; then
      echo "To add them, run:" >&2
      printf '%s\n' "${SUGGESTED_COMMANDS[@]}" | sort -u \
        | sed -e 's/^/  /' >&2
      echo "" >&2
    fi
    echo "Or add all E2E USB tests at once:" >&2
    echo "  fx add-host-test //src/tests/end_to_end/usb:tests" >&2
  else
    echo "ERROR: 'fx test --dry' failed, but not because of a missing" >&2
    echo "test. Output was:" >&2
    printf '%s\n' "${dry_run_out}" >&2
  fi
  exit 1
fi

if [[ "${DRY_RUN}" == true ]]; then
  echo "Dry run only; the following tests resolved successfully:"
  printf '  %s\n' "${TESTS_TO_RUN[@]}"
  exit 0
fi

echo "Running tests: ${TESTS_TO_RUN[*]}"
# -o streams test output as it happens. exec so that the exit status and
# any signals belong to fx directly.
exec "${FX}" ${FX_GLOBAL_ARGS[@]+"${FX_GLOBAL_ARGS[@]}"} \
  test -o --e2e "${TESTS_TO_RUN[@]}" \
  ${EXTRA_FX_ARGS[@]+"${EXTRA_FX_ARGS[@]}"}
