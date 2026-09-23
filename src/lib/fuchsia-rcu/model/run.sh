#!/bin/bash

# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Compilation and execution script for RCU test with CDSChecker

echo "Compiling rcu_test.cc..."
g++ -std=c++11 -I"cdschecker/include" -L"cdschecker" -Wl,-rpath,"cdschecker" "rcu_test.cc" -lmodel -o "rcu_test" -O3

if [ $? -eq 0 ]; then
    echo "Running rcu_test (with 120s timeout)..."
    if [ $# -eq 0 ]; then
        timeout 120s "./rcu_test" -f 20
    else
        timeout 120s "./rcu_test" "$@"
    fi
else
    echo "Compilation failed."
    exit 1
fi
