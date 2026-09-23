// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dlfcn.h>
#include <fcntl.h>
#include <getopt.h>
#include <sys/mman.h>
#include <unistd.h>
#include <zircon/assert.h>

#include <cinttypes>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <locale>
#include <optional>
#include <span>
#include <string_view>
#include <thread>
#include <vector>

#include <fbl/unique_fd.h>

#ifdef __Fuchsia__
#include <lib/zx/exception.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/exception.h>
#include <zircon/threads.h>
#endif

#include "test-child-dso.h"

namespace {

constexpr std::string_view kStdinoutFilename = "-";

constexpr char kOptString[] = "c:e:m:M:o:p:t:w:x:dDC:";
constexpr option kLongOpts[] = {
    {"cat-from", required_argument, nullptr, 'c'},      //
    {"cat-to", required_argument, nullptr, 'o'},        //
    {"echo", required_argument, nullptr, 'e'},          //
    {"memory", required_argument, nullptr, 'm'},        //
    {"memory-ints", required_argument, nullptr, 'M'},   //
    {"memory-pages", required_argument, nullptr, 'p'},  //
    {"threads", required_argument, nullptr, 't'},       //
    {"memory-wchar", required_argument, nullptr, 'w'},  //
    {"dladdr-main", no_argument, nullptr, 'd'},         //
    {"dladdr-dso", no_argument, nullptr, 'D'},          //
    {"exit", required_argument, nullptr, 'x'},          //
    {"crash", required_argument, nullptr, 'C'},         //
};

int Usage() {
  fprintf(stderr,
          "Usage: test-child [--echo=STRING] [--memory=STRING] [--memory-ints=INT,...] "
          "[--cat-from=FILE] [--cat-to=FILE] "
          "[--threads=N]\n");
  return 1;
}

#ifdef __Fuchsia__
void PrintKoid(thrd_t thread) {
  zx::unowned_thread handle(thrd_get_zx_handle(thread));
  zx_info_handle_basic_t info;
  zx_status_t status =
      handle->get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  if (status != ZX_OK) {
    fprintf(stderr, "ZX_INFO_HANDLE_BASIC (%#x) %s\n", handle->get(), zx_status_get_string(status));
    exit(1);
  }
  printf("%" PRIu64 "\n", info.koid);
}
#endif

[[noreturn]] void Hang() {
  while (true) {
#ifdef __Fuchsia__
    zx_thread_legacy_yield(0);
#else
    pause();
#endif
  }
}

void Cat(fbl::unique_fd from, int to) {
  char buf[BUFSIZ];
  ssize_t nread;
  while ((nread = read(from.get(), buf, sizeof(buf))) > 0) {
    std::span<const char> chunk(buf, static_cast<size_t>(nread));
    while (!chunk.empty()) {
      ssize_t nwrote = write(to, chunk.data(), chunk.size());
      if (nwrote < 0) {
        perror("write");
        exit(2);
      }
      chunk = chunk.subspan(static_cast<size_t>(nwrote));
    }
  }
  if (nread < 0) {
    perror("read");
    exit(2);
  }
}

fbl::unique_fd CatOpen(const char* filename, int stdfd, int oflags) {
  fbl::unique_fd fd{filename == kStdinoutFilename ? stdfd : open(filename, oflags, 0666)};
  if (!fd) {
    perror(filename);
    exit(2);
  }
  return fd;
}

void CatFrom(const char* filename) {
  Cat(CatOpen(filename, STDIN_FILENO, O_RDONLY), STDOUT_FILENO);
}

void CatTo(const char* filename) {
  Cat(fbl::unique_fd{STDIN_FILENO},
      CatOpen(filename, STDOUT_FILENO, O_WRONLY | O_CREAT | O_EXCL).get());
}

template <typename T>
void PrintDladdr(T ptr) {
  void* address = reinterpret_cast<void*>(reinterpret_cast<uintptr_t>(ptr));
  Dl_info info;
  ZX_ASSERT_MSG(dladdr(address, &info) != 0, "dladdr failed on %p", address);
  printf("%p\n", info.dli_fbase);
}

// Crash with the given value in a known register.
[[noreturn]] void CrashWithRegisterValue(uint64_t value) {
#if defined(__aarch64__)
  __asm__(
      "mov x0, %0\n"
      "udf #0"
      :
      : "r"(value)
      : "x0");
#elif defined(__riscv)
  __asm__(
      "mv a0, %0\n"
      "unimp"
      :
      : "r"(value)
      : "a0");
#elif defined(__x86_64__)
  __asm__("ud2" : : "a"(value));
#endif
  __builtin_trap();
}

}  // namespace

int main(int argc, char** argv) {
  size_t thread_count = 0;
  std::vector<int> ints;
  std::wstring wstr;
  std::optional<uint64_t> crash_register;

#ifdef __Fuchsia__
  // This doesn't do anything, but calling it ensures that the test-child-dso
  // shared library is linked in.
  TestDsoFunction();
#endif

  while (true) {
    switch (getopt_long(argc, argv, kOptString, kLongOpts, nullptr)) {
      case -1:
        // This ends the loop.  All other cases continue (or return).
        break;

      case 'c':
        CatFrom(optarg);
        continue;

      case 'o':
        CatTo(optarg);
        continue;

      case 'e':
        puts(optarg);
        continue;

      case 'm':
        printf("%p\n", optarg);
        continue;

      case 'M': {
        std::string optstring = optarg;
        char* rest = optstring.data();
        char* p;
        while ((p = strsep(&rest, ",")) != nullptr) {
          ints.push_back(atoi(p));
        }
        printf("%p\n", ints.data());
        continue;
      }

#ifdef __Fuchsia__
      case 'p': {
        size_t allocated_size, reserved_size;
        ZX_ASSERT(sscanf(optarg, "%zu,%zu", &allocated_size, &reserved_size) == 2);

        size_t mapped_size = allocated_size + reserved_size;
        ZX_ASSERT(mapped_size > 0);

        zx::vmo vmo;
        zx_status_t status = zx::vmo::create(allocated_size, 0, &vmo);
        ZX_ASSERT_MSG(status == ZX_OK, "zx_vmo_create: %s", zx_status_get_string(status));

        zx_vaddr_t vaddr;
        status = zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE | ZX_VM_ALLOW_FAULTS,
                                            0, vmo, 0, mapped_size, &vaddr);
        ZX_ASSERT_MSG(status == ZX_OK, "zx_vmar_map: %s", zx_status_get_string(status));

        uint8_t* mapped = reinterpret_cast<uint8_t*>(vaddr);
        for (size_t i = 0; i < allocated_size; ++i) {
          mapped[i] = static_cast<uint8_t>(i);
        }

        printf("%p\n", mapped);
        continue;
      }
#endif

      case 't':
        thread_count = atoi(optarg);
        continue;

      case 'w': {
        std::string_view byte_string = optarg;
        wstr.resize(byte_string.size());
        wstr.resize(mbstowcs(wstr.data(), byte_string.data(), wstr.size()));
        ZX_ASSERT(wstr.size() == byte_string.size());
        printf("%p\n", wstr.data());
        continue;
      }

      case 'd':
        PrintDladdr(&main);
        continue;

#ifdef __Fuchsia__
      case 'D':
        PrintDladdr(&TestDsoFunction);
        continue;
#endif

      case 'x':
        return atoi(optarg);

      case 'C':
        crash_register = strtoull(optarg, nullptr, 0);
        break;

      default:
        return Usage();
    }
    break;
  }
  if (optind != argc) {
    return Usage();
  }

#ifdef __Fuchsia__
  // Register for process exceptions.
  zx::channel exception_channel;
  if (crash_register) {
    zx_status_t status = zx::process::self()->create_exception_channel(0, &exception_channel);
    ZX_ASSERT_MSG(status == ZX_OK, "zx_task_create_exception_channel: %s",
                  zx_status_get_string(status));
  }
#endif

  std::vector<std::thread> threads(thread_count);
  for (std::thread& thread : threads) {
    if (crash_register) {
      thread = std::thread(CrashWithRegisterValue, *crash_register);
    } else {
      thread = std::thread(Hang);
    }
  }

#ifdef __Fuchsia__
  // Wait for all the crashing threads to start up and actually crash.  Then
  // keep the exception handles alive so the threads stay suspended.
  std::vector<zx::exception> thread_exceptions;
  std::vector<zx::suspend_token> thread_suspensions;  // TODO(https://fxbug.dev/42071974): see below
  if (exception_channel) {
    while (thread_exceptions.size() < threads.size()) {
      zx_exception_info_t info;
      zx_signals_t pending;
      zx_status_t status =
          exception_channel.wait_one(ZX_CHANNEL_READABLE, zx::time::infinite(), &pending);
      ZX_ASSERT_MSG(status == ZX_OK, "wait on exception channel: %s", zx_status_get_string(status));
      uint32_t nbytes, nhandles;
      thread_exceptions.emplace_back();
      status = exception_channel.read(0, &info, thread_exceptions.back().reset_and_get_address(),
                                      sizeof(info), 1, &nbytes, &nhandles);
      ZX_ASSERT_MSG(status == ZX_OK, "read on exception channel: %s", zx_status_get_string(status));
      ZX_ASSERT(nbytes == sizeof(info));
      ZX_ASSERT(nhandles == 1);

      // TODO(https://fxbug.dev/42071974): That should work, but actually the kernel
      // doesn't report a thread as suspended until after exception processing
      // completes.  Until that is fixed in the kernel, we need to do another
      // little dance here: we suspend each thread, then let it resume from the
      // exception so it would retry the faulting instruction; but we then wait
      // for it to instead report that it's now suspended, and keep it that way
      // so that when the dumper in the parent test process comes along and
      // suspends it, it won't be blocked waiting for the suspension to be
      // completable. Note we don't have to synchronize with the suspension
      // here being reported as completed, because it's enough to know that the
      // exception resumption is in progress and the suspension will complete
      // soon. Our suspension here doesn't interfere with the dumper's
      // suspension like the exception does, and we've already synchronized
      // that the thread hit the exception so it will be in the right register
      // state when the dumper's suspension completes.
      zx::thread thread;
      status = thread_exceptions.back().get_thread(&thread);
      ZX_ASSERT_MSG(status == ZX_OK, "zx_exception_get_thread: %s", zx_status_get_string(status));
      thread_suspensions.emplace_back();
      status = thread.suspend(&thread_suspensions.back());
      ZX_ASSERT_MSG(status == ZX_OK, "zx_task_suspend: %s", zx_status_get_string(status));
      constexpr uint32_t kHandled = ZX_EXCEPTION_STATE_HANDLED;
      status = thread_exceptions.back().set_property(ZX_PROP_EXCEPTION_STATE, &kHandled,
                                                     sizeof(kHandled));
      ZX_ASSERT_MSG(status == ZX_OK, "zx_object_set_property on exception object: %s",
                    zx_status_get_string(status));
      // This lets the thread resume, and immediately process its suspension.
      // Now the thread_suspensions.back() handle will keep it suspended.
      thread_exceptions.back().reset();
    }
  }
#endif

  if (thread_count > 0) {
#ifdef __Fuchsia__
    PrintKoid(thrd_current());
    for (std::thread& thread : threads) {
      PrintKoid(thread.native_handle());
    }
#endif
  }

  fflush(stdout);

  Hang();

  return 0;
}
