// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2015 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#include <ctype.h>
#include <debug.h>
#include <lib/debuglog.h>
#include <lib/io.h>
#include <lib/persistent-debuglog.h>
#include <lib/zircon-internal/macros.h>
#include <platform.h>
#include <string.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <arch/ops.h>
#include <kernel/lockdep.h>
#include <kernel/thread.h>
#include <platform/debug.h>
#include <vm/vm.h>

namespace {

enum class SkipPersistedDebuglog { No = 0, Yes };

DECLARE_SINGLETON_SPINLOCK_WITH_TYPE(dputc_spin_lock, MonitoredSpinLock);

}  // namespace

void serial_write(ktl::string_view str) {
  Guard<MonitoredSpinLock, IrqSave> guard{dputc_spin_lock::Get(), SOURCE_TAG};

  // Write out the serial port.
  platform_dputs_irq(str.data(), str.size());
}

static void stdout_write(ktl::string_view str, SkipPersistedDebuglog skip_pdlog) {
  if (skip_pdlog == SkipPersistedDebuglog::No) {
    persistent_dlog_write(str);
  }

  if (dlog_bypass() == false) {
    if (dlog_write(DEBUGLOG_INFO, 0, str) == ZX_OK)
      return;
  }
  serial_write(str);
}

static void stdout_write_buffered(ktl::string_view str, SkipPersistedDebuglog skip_pdlog) {
  Thread* t = Thread::Current::Get();

  if (unlikely(t == nullptr)) {
    stdout_write(str, skip_pdlog);
    return;
  }

  // Look for corruption and don't continue.
  Thread::Linebuffer& lb = t->linebuffer();
  if (unlikely(!is_kernel_address((uintptr_t)lb.buffer.data()) || lb.pos >= lb.buffer.size())) {
    stdout_write("<linebuffer corruption>\n"sv, skip_pdlog);
    return;
  }

  while (!str.empty()) {
    size_t remaining = lb.buffer.size() - lb.pos;
    auto substring = str.substr(0, remaining);
    size_t newline_pos = substring.find_first_of('\n');

    size_t size;
    bool inject;
    bool flush;
    if (newline_pos != substring.npos) {
      // A newline that fits in our remaining buffer.
      size = newline_pos + 1;
      inject = false;
      flush = true;
    } else if (substring.size() == remaining) {
      // We fill the buffer, injecting a newline.
      size = remaining - 1;
      inject = true;
      flush = true;
    } else {
      // We only add to the buffer.
      size = substring.size();
      inject = false;
      flush = false;
    }

    memcpy(&lb.buffer[lb.pos], substring.data(), size);
    str.remove_prefix(size);
    lb.pos += size;

    if (inject) {
      lb.buffer[lb.pos] = '\n';
      lb.pos += 1;
    }
    if (flush) {
      stdout_write({lb.buffer.data(), lb.pos}, skip_pdlog);
      lb.pos = 0;
    }
  }
}

// This is what printf calls.  Really this could and should be const.
// But all the stdio function signatures require non-const `FILE*`.
FILE gStdout{[](ktl::string_view str) {
  stdout_write_buffered(str, SkipPersistedDebuglog::No);
  return static_cast<int>(str.size());
}};

FILE gSerialFile{[](ktl::string_view str) {
  serial_write(str);
  return static_cast<int>(str.size());
}};

FILE gStdoutUnbuffered{[](ktl::string_view str) {
  stdout_write(str, SkipPersistedDebuglog::No);
  return static_cast<int>(str.size());
}};

FILE gStdoutNoPersist{[](ktl::string_view str) {
  stdout_write_buffered(str, SkipPersistedDebuglog::Yes);
  return static_cast<int>(str.size());
}};
