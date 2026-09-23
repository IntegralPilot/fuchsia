#!/bin/bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -Eeuo pipefail

# Ganerates a table comparing Netstack2 and Netstack3 syscall and socket test
# expectations.
#
# Usage:
#
# This script is called without arguments. It requires communication with a
# Fuchsia target device to extract test case names and expectations. It uses
# `fx` commands to achieve that.
#
# The tool outputs a CSV table to stdout where the first column is the test
# name, second column is Netstack2 expectation, and third column is Netstack3
# expectation.

# Logs inputs to stderr.
log() { echo "$@" 1>&2; }

# Gets the full URL for a test component target.
#
# Arguments
#  - Test name as passed to fx test.
get_test_url() {
    log "getting URL for $1"
    fx test --info "$1" | sed -n "s/^package_url: \(\S*\)$/\1/p"
}

# Gets the package URL from a fully qualified fuchsia package URL to a
# COMPONENT.
#
# Arguments
#  - Fully qualified fuchsia package URL for a test COMPONENT.
get_package_url() {
    echo "$1" | sed -n "s/\(\S*\)#meta.*/\1/p"
}

# Reads expectations JSON for a test.
#
# Arguments
#  - Fully qualified fuchsia package URL for the PACKAGE containing the test.
#  - Path to expectations file relative to package root.
read_expectations() {
    local hash
    log "reading expectations at $2 for $1"
    ffx target package explore "$1" --command "cat /pkg/$2"
}

# Lists cases for a test.
#
# Arguments
#  - Fully qualified fuchsia package URL for the test COMPONENT.
list_cases() {
    log "listing cases for $1"
    ffx test list-cases "$1"
}

# Loads test cases and expectations JSON for a test.
#
# Arguments:
#  - Test name as passed to fx test.
#  - File path to save list of cases to.
#  - File path to save expectation JSON to.
#  - Path to expectations file relative to the test's package root.
load_cases_and_expectations() {
    local url
    local package
    local cases
    local expect
    url=$(get_test_url "$1")
    package=$(get_package_url "$url")
    cases="$2"
    expect="$3"
    list_cases "$url" > "$cases"
    read_expectations "$package" "$4" > "$expect"
}


# Lists expectations for two expectation sets on the same set of tests.
#
# Arguments:
#  - Test name as passed to fx test for first expectation set.
#  - Path to expectations file relative to package root for first expectation
#    set.
#  - Test name as passed to fx test for second expectation set.
#  - Path to expectations file relative to package root for second expectation
#    set.
cmp_expectations() {
    local left_cases
    local left_expect
    local right_cases
    local right_expect
    left_cases=$(mktemp)
    left_expect=$(mktemp)
    right_cases=$(mktemp)
    right_expect=$(mktemp)
    trap "rm -f $left_cases $left_expect $right_cases $right_expect" RETURN EXIT
    load_cases_and_expectations "$1" "$left_cases" "$left_expect" "$2"
    load_cases_and_expectations "$3" "$right_cases" "$right_expect" "$4"
    # Diff the cases list to make sure we're comparing equivalent test suites.
    diff "$left_cases" "$right_cases"
    fx list_test_expectations "$left_expect" "$right_expect" < "$left_cases"
}

# Lists expectations from a single component only, adding a column filled
# with `Pass` before it.
#
# Arguments:
#  - Test name as passed to fx test.
#  - Path to expectations file relative to package root.
cmp_expectations_assume_first_passes() {
    local cases
    local expect
    cases=$(mktemp)
    expect=$(mktemp)
    trap "rm -f $cases $expect" RETURN EXIT
    load_cases_and_expectations "$1" "$cases" "$expect" "$2"
    # Add a column with all passes so it matches the layout of cmp_expectations.
    fx list_test_expectations "$expect" < "$cases" | sed "s/,/,Pass,/"
}

all_syscalls=(
    "generic"
    "loopback"
    "loopback_tcp_accept_backlog_listen_v4"
    "loopback_tcp_accept_backlog_listen_v4_mapped"
    "loopback_tcp_accept_backlog_listen_v6"
    "loopback_tcp_accept"
    "loopback_tcp_backlog"
    "loopback_isolated"
    "loopback_isolated_tcp_fin_wait"
    "loopback_isolated_tcp_linger_timeout"
    "raw_packet"
    "udp"
    "udp_unbound"
    "tcp"
    "tcp_blocking"
)

# Socket tests are assumed to be all pass on NS2 and we just load NS3
# expectations.
all_socket=(
    # Mark some tests with _errlog, we match on them later to adapt to the
    # pattern of split packages for error logs.
    "bsdsocket_errlog"
    "dgramsocket_errlog"
    "streamsocket"
    "fuchsia"
    "packetsocket"
    "rawsocket"
)

expectations_filename="expectations.json5"

for test in "${all_socket[@]}"
do
    component="netstack3_${test}_test"
    # The expectations comparer includes a mechanism for expecting a case to
    # generate error logs; when such expectations are present, the test is split
    # into:
    # 1) a package that allows error logs, in which cases generating those logs
    #    are run.
    # 2) a package that does not allow error logs, in which cases generating
    #    those logs are skipped.
    #
    # To keep things simple, examine only the second (non error log) package.
    # That means test cases expecting pass/failure with error logs will show up
    # as `Skip` when this tool is used.
    #
    # TODO(https://fxbug.dev/42064224): Support listing expects with error logs.
    if [[ $test == *"_errlog" ]]; then
        testname=${component/_errlog_test/_test_no_err_logs}
        expect=$expectations_filename
    else
        testname=$component
        # This path is enforced by the fuchsia_test_component_with_expectations
        # template.
        expect="data/tests/expectations/${component}_component/$expectations_filename"
    fi
    cmp_expectations_assume_first_passes "$testname" "$expect"
done

for test in "${all_syscalls[@]}"
do
    cmp_expectations "netstack2_syncudp_${test}_syscall" $expectations_filename "netstack3_${test}_syscall" $expectations_filename
done

