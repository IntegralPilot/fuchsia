# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

if [ -f ~/.zshenv ]; then
  source ~/.zshenv
fi
# Don't look for system RC files (we will source manually)
unsetopt GLOBAL_RCS
