#!/boot/bin/sh
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

/boot/bin/component explore /core/usb-policy -c "usb-cli $*"
