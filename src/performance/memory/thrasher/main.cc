// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fidl/fuchsia.fxfs/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fit/defer.h>

#include <atomic>
#include <charconv>
#include <cstdint>
#include <iomanip>
#include <iostream>
#include <mutex>
#include <sstream>
#include <string>
#include <unordered_set>
#include <vector>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/lib/fxl/strings/string_number_conversions.h"
#include "src/performance/memory/thrasher/lib.h"

namespace {
std::vector<std::string> ListBlobMerkles() {
  std::vector<std::string> merkle_roots;
  DIR* dir = opendir("/blob");
  if (!dir) {
    std::cerr << "Failed to open /blob directory: " << strerror(errno) << std::endl;
    return merkle_roots;
  }

  struct dirent* entry;
  while ((entry = readdir(dir)) != nullptr) {
    std::string name = entry->d_name;
    if (name != "." && name != "..") {
      merkle_roots.push_back(name);
    }
  }
  closedir(dir);
  return merkle_roots;
}

std::string FormatMemorySize(uint64_t bytes) {
  const char* suffixes[] = {"B", "KiB", "MiB", "GiB", "TiB"};
  int suffix_index = 0;
  double size = static_cast<double>(bytes);
  while (size >= 1024 && suffix_index < 4) {
    size /= 1024;
    suffix_index++;
  }
  std::stringstream ss;
  ss << std::fixed << std::setprecision(2) << size << " " << suffixes[suffix_index];
  return ss.str();
}

}  // namespace

int main(int argc, const char** argv) {
  const auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);

  if (command_line.HasOption("verbose")) {
    std::cout << "Starting (Version THRASHER_V3 time-redacted)" << std::endl;
  }

  if (command_line.HasOption("help")) {
    std::cout
        << "Usage: thrasher [--anon_size_mb=<MiB>] [--blob_size_mb=<MiB>] "
           "[--bursts_per_s=<bursts_per_second>] [--run_for_s=<seconds>] "
           "[--file=<path>] [--dir=<path>] [--num_threads=<threads>] "
           "[--consecutive_pages_per_read=<pages>] [--verbose]\n"
        << "  --thrashers: Comma-separated list of thrashers to run. "
        << "Options: anon, file, dir, blob. At least one required.\n"
        << "  --anon_size_mb: Size in MiB for anonymous memory thrashing (default 100).\n"
        << "  --blob_size_mb: Maximum size in MiB for blob thrashing (default 100).\n"
        << "  --status_interval_ms: Interval in milliseconds for status updates (default 1000).\n"
        << "  --verbose: Enable verbose logging, including VMO dumps." << std::endl;
    return 0;
  }

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  size_t anon_size_mb = 100;
  size_t blob_size_mb = 100;
  ThrashConfig config = {
      .bursts_per_second = 1000,
      .run_for_seconds = 60,
      .num_threads = 1,
      .pages_per_read = 1,
      .consecutive_pages_per_read = 1,
      .dispatcher = loop.dispatcher(),
      .verbose = command_line.HasOption("verbose"),
  };
  std::string file_path;
  std::string dir_path;

  std::string anon_size_str;
  if (command_line.GetOptionValue("anon_size_mb", &anon_size_str)) {
    if (!fxl::StringToNumberWithError(anon_size_str, &anon_size_mb)) {
      std::cerr << "Invalid value for --anon_size_mb: " << anon_size_str << std::endl;
      return 1;
    }
  }

  std::string blob_size_str;
  if (command_line.GetOptionValue("blob_size_mb", &blob_size_str)) {
    if (!fxl::StringToNumberWithError(blob_size_str, &blob_size_mb)) {
      std::cerr << "Invalid value for --blob_size_mb: " << blob_size_str << std::endl;
      return 1;
    }
  }

  std::string rate_str;
  if (command_line.GetOptionValue("bursts_per_s", &rate_str)) {
    if (!fxl::StringToNumberWithError(rate_str, &config.bursts_per_second)) {
      std::cerr << "Invalid value for --bursts_per_s: " << rate_str << std::endl;
      return 1;
    }
  }

  std::string run_for_s_str;
  if (command_line.GetOptionValue("run_for_s", &run_for_s_str)) {
    if (!fxl::StringToNumberWithError(run_for_s_str, &config.run_for_seconds)) {
      std::cerr << "Invalid value for --run_for_s: " << run_for_s_str << std::endl;
      return 1;
    }
  }

  std::string num_threads_str;
  if (command_line.GetOptionValue("num_threads", &num_threads_str)) {
    if (!fxl::StringToNumberWithError(num_threads_str, &config.num_threads)) {
      std::cerr << "Invalid value for --num_threads: " << num_threads_str << std::endl;
      return 1;
    }
  }

  std::string pages_per_read_str;
  if (command_line.GetOptionValue("pages_per_read", &pages_per_read_str)) {
    if (!fxl::StringToNumberWithError(pages_per_read_str, &config.pages_per_read)) {
      std::cerr << "Invalid value for --pages_per_read: " << pages_per_read_str << std::endl;
      return 1;
    }
  }

  std::string consecutive_pages_per_read_str;
  if (command_line.GetOptionValue("consecutive_pages_per_read", &consecutive_pages_per_read_str)) {
    if (!fxl::StringToNumberWithError(consecutive_pages_per_read_str,
                                      &config.consecutive_pages_per_read)) {
      std::cerr << "Invalid value for --consecutive_pages_per_read: "
                << consecutive_pages_per_read_str << std::endl;
      return 1;
    }
  }

  std::string status_interval_str;
  if (command_line.GetOptionValue("status_interval_ms", &status_interval_str)) {
    if (!fxl::StringToNumberWithError(status_interval_str, &config.status_interval_ms)) {
      std::cerr << "Invalid value for --status_interval_ms: " << status_interval_str << std::endl;
      return 1;
    }
  }

  size_t max_blob_size_bytes = blob_size_mb * 1024 * 1024;

  command_line.GetOptionValue("file", &file_path);
  command_line.GetOptionValue("dir", &dir_path);

  std::string thrashers_str;
  if (!command_line.GetOptionValue("thrashers", &thrashers_str) || thrashers_str.empty()) {
    std::cerr << "--thrashers is required." << std::endl;
    return 1;
  }

  std::unordered_set<std::string> thrashers_set;
  std::stringstream ss(thrashers_str);
  std::string thrasher_type;
  const std::unordered_set<std::string> valid_thrashers = {"anon", "file", "dir", "blob"};
  while (std::getline(ss, thrasher_type, ',')) {
    if (valid_thrashers.find(thrasher_type) == valid_thrashers.end()) {
      std::cerr << "Invalid thrasher type: " << thrasher_type << std::endl;
      std::cerr << "Valid types are: anon, file, dir, blob" << std::endl;
      return 1;
    }
    thrashers_set.insert(thrasher_type);
  }

  auto clone_config = [&config]() {
    ThrashConfig copy = {
        .bursts_per_second = config.bursts_per_second,
        .run_for_seconds = config.run_for_seconds,
        .num_threads = config.num_threads,
        .pages_per_read = config.pages_per_read,
        .consecutive_pages_per_read = config.consecutive_pages_per_read,
        .dispatcher = config.dispatcher,
        .verbose = config.verbose,
        .status_interval_ms = config.status_interval_ms,
    };
    return copy;
  };

  std::vector<std::shared_ptr<Thrasher>> thrashers;
  if (thrashers_set.count("anon")) {
    thrashers.push_back(CreateAnonThrasher(clone_config(), anon_size_mb * 1024 * 1024));
  }
  if (thrashers_set.count("file")) {
    if (file_path.empty()) {
      std::cerr << "--file must be specified for --thrashers=file" << std::endl;
      return 1;
    }
    thrashers.push_back(CreateMmapThrasher(clone_config(), file_path));
  }
  if (thrashers_set.count("dir")) {
    if (dir_path.empty()) {
      std::cerr << "--dir must be specified for --thrashers=dir" << std::endl;
      return 1;
    }
    thrashers.push_back(CreateDirThrasher(clone_config(), dir_path));
  }
  if (thrashers_set.count("blob")) {
    std::vector<std::string> merkle_roots = ListBlobMerkles();
    if (!merkle_roots.empty()) {
      thrashers.push_back(
          CreateBlobThrasher(clone_config(), std::move(merkle_roots), max_blob_size_bytes));
    } else {
      std::cerr << "No blobs found in /blob" << std::endl;
    }
  }

  if (thrashers.empty()) {
    std::cerr << "No valid thrashers to run." << std::endl;
    return 1;
  }

  std::mutex mutex;
  std::vector<zx::vmo> collected_vmos;
  std::atomic<int> completed_thrashers = 0;
  int num_thrashers = static_cast<int>(thrashers.size());

  auto thrash_callback = std::make_shared<ThrashCallback>([&](std::vector<zx::vmo> vmos) {
    std::lock_guard<std::mutex> lock(mutex);
    for (auto& vmo : vmos) {
      collected_vmos.push_back(std::move(vmo));
    }
    if (++completed_thrashers == num_thrashers) {
      loop.Quit();
    }
  });

  auto status_callback = std::make_shared<StatusCallback>([](const ThrashStatus& status) {
    uint64_t distinct_bytes = status.distinct_pages_delta * zx_system_get_page_size();
    std::cout << std::left << std::setw(5) << status.thrasher_type << " " << std::fixed
              << std::setprecision(1) << static_cast<double>(status.total_time.to_msecs()) / 1000.0
              << "s"
              << " | Touches: " << std::right << std::setw(8) << status.touches_delta
              << " | Distinct: " << std::setw(8) << status.distinct_pages_delta << " ("
              << FormatMemorySize(distinct_bytes) << ")" << std::endl;
  });

  std::atomic<int> pending_inits = static_cast<int>(thrashers.size());
  std::atomic<bool> init_failed = false;

  std::stringstream init_summary;
  init_summary << "Initializing ";
  bool first = true;
  if (thrashers_set.count("anon")) {
    init_summary << (first ? "" : "and ") << anon_size_mb << " MiB of anonymous memory";
    first = false;
  }
  if (thrashers_set.count("blob")) {
    init_summary << (first ? "" : " and ") << blob_size_mb << " MiB of blobs";
    first = false;
  }
  if (thrashers_set.count("file")) {
    init_summary << (first ? "" : " and ") << "file " << file_path;
    first = false;
  }
  if (thrashers_set.count("dir")) {
    init_summary << (first ? "" : " and ") << "directory " << dir_path;
    first = false;
  }
  init_summary << "...";
  std::cout << init_summary.str() << std::flush;

  async::TaskClosure dot_printer;
  std::function<void()> print_dot = [&]() {
    std::cout << "." << std::flush;
    dot_printer.PostDelayed(loop.dispatcher(), zx::sec(1));
  };
  dot_printer.set_handler(print_dot);
  dot_printer.PostDelayed(loop.dispatcher(), zx::sec(1));

  for (auto& thrasher : thrashers) {
    thrasher->Initialize([&](zx_status_t status) {
      if (status != ZX_OK) {
        std::cerr << "[THRASHER] Initialization failed with status: "
                  << zx_status_get_string(status) << std::endl;
        init_failed.store(true);
      }
      if (--pending_inits == 0) {
        dot_printer.Cancel();
        std::cout << " Done" << std::endl;
        if (init_failed.load()) {
          std::cerr << "One or more thrashers failed to initialize. Exiting." << std::endl;
          loop.Quit();
        } else {
          std::cout << "Thrashing for " << config.run_for_seconds << "s in " << config.num_threads
                    << " thread(s), each with " << config.bursts_per_second
                    << " bursts per second containing up to " << config.consecutive_pages_per_read
                    << " consecutive pages." << std::endl;
          for (auto& t : thrashers) {
            t->Start(thrash_callback, status_callback);
          }
        }
      }
    });
  }

  loop.Run();

  if (init_failed.load()) {
    return 1;
  }

  if (config.verbose) {
    std::cout << "All thrashers done, logging VMOs..." << std::endl;
    LogVmos(collected_vmos, config.verbose);
  }

  return 0;
}
