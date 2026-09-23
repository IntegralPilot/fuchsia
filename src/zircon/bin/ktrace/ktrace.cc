// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/defer.h>
#include <lib/zircon-internal/ktrace.h>
#include <lib/zx/resource.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <zircon/status.h>

#include <fbl/string.h>
#include <fbl/unique_fd.h>

namespace {

constexpr char kUsage[] =
    "\
Usage: ktrace [options] <control>\n\
Where <control> is one of:\n\
  start <group_mask>         - start tracing\n\
  stop                       - stop tracing\n\
  rewind                     - rewind trace buffer\n\
  stream <group_mask> <secs> <path> - stream trace data for <secs> seconds\n\
  written                    - print bytes written to trace buffer\n\
    Note: This value doesn't reset on \"rewind\". Instead, the rewind\n\
    takes effect on the next \"start\".\n\
  save <path>                - save contents of trace buffer to <path>\n\
\n\
Options:\n\
  --help  - Print this help.\n\
";

void PrintUsage(FILE* f) { fputs(kUsage, f); }

int DoStart(const zx::resource& tracing_resource, uint32_t group_mask) {
  if (zx_status_t status =
          zx_ktrace_control(tracing_resource.get(), KTRACE_ACTION_START, group_mask, nullptr);
      status != ZX_OK) {
    fprintf(stderr, "Error starting ktrace: %s(%d)\n", zx_status_get_string(status), status);
    return EXIT_FAILURE;
  }
  return EXIT_SUCCESS;
}

int DoStop(const zx::resource& tracing_resource) {
  if (zx_status_t status =
          zx_ktrace_control(tracing_resource.get(), KTRACE_ACTION_STOP, 0, nullptr);
      status != ZX_OK) {
    fprintf(stderr, "Error stopping ktrace: %s(%d)\n", zx_status_get_string(status), status);
    return EXIT_FAILURE;
  }
  return EXIT_SUCCESS;
}

int DoRewind(const zx::resource& tracing_resource) {
  if (zx_status_t status =
          zx_ktrace_control(tracing_resource.get(), KTRACE_ACTION_REWIND, 0, nullptr);
      status != ZX_OK) {
    fprintf(stderr, "Error rewinding ktrace: %s(%d)\n", zx_status_get_string(status), status);
    return EXIT_FAILURE;
  }
  return EXIT_SUCCESS;
}

int DoStream(const zx::resource& tracing_resource, uint32_t group_mask,
             std::chrono::seconds duration, const char* path) {
  fbl::unique_fd out_fd(open(path, O_CREAT | O_TRUNC | O_WRONLY, 0666));
  if (!out_fd.is_valid()) {
    fprintf(stderr, "Unable to open file for writing: %s, %s\n", path, strerror(errno));
    return EXIT_FAILURE;
  }

  using namespace std::chrono_literals;
  // Start by using a 50ms polling rate. We'll adjust dynamically for the amount of data we actually
  // receive.
  auto polling_interval = 50ms;
  auto start_time = std::chrono::steady_clock::now();
  auto end_time = start_time + duration;

  size_t read_size;
  if (zx_status_t status = zx_ktrace_read(tracing_resource.get(), nullptr, 0, 0, &read_size);
      status != ZX_OK) {
    fprintf(stderr, "Failed to query expected ktrace buffer size\n");
    return EXIT_FAILURE;
  }
  std::unique_ptr buf = std::make_unique<uint8_t[]>(read_size);
  if (zx_status_t status =
          zx_ktrace_control(tracing_resource.get(), KTRACE_ACTION_REWIND, 0, nullptr);
      status != ZX_OK) {
    fprintf(stderr, "Error rewinding ktrace: %s(%d)\n", zx_status_get_string(status), status);
    return EXIT_FAILURE;
  }
  if (zx_status_t status =
          zx_ktrace_control(tracing_resource.get(), KTRACE_ACTION_START, group_mask, nullptr);
      status != ZX_OK) {
    fprintf(stderr, "Error starting ktrace: %s(%d)\n", zx_status_get_string(status), status);
    return EXIT_FAILURE;
  }
  size_t actual;
  std::chrono::steady_clock::time_point next_service = std::chrono::steady_clock::now();

  while (next_service < end_time) {
    zx_status_t status = zx_ktrace_read(tracing_resource.get(), buf.get(), 0, read_size, &actual);

    if (status != ZX_OK) {
      fprintf(stderr, "Failed to zx_ktrace_read: %d\n", status);
      break;
    }
    size_t written = 0;
    while (written < actual) {
      size_t bytes_written = write(out_fd.get(), buf.get() + written, actual - written);
      written += bytes_written;
    }

    // Attempt to adapt our polling interval to the actual buffer rate. Nothing fancy, just reading
    // attempting to poll at a rate that keeps the buffer use at around 25% each time we read. That
    // way, if we'd need a 4x spike of trace data output over the polling interval to overflow the
    // buffer.
    //
    size_t new_poll = (polling_interval.count() * read_size) / (actual * 4);

    // Clamp the value between 1ms and 100ms.
    //
    // Servicing the buffer takes on the order of 100-200us. Faster than 1ms and we begin hogging a
    // significant amount of CPU.
    //
    // Above 100ms, we're already using a smaller percent of cpu polling the buffer. We don't want
    // it to get too big else we could miss a burst of activity after a long idle period.
    polling_interval = std::chrono::milliseconds(std::clamp(new_poll, size_t{1}, size_t{100}));

    next_service += polling_interval;
    std::this_thread::sleep_until(next_service);
  }

  if (zx_status_t status =
          zx_ktrace_control(tracing_resource.get(), KTRACE_ACTION_STOP, 0, nullptr);
      status != ZX_OK) {
    fprintf(stderr, "Error stopping ktrace: %s(%d)\n", zx_status_get_string(status), status);
    return EXIT_FAILURE;
  }
  return EXIT_SUCCESS;
}

int DoWritten(const zx::resource& tracing_resource) {
  uint64_t bytes_written;
  if (zx_status_t status = zx_ktrace_read(tracing_resource.get(), nullptr, 0, 0, &bytes_written);
      status != ZX_OK) {
    fprintf(stderr, "Error getting bytes written: %s(%d)\n", zx_status_get_string(status), status);
    return EXIT_FAILURE;
  }
  printf("Bytes written: %ld\n", bytes_written);
  return EXIT_SUCCESS;
}

int DoSave(const zx::resource& tracing_resource, const char* path) {
  fbl::unique_fd out_fd(open(path, O_CREAT | O_TRUNC | O_WRONLY, 0666));
  if (!out_fd.is_valid()) {
    fprintf(stderr, "Unable to open file for writing: %s, %s\n", path, strerror(errno));
    return EXIT_FAILURE;
  }

  size_t read_size;
  if (zx_status_t status = zx_ktrace_read(tracing_resource.get(), nullptr, 0, 0, &read_size);
      status != ZX_OK) {
    fprintf(stderr, "Failed to query expected ktrace buffer size: %s(%d)\n",
            zx_status_get_string(status), status);
    return EXIT_FAILURE;
  }

  std::unique_ptr buf = std::make_unique<uint8_t[]>(read_size);

  zx_status_t status;
  size_t actual;
  while ((status = zx_ktrace_read(tracing_resource.get(), buf.get(), 0, read_size, &actual)) ==
         ZX_OK) {
    if (actual == 0) {
      break;
    }
    size_t bytes_written = write(out_fd.get(), buf.get(), actual);
    if (bytes_written < 0) {
      fprintf(stderr, "I/O error saving buffer: %s\n", strerror(errno));
      return EXIT_FAILURE;
    }
    if (bytes_written != actual) {
      fprintf(stderr, "Short write saving buffer: %zd vs %zd\n", bytes_written, actual);
      return EXIT_FAILURE;
    }
  }

  if (status != ZX_OK) {
    fprintf(stderr, "Error reading ktrace: %s(%d)\n", zx_status_get_string(status), status);
    return EXIT_FAILURE;
  }

  return EXIT_SUCCESS;
}

void EnsureNArgs(const fbl::String& cmd, int argc, int expected_argc) {
  if (argc != expected_argc) {
    fprintf(stderr, "Unexpected number of args for command %s\n", cmd.c_str());
    PrintUsage(stderr);
    exit(EXIT_FAILURE);
  }
}
}  // namespace

int main(int argc, char** argv) {
  if (argc >= 2 && strcmp(argv[1], "--help") == 0) {
    PrintUsage(stdout);
    return EXIT_SUCCESS;
  }

  if (argc < 2) {
    PrintUsage(stderr);
    return EXIT_FAILURE;
  }
  const fbl::String cmd{argv[1]};

  auto tracing_client_end = component::Connect<fuchsia_kernel::TracingResource>();
  if (tracing_client_end.is_error()) {
    fprintf(stderr, "Error in getting tracing resource: %s(%d)\n",
            tracing_client_end.status_string(), tracing_client_end.status_value());
    return 1;
  }
  auto tracing_result = fidl::SyncClient(std::move(*tracing_client_end))->Get();
  if (!tracing_result.is_ok()) {
    fprintf(stderr, "Error in getting tracing resource: %s\n",
            tracing_result.error_value().status_string());
    return 1;
  }

  zx::resource tracing_resource = std::move(tracing_result->resource());

  if (cmd == "start") {
    EnsureNArgs(cmd, argc, 3);
    int group_mask = atoi(argv[2]);
    if (group_mask < 0) {
      fprintf(stderr, "Invalid group mask\n");
      return EXIT_FAILURE;
    }
    return DoStart(tracing_resource, group_mask);
  } else if (cmd == "stream") {
    EnsureNArgs(cmd, argc, 5);
    int group_mask = atoi(argv[2]);
    if (group_mask < 0) {
      fprintf(stderr, "Invalid group mask\n");
      return EXIT_FAILURE;
    }
    int duration = atoi(argv[3]);
    if (duration <= 0) {
      fprintf(stderr, "Invalid duration\n");
      return EXIT_FAILURE;
    }
    const char* path = argv[4];
    return DoStream(tracing_resource, group_mask, std::chrono::seconds{duration}, path);
  } else if (cmd == "stop") {
    EnsureNArgs(cmd, argc, 2);
    return DoStop(tracing_resource);
  } else if (cmd == "rewind") {
    EnsureNArgs(cmd, argc, 2);
    return DoRewind(tracing_resource);
  } else if (cmd == "written") {
    EnsureNArgs(cmd, argc, 2);
    return DoWritten(tracing_resource);
  } else if (cmd == "save") {
    EnsureNArgs(cmd, argc, 3);
    const char* path = argv[2];
    return DoSave(tracing_resource, path);
  }

  PrintUsage(stderr);
  return EXIT_FAILURE;
}
