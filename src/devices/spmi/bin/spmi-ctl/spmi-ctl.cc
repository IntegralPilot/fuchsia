// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "spmi-ctl-impl.h"

int main(int argc, char** argv) {
  SpmiCtl spmi_ctl;
  return spmi_ctl.Execute(argc, argv);
}
