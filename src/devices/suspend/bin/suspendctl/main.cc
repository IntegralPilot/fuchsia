// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <iostream>

#include "suspendctl.h"

int main(int argc, char** argv) { return suspendctl::run(argc, argv); }
