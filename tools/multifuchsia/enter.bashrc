# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

if [ -f /etc/bash.bashrc ]; then
  source /etc/bash.bashrc
fi
if [ -f ~/.bashrc ]; then
  source ~/.bashrc
fi
PS1="[${MULTIFUCHSIA_ENTER_ENV}] ${PS1}"
