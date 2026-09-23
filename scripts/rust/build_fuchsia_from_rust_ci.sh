#!/usr/bin/env bash
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This script is run from the Rust Project CI to build Fuchsia as a way of
# catching regressions. If you are a Rust developer, see the comments toward
# the end of this file for more information about how the Fuchsia build works
# and how to customize it. More documentation can be found in the Rustc
# Developer Guide.

# To test this script locally, run it from inside a Fuchsia checkout with
# RUST_INSTALL_DIR pointing to the "install" directory of a Rust toolchain.
# Be warned that the script MODIFIES the toolchain (requires write permissions)
# and reconfigures the current Fuchsia build.

set -eu -o pipefail

print_banner() {
  {
    git_commit=$(git rev-parse HEAD)
    echo
    echo "###############################################################################"
    echo "#                                                                             #"
    echo "#  This check builds the Fuchsia operating system.                            #"
    echo "#                                                                             #"
    echo "#  Most code is built in 'check mode' using clippy, to maximize coverage in   #"
    echo "#  the compiler front end while reducing build times.                         #"
    echo "#                                                                             #"
    echo "#  You can browse the Fuchsia source code at the following URL:               #"
    echo "#  https://cs.opensource.google/fuchsia/fuchsia/+/$git_commit:"
    echo "#                                                                             #"
    echo "###############################################################################"
    echo
  } >&2
}

# toabs converts the possibly relative argument into an absolute path. Run in a
# subshell to avoid changing the caller's working directory.
toabs() (
  cd $(dirname $1)
  echo ${PWD}/$(basename $1)
)

fuchsia=$(toabs $(dirname $0)/../..)
fx=$fuchsia/.jiri_root/bin/fx

if ! [ -d $RUST_INSTALL_DIR ]; then
    echo "RUST_INSTALL_DIR must be set to a valid value: $RUST_INSTALL_DIR"
    exit 1
fi

rust_prefix=$RUST_INSTALL_DIR

# Generate a minimal runtime.json.
(
  cd $rust_prefix/lib
  x64_libstd_path=$(ls rustlib/x86_64-unknown-fuchsia/lib/libstd-*.so)
  x64_libstd_soname=$(basename $x64_libstd_path)
  cat <<END >runtime.json
  [
    {
      "runtime": [],
      "rustflags": [],
      "target": [
        "aarch64-unknown-fuchsia"
      ]
    },
    {
      "runtime": [],
      "rustflags": [],
      "target": [
        "x86_64-unknown-fuchsia"
      ]
    },
    {
      "runtime": [
      ],
      "rustflags": [
        "-Cprefer-dynamic"
      ],
      "target": [
        "aarch64-unknown-fuchsia"
      ]
    },
    {
      "runtime": [
        {
          "name": "libstd",
          "dist": "$x64_libstd_path",
          "soname": "$x64_libstd_soname"
        }
      ],
      "rustflags": [
        "-Cprefer-dynamic"
      ],
      "target": [
        "x86_64-unknown-fuchsia"
      ]
    }
  ]
END
)

$fx metrics disable || echo "Warning: Failed to disable metrics"

print_banner

# Detect Rust toolchain changes by hashing the entire toolchain.
version_string="$(find $rust_prefix -type f -print0 | sort -z | xargs -0 sha1sum | sha1sum | awk '{print $1}')"

set -x

# Here are the build arguments used when building Fuchsia.
#
# These wire up the compiler from CI and customize the build for the Rust CI
# environment in a few ways:
#
# - Don't fail the build because of new warnings, since we don't use the pinned
#   compiler version.
# - Disable debuginfo, which speeds up the build by about 8%.
# - Disable some rustc wrapper scripts that perform unnecessary build checks.
# - Build the bundle of unit tests called "workbench".
#
# `fx set` creates a file called `out/default/args.gn` with the build arguments
# and then runs GN, Fuchsia's meta-build system which generates ninja files. You
# may also modify args.gn directly and rerun the build, which will pick up the
# changes automatically.
$fx set \
    --args "rustc_prefix = \"$rust_prefix\"" \
    --args "rustc_version_string = \"$version_string\"" \
    --args 'rust_cap_lints = "warn"' \
    --with '//bundles/buildbot/workbench' \
    workbench_eng.x64 \
    ; echo

# Now run the build. We use `fx clippy` to drive the build because it reduces the
# amount of Rust code that we actually need to produce binaries for. Under the
# hood `fx clippy` runs ninja.
set +e
time $fx clippy --all
retcode=$?
set -e

set +x

print_banner

exit $retcode
