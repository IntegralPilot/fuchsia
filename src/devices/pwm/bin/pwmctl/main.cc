// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.pwm/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <stdio.h>

#include "pwmctl.h"

void usage(int argc, char const* argv[]) {
  fprintf(stderr, "Usage: %s <device> <command> [args]\n", argv[0]);
  fprintf(stderr, "enable                    Enables the PWM\n");
  fprintf(stderr, "disable                   Disables the PWM\n");
  fprintf(stderr, "config <pol> <per> <d>    Sets the polarity (pol), and\n");
  fprintf(stderr, "                          period (per) and duty cycle (d) of the PWM.\n");
  fprintf(stderr, "                          Polarity must be 0 or 1\n");
  fprintf(stderr, "                          Period must be a positive integer in nanoseconds\n");
  fprintf(stderr, "                          Duty cycle must be a float [0.0, 100.0]\n");
}

int main(int argc, char const* argv[]) {
  if (argc < 3) {
    fprintf(stderr, "Expected at least 3 arguments\n");
    usage(argc, argv);
    return -1;
  }

  const char* devpath = argv[1];
  zx::result client_end = component::Connect<fuchsia_hardware_pwm::Pwm>(devpath);
  if (client_end.is_error()) {
    fprintf(stderr, "Failed to open %s: %s\n", devpath, client_end.status_string());
    return -1;
  }
  zx_status_t st = pwmctl::run(argc, argv, std::move(client_end.value()));
  return st == ZX_OK ? 0 : -1;
}
