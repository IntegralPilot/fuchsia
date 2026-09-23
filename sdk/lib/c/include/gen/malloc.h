//===-- BSD / GNU header <malloc.h> --===//
//
// Part of the LLVM Project, under the Apache License v2.0 with LLVM Exceptions.
// See https://llvm.org/LICENSE.txt for license information.
// SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
//
//===---------------------------------------------------------------------===//

#ifndef _LLVM_LIBC_MALLOC_H
#define _LLVM_LIBC_MALLOC_H

#include "__llvm-libc-common.h"
#include "llvm-libc-macros/malloc-macros.h"
#include "llvm-libc-types/size_t.h"

__BEGIN_C_DECLS

void *aligned_alloc(size_t, size_t) __NOEXCEPT;

void *calloc(size_t, size_t) __NOEXCEPT;

void free(void *) __NOEXCEPT;

void free_aligned_sized(void *, size_t, size_t) __NOEXCEPT;

void free_sized(void *, size_t) __NOEXCEPT;

void *malloc(size_t) __NOEXCEPT;

size_t malloc_usable_size(void *) __NOEXCEPT;

int mallopt(int, int) __NOEXCEPT;

void *memalign(size_t, size_t) __NOEXCEPT;

void *pvalloc(size_t) __NOEXCEPT;

void *realloc(void *, size_t) __NOEXCEPT;

void *reallocarray(void *, size_t, size_t) __NOEXCEPT;

void *valloc(size_t) __NOEXCEPT;

__END_C_DECLS

#endif // _LLVM_LIBC_MALLOC_H
