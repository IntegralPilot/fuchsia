// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/uio.h>
#include <unistd.h>

// The miscellaneous code that is statically linked into the dynamically linker
// uses the standard system headers and public C symbols for system calls.
// Since it won't be linked with any libc, we provide here the ones that are
// actually used.

namespace {

// Get the sys_* functions defined as local inlines.

int gStartupErrno;
#define SYS_ERRNO gStartupErrno

#define SYS_INLINE inline

#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wshorten-64-to-32"

#include "linux_syscall_support.h"

#pragma GCC diagnostic pop

}  // namespace

// Library code will use the system <errno.h>, which defines errno using this
// function. So define the function here to use the local errno storage.  The
// LSS code stores into gStartupErrno directly.
int* __errno_location() { return &gStartupErrno; }

_Noreturn void _exit(int status) {
  sys_exit_group(status);
  __builtin_trap();
}

int open(const char* filename, int oflag, ...) {
  // No O_CREAT cases are used, so no need to decode the optional argument.
  return sys_open(filename, oflag, 0);
}

int close(int fd) { return sys_close(fd); }

ssize_t read(int fd, void* buf, size_t n) { return sys_read(fd, buf, n); }

ssize_t write(int fd, const void* buf, size_t n) { return sys_write(fd, buf, n); }

ssize_t writev(int fd, const iovec* iov, int n) {
  static_assert(sizeof(iovec) == sizeof(kernel_iovec));
  return sys_writev(fd, reinterpret_cast<const kernel_iovec*>(iov), n);
}

ssize_t pread(int fd, void* buf, size_t n, off_t pos) { return sys_pread64(fd, buf, n, pos); }

void* mmap(void* addr, size_t len, int prot, int flags, int fd, off_t pos) {
  return sys_mmap(addr, len, prot, flags, fd, pos);
}

int mprotect(void* addr, size_t len, int prot) { return sys_mprotect(addr, len, prot); }

int munmap(void* addr, size_t len) { return sys_munmap(addr, len); }

// TODO(https://fxbug.dev/42141211): The llvm-libc implementation would be
// almost fine, but needs to be compiled in a way where it won't try to use a
// thread_local variable.  Some special plumbing could achieve that, maybe do
// it later.
char* strerror(int errnum) {
  static char buffer[32];
  snprintf(buffer, sizeof(buffer), "errno %d", errnum);
  return buffer;
}
