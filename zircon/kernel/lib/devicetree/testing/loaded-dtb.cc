// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/devicetree/testing/loaded-dtb.h"

#include <lib/fit/defer.h>

#include <filesystem>
#include <string_view>

#ifndef __Fuchsia__
#include <libgen.h>
#include <unistd.h>
#endif  // !__Fuchsia__

#ifdef __APPLE__
#include <mach-o/dyld.h>
#endif

namespace devicetree::testing {
namespace {
// TODO: make the non-fuchsia part of this a general utility that can be shared
// across host-side tests.
std::filesystem::path GetResourcePath() {
  std::filesystem::path path;
#if defined(__Fuchsia__)
  path.append("/pkg");
#elif defined(__APPLE__)
  uint32_t length = PATH_MAX;
  char self_path[PATH_MAX];
  char self_path_symlink[PATH_MAX];
  _NSGetExecutablePath(self_path_symlink, &length);
  const char* bin_dir = dirname(realpath(self_path_symlink, self_path));
  path.append(bin_dir);
#elif defined(__linux__)
  char self_path[PATH_MAX];
  const char* bin_dir = dirname(realpath("/proc/self/exe", self_path));
  path.append(bin_dir);
#else
#error unknown platform.
#endif
  return path;
}

}  // namespace

fit::result<std::string, LoadedDtb> LoadDtb(std::string_view dtb_file) {
  std::filesystem::path resource_path = GetResourcePath().append(LIB_DEVICETREE_DATA).string();
  resource_path.append(dtb_file);
  FILE* file = fopen(resource_path.c_str(), "r");
  if (!file) {
    std::string err =
        "Failed to open: " + std::string(resource_path.c_str()) + ": " + strerror(errno);
    return fit::error(err);
  }
  auto close_file = fit::defer([file]() { fclose(file); });

  if (fseek(file, 0, SEEK_END) != 0) {
    std::string err = "Failed to seek to end of file : " + std::string(resource_path.c_str()) +
                      ": " + strerror(errno);
    return fit::error(err);
  }

  auto size = static_cast<size_t>(ftell(file));
  LoadedDtb ldtb;

  ldtb.dtb.resize(size);
  rewind(file);

  if (auto actual = fread(reinterpret_cast<char*>(ldtb.dtb.data()), 1, size, file);
      actual != size) {
    std::string err = "Failed to read file : " + std::string(resource_path.c_str()) + ". Read " +
                      std::to_string(actual) + " bytes of " + std::to_string(size);
    return fit::error(err);
  }

  return fit::success(ldtb);
}

}  // namespace devicetree::testing
