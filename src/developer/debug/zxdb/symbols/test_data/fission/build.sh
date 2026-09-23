# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This file compiles the symbols for the debug fission tests (--gsplit-dwarf). This remaps absolute
# paths to "." so the resulting cross-file references will be valid on any system.
g++ -g -gsplit-dwarf -fdebug-prefix-map="`pwd`=." -o other.o -c other.cc
g++ -g -gsplit-dwarf -fdebug-prefix-map="`pwd`=." -o main.o -c main.cc
g++ -g -o fission other.o main.o
rm main.o other.o
