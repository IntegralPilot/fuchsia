#!/bin/bash
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

readonly script="$0"
readonly script_base="${script##*/}"  # basename

# This script helps determine whether the user's LOAS credentials type
# is restricted or unrestricted.  The outcome influences the authentication
# strategy that can be used by tools that access Google Cloud Platform
# services, like RBE.
# The outcome of this script could change if your network configuration
# changes, e.g. your gLinux laptop switches between privileged
# and unprivileged networks.
# The outcome will rarely change for devices with physical network
# connections, and in practice this result can be cached.

function usage() {
cat <<EOF
Determine whether or not host environment can get unrestricted LOAS
credentials.
Prints "restricted" or "unrestricted" and exits 0 if no errors were encountered.
Exits non-zero if some other error occurred.
EOF
}

function error() {
  >&2 echo "[$script_base]" "$@"
}

function fatal() {
  >&2 echo "[$script_base]" "$@"
  exit 1
}

verbose=0
# Process options.
for opt in "$@"
do
  case "$opt" in
    -h|--help) usage ; exit ;;
    -v|--verbose) verbose=1 ;;
    *) fatal "Unknown option $opt" ;;
  esac
done

function vmsg() {
  [[ "$verbose" == 0 ]] || echo "[$script_base]" "$@"
}

# No gcert, no LOAS
which gcert > /dev/null || fatal "gcert not found."
which gcertstatus > /dev/null || fatal "gcertstatus not found."

# Renew certificate if needed (interactive).
# To force re-auth for a fresh certificate, run 'loas_destroy' first.
gcertstatus -check_ssh=false > /dev/null || gcert || {
  error "Failed to renew LOAS2 certificate."
  echo "restricted"
  exit 0
}

# Note: accessing BinFS (/google/bin) requires LOAS.
readonly cred_printer=/google/bin/releases/prodsec/tools/credential_printer
test -x "$cred_printer" || {
  error "Unable to access $cred_printer.  See go/binfs-glinux-workstations."
  echo "restricted"
  exit 0
}

"$cred_printer" > /dev/null 2>&1 || {
  # For gLinux laptops, this may be blocked on b/380510724, b/380507052.
  # Conservatively, treat credentials as restricted.
  echo "restricted"
  exit 0
}

# Locate LOAS credentials file.
creds_file=
case "$OSTYPE" in
  linux*) creds_file="/run/credentials-cache/loas-$USER/cookies/$USER.loas2credentials" ;;  # assuming gLinux
  darwin*) creds_file="/var/run/credentials-cache/loas-$USER/cookies/$USER.loas2credentials" ;;  # assuming gMac
  *) fatal "Unhandled OS: $OSTYPE" ;;
esac
vmsg "Examining credentials file: $creds_file"

# Check for unrestricted LOAS credentials.
restriction_lines="$("$cred_printer" "$creds_file" | sed -n -e '/\[AcctRestrictions\]/,/}/p' | wc -l)"
if [[ "$restriction_lines" == 2 ]]
then
  # restrictions section is empty, meaning unrestricted
  echo "unrestricted"
else
  # section is non-empty, which means restricted
  echo "restricted"
fi
