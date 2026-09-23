// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <stdarg.h>
#include <stdio.h>

#include <phys/stdio.h>

#include "log.h"

FILE gStdout;

PhysConsole::PhysConsole()
    : null_{[](ktl::string_view s) -> int { return static_cast<int>(s.length()); }}, mux_files_{} {}

PhysConsole& PhysConsole::Get() {
  static PhysConsole gConsole;
  return gConsole;
}

void InitStdout() { gStdout = PhysConsole::Get().mux_; }

void PhysConsole::SetMux(size_t idx, const FILE& f) {
  mux_files_[idx] = f;
  mux_.files()[idx] = &mux_files_[idx];
}

void debugf(const char* fmt, ...) {
  FILE* outfile = stdout;

  // If kernel.phys.verbose=false then only write to the log.
  FILE log_only_file;
  if (BootOptions::Get() && !BootOptions::Get()->phys_verbose) {
    if (!gLog) {
      // The log isn't initialized yet, so write nothing.
      return;
    }

    log_only_file = gLog->LogOnlyFile();
    outfile = &log_only_file;
  }

  va_list args;
  va_start(args, fmt);
  vfprintf(outfile, fmt, args);
  va_end(args);
}
