// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/zx/clock.h>

#include <fbl/string.h>
#include <fbl/unique_fd.h>
#include <fbl/vector.h>
#include <runtests-utils/fuchsia-run-test.h>
#include <runtests-utils/log-exporter.h>
#include <runtests-utils/runtests-utils.h>

namespace {

// The name of the file containing the syslog.
constexpr char kSyslogFileName[] = "syslog.txt";

const char* kDefaultTestDirs[] = {
    // zircon builds place everything in ramdisks so tests are located in /boot
    "/boot/test",     "/boot/test/c",   "/boot/test/core", "/boot/test/libc",
    "/boot/test/ddk", "/boot/test/sys", "/boot/test/fs",   "/boot/test/storage",
};

class FuchsiaStopwatch final : public runtests::Stopwatch {
 public:
  FuchsiaStopwatch() { Start(); }
  void Start() override { start_time_ = Now(); }
  int64_t DurationInMsecs() override { return (Now() - start_time_).to_msecs(); }

 private:
  static zx::time Now() { return zx::clock::get_monotonic(); }

  zx::time start_time_;
};

// Parse |argv| for an output directory argument.
const char* GetOutputDir(int argc, const char* const* argv) {
  int i = 1;
  while (i < argc - 1 && (strcmp(argv[i], "--output") != 0 && strcmp(argv[i], "-o") != 0)) {
    ++i;
  }
  if (i >= argc - 1) {
    return nullptr;
  }
  return argv[i + 1];
}

}  // namespace

int main(int argc, char** argv) {
  const char* output_dir = GetOutputDir(argc, argv);

  // Start Log Listener.
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  std::unique_ptr<runtests::LogExporter> log_exporter_ptr;
  if (output_dir != nullptr) {
    int error = runtests::MkDirAll(output_dir);
    if (error) {
      printf("Error: Could not create output directory: %s, %s\n", output_dir, strerror(error));
      return -1;
    }

    runtests::ExporterLaunchError exporter_error;
    log_exporter_ptr = runtests::LaunchLogExporter(
        loop.dispatcher(), runtests::JoinPath(output_dir, kSyslogFileName), &exporter_error);
    // Don't fail if logger service is not available because it is only
    // available in core and above.
    if (!log_exporter_ptr &&
        exporter_error != runtests::ExporterLaunchError::CONNECT_TO_LOGGER_SERVICE) {
      printf("Error: Failed to launch log listener: %d\n", static_cast<int>(exporter_error));
      return -1;
    }
    if (zx_status_t status = loop.StartThread(); status != ZX_OK) {
      printf("Error: Failed to start log exporter: %d (%s).\n", static_cast<int>(status),
             zx_status_get_string(status));
      return -1;
    }
  }

  fbl::Vector<fbl::String> default_test_dirs;
  for (auto& kDefaultTestDir : kDefaultTestDirs) {
    default_test_dirs.push_back(kDefaultTestDir);
  }

  // runtests is used to run tests in bringup configurations. Because the
  // console shell dynamically mounts the "/dev" directory during system
  // startup, we need to wait for "/dev" to appear, before running any tests
  // which may depend on nodes inside "/dev".
  //
  // We are able to open the namespace root because the shell has a single
  // namespace entry at "/". See /src/bringup/bin/console-launcher/main.cc.
  fbl::unique_fd root(open("/", O_RDONLY | O_DIRECTORY));
  ZX_ASSERT_MSG(root.is_valid(),
                "runtests must be given \"/\" as the sole entry "
                "in its incoming namespace");
  zx::result channel = device_watcher::RecursiveWaitForFile(root.get(), "dev");
  if (channel.is_error()) {
    printf("Error: Failed to wait for /dev to appear: %s", channel.status_string());
    return -1;
  }

  FuchsiaStopwatch stopwatch;
  return runtests::DiscoverAndRunTests(argc, argv, default_test_dirs, &stopwatch, kSyslogFileName);
}
