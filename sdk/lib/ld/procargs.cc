// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/processargs.h>
#include <lib/zircon-internal/unique-backtrace.h>

#include <string_view>

#include "zircon.h"

namespace ld {
namespace {

using namespace std::literals;

constexpr std::string_view kLdDebugPrefix = "\0LD_DEBUG="sv;
constexpr std::string_view kLdDebugPrefixFirst = kLdDebugPrefix.substr(1);

constexpr bool HasLdDebug(std::string_view env) {
  std::string_view debug;
  if (env.starts_with(kLdDebugPrefixFirst)) {
    debug = env.substr(kLdDebugPrefixFirst.size());
  } else if (size_t found = env.find(kLdDebugPrefix); found != std::string_view::npos) {
    debug = env.substr(found + kLdDebugPrefix.size());
  }
  return !debug.empty() && debug.front() != '\0';
}

}  // namespace

StartupData ReadBootstrap(zx::unowned_channel bootstrap) {
  StartupData startup;

  ProcessargsBuffer<> message;
  ProcessargsBuffer<>::HandlesBuffer handles_buffer;
  zx::result read = message.Read(bootstrap->borrow(), handles_buffer);
  if (read.is_error()) [[unlikely]] {
    CRASH_WITH_UNIQUE_BACKTRACE();
  }
  std::span handles = std::span{handles_buffer}.subspan(0, read->handles);
  const std::span handle_info = message.handle_info(read->handles);

  if (!message.Valid(*read)) [[unlikely]] {
    CRASH_WITH_UNIQUE_BACKTRACE();
  }

  for (uint32_t i = 0; i < handles.size(); ++i) {
    // If not otherwise consumed below, the handle will be closed.
    zx::handle handle{std::exchange(handles[i], {})};
    switch (PA_HND_TYPE(handle_info[i])) {
      case PA_VMAR_ROOT:
        startup.vmar.reset(handle.release());
        break;

      case PA_VMAR_LOADED:
        startup.self_vmar.reset(handle.release());
        break;

      case PA_VMO_EXECUTABLE:
        startup.executable_vmo.reset(handle.release());
        break;

      case PA_FD:
        if (ld::IsProcessargsLogHandle(handle_info[i])) {
          startup.log.TakeLogHandle(std::move(handle));
        }
        break;

      case PA_LDSVC_LOADER:
        startup.ldsvc.reset(handle.release());
        break;

      default:  // Other handles are not interesting and get dropped.
        break;
    }
  }

  // The only part of the strings of interest is the environment, and only to
  // search it for LD_DEBUG rather than finding all the individual strings.
  startup.ld_debug = HasLdDebug(message.environ_chars(read->bytes));

  return startup;
}

}  // namespace ld
