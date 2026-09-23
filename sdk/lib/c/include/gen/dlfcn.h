//===-- POSIX header <dlfcn.h> --===//
//
// Part of the LLVM Project, under the Apache License v2.0 with LLVM Exceptions.
// See https://llvm.org/LICENSE.txt for license information.
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
//===---------------------------------------------------------------------===//

#ifndef _LLVM_LIBC_DLFCN_H
#define _LLVM_LIBC_DLFCN_H

#include "__llvm-libc-common.h"
#include "llvm-libc-types/Dl_info.h"

#define RTLD_BINDING_MASK 0x00003

#define RTLD_DEEPBIND 0x00008

#define RTLD_DEFAULT ((void *) 0)

#define RTLD_GLOBAL 0x00100

#define RTLD_LAZY 0x00001

#define RTLD_LOCAL 0

#define RTLD_NEXT ((void *) -1l)

#define RTLD_NODELETE 0x01000

#define RTLD_NOLOAD 0x00004

#define RTLD_NOW 0x00002

enum {
  RTLD_DI_LMID = 1,
  RTLD_DI_LINKMAP = 2,
  RTLD_DI_CONFIGADDR = 3,
  RTLD_DI_SERINFO = 4,
  RTLD_DI_SERINFOSIZE = 5,
  RTLD_DI_ORIGIN = 6,
  RTLD_DI_PROFILENAME = 7,
  RTLD_DI_PROFILEOUT = 8,
  RTLD_DI_TLS_MODID = 9,
  RTLD_DI_TLS_DATA = 10,
  RTLD_DI_PHDR = 11,
  RTLD_DI_MAX = 11,
};

__BEGIN_C_DECLS

int dladdr(const void *__restrict, Dl_info *__restrict) __NOEXCEPT;

int dlclose(void *) __NOEXCEPT;

char *dlerror(void) __NOEXCEPT;

int dlinfo(void *__restrict, int, void *__restrict) __NOEXCEPT;

void *dlopen(const char *, int) __NOEXCEPT;

void *dlsym(void *__restrict, const char *__restrict) __NOEXCEPT;

__END_C_DECLS

#endif // _LLVM_LIBC_DLFCN_H
