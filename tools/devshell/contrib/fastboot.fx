# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

#### CATEGORY=Device management
### interact directly with a device's bootloader via the fastboot protocol

## USAGE:
##     fx fastboot [arguments to fastboot]
##
## This simply runs the prebuilt fastboot binary with the arguments given.
## See "fx fastboot help" for options to fastboot.

#### EXECUTABLE=${PREBUILT_3P_DIR}/fastboot/fastboot
