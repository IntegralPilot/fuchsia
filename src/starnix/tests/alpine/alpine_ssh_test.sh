# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -ex

/usr/sbin/sshd -f /etc/ssh/sshd_config -E /tmp/ssh.log
sshpass -p root ssh -o StrictHostKeychecking=no localhost -p 7000 "echo Connected && exit"
echo Success
