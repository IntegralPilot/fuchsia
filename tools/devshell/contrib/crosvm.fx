# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

#### CATEGORY=Run, inspect and debug
#### EXECUTABLE=${PREBUILT_3P_DIR}/crosvm/${HOST_PLATFORM}/crosvm
### Run crosvm

## Usage: fx crosvm [crosvm args...]
##
## This just invokes the prebuilt crosvm directly.  This is not a wrapper
## like `fx qemu` for passing useful options to run a Fuchsia image.
##
## For trivial cases, `fx crosvm run --initrd ZBI .../linux-*-boot-shim.bin`
## may be sufficient.
##
## For usage details, visit https://crosvm.dev/book/ or see a summary with
## `fx crosvm --help` and various `fx crosvm <subcommand> --help` commands.
