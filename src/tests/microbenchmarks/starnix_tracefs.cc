// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/syslog/cpp/macros.h>
#include <string.h>
#include <sys/mount.h>

#include <cerrno>
#include <cstdio>
#include <memory>

#include <perftest/perftest.h>

// Test fixture to manage mounting and unmounting tracefs.
class TraceFs {
 public:
  TraceFs() = default;
  TraceFs(TraceFs&&) = default;
  TraceFs& operator=(TraceFs&&) = default;

  TraceFs(const TraceFs&) = delete;
  TraceFs& operator=(const TraceFs&) = delete;

  ~TraceFs() { umount("/sys/kernel/tracing"); }

  // RAII wrapper for FILE*.
  using unique_file = std::unique_ptr<FILE, decltype(&fclose)>;

  static std::optional<TraceFs> Mount() {
    if (mount("tracefs", "/sys/kernel/tracing", "tracefs", MS_NODEV, nullptr) != 0) {
      fprintf(stderr, "Failed to mount tracefs!: %s\n", strerror(errno));
      return std::nullopt;
    }
    return std::make_optional<TraceFs>();
  }

  unique_file Open(const char* path, const char* mode) const {
    FILE* file = fopen(path, mode);
    if (file == nullptr) {
      fprintf(stderr, "Failed to open %s: %s\n", path, strerror(errno));
    }
    return {file, &fclose};
  }
};

namespace {

// Measure the time taken to write an atrace style event to tracefs
bool WriteEvent(perftest::RepeatState* state) {
  std::optional<TraceFs> tracefs = TraceFs::Mount();
  FX_CHECK(tracefs.has_value());

  auto tracing_file = tracefs->Open("/sys/kernel/tracing/tracing_on", "w");
  FX_CHECK(tracing_file != nullptr);
  fputs("1\n", tracing_file.get());
  // Don't forget to flush! Or else we may not actually write the file.
  fflush(tracing_file.get());

  auto marker_file = tracefs->Open("/sys/kernel/tracing/trace_marker", "w");
  FX_CHECK(marker_file != nullptr);

  while (state->KeepRunning()) {
    fputs("B|1234|slice", marker_file.get());
    // We flush every time so we actually measure the trip into the syscall.
    fflush(marker_file.get());
  }
  fputs("0\n", tracing_file.get());
  return true;
}

// Measure the time to write to tracefs when tracefs isn't enabled. This measures the syscall and
// vfs overhead in tracefs.
bool WriteDisabled(perftest::RepeatState* state) {
  std::optional<TraceFs> tracefs = TraceFs::Mount();
  FX_CHECK(tracefs.has_value());

  auto marker_file = tracefs->Open("/sys/kernel/tracing/trace_marker", "w");
  FX_CHECK(marker_file != nullptr);

  while (state->KeepRunning()) {
    fputs("B|1234|slice", marker_file.get());
    // We flush every time so we actually measure the trip into the syscall.
    fflush(marker_file.get());
  }
  return true;
}

void RegisterTests() {
  perftest::RegisterTest("TraceFs/WriteDisabled", WriteDisabled);
  perftest::RegisterTest("TraceFs/WriteEvent", WriteEvent);
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
