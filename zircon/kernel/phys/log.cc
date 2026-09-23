// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "log.h"

#include <lib/boot-options/boot-options.h>

#include <phys/address-space.h>

Log* gLog;

int Log::Write(ktl::string_view str) {
  // Always write to the live console first.
  if (mirror_) {
    mirror_.Write(str);
  }

  AppendToLog(str);

  return static_cast<int>(str.size());
}

void Log::SetStdout() {
  ZX_ASSERT(*stdout != FILE{this});
  ZX_ASSERT(!mirror_);
  mirror_ = *stdout;
  *stdout = FILE{this};
}

void Log::RestoreStdout() {
  if (*stdout == FILE{this}) {
    *stdout = mirror_;
    mirror_ = FILE{};
  }
}

void Log::AppendToLog(ktl::string_view str) {
  // Use the remaining space in the existing buffer, or make more space.
  auto make_space = [this](size_t needed) -> ktl::span<char> {
    if (ktl::span space = buffer_chars_left(); space.size() >= needed) {
      return space;
    }

    // Expand (or initially allocate) the buffer if it's too small.
    // The buffer is always allocated in whole pages.
    const size_t expand_size = (needed + AddressSpace::kPageSize - 1) & -AddressSpace::kPageSize;
    fbl::AllocChecker ac;
    if (buffer_) {
      buffer_.Resize(ac, buffer_.size_bytes() + expand_size);
    } else {
      buffer_ = Allocation::New(ac, memalloc::Type::kPhysLog, expand_size, AddressSpace::kPageSize);
    }
    if (!ac.check()) {
      RestoreStdout();
      ZX_PANIC("failed to increase phys log from %#zx to %#zx bytes", buffer_.size_bytes(),
               buffer_.size_bytes() + expand_size);
    }
    return buffer_chars_left();
  };

  ktl::span<char> space = make_space(str.size());
  size_t copied = str.copy(space.data(), space.size());
  ZX_DEBUG_ASSERT(copied == str.size());
  size_ += copied;
}

FILE Log::LogOnlyFile() {
  return FILE{[](void* log, const char* str, size_t len) -> int {
                static_cast<Log*>(log)->AppendToLog({str, len});
                return static_cast<int>(len);
              },
              this};
}

FILE Log::VerboseOnlyFile() {
  if (BootOptions::Get() && !BootOptions::Get()->phys_verbose) {
    return LogOnlyFile();
  }
  return FILE{this};
}
