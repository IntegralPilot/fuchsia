# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# reset ZDOTDIR back to default
unset ZDOTDIR

if [ -f /etc/zsh/zshrc ]; then
  source /etc/zsh/zshrc
fi

if [ -f ~/.zshrc ]; then
  source ~/.zshrc
fi
PS1="[${MULTIFUCHSIA_ENTER_ENV}] ${PS1}"
