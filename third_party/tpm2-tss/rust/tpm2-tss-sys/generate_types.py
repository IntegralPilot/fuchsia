#!/usr/bin/env python3
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This script will generate a mostly complete tss2_tpm_types.rs file but it
# will still require some editing as we aren't trying to write a complete
# c-preprocessor parser in Python.

import re

with open('tss2_tpm2_types.rs', 'w') as w:
  with open('../../src/include/tss2/tss2_tpm2_types.h', 'r') as r:
    for line in r.readlines():
      # Types
      m = re.match(r'typedef\s+(\w+)\s+(\w+);', line)
      if m:
        type_name = m.group(2)
        type_value = m.group(1)
        print(f'pub type {type_name} = {type_value};', file=w)
        continue
      # Constants
      m = re.match(r'#define\s+(\w+)\s+\(\((\w+)\) (\w+)\)', line)
      if m:
        var_name = m.group(1)
        var_type = m.group(2)
        var_val = m.group(3)
        print(f'pub const {var_name}: {var_type} = {var_val};', file=w)
        continue
      # Ignore other preprocessor macros
      m = re.match(r'#.*', line)
      if m:
        continue
