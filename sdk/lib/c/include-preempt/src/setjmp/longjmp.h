// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PREEMPT_SRC_SETJMP_LONGJMP_H_
#define PREEMPT_SRC_SETJMP_LONGJMP_H_

#include "asm-linkage.h"

// The llvm-libc header provides the namespaced declaration.
#include_next "src/setjmp/longjmp.h"

namespace LIBC_NAMESPACE_DECL {

// Redeclare it to apply the custom linkage name.
decltype(longjmp) longjmp LIBC_ASM_LINKAGE_DECLARE(longjmp);

// Declare namespaced aliases for the other public aliases, for uniformity.
decltype(longjmp) _longjmp LIBC_ASM_LINKAGE_DECLARE(_longjmp);
decltype(longjmp) siglongjmp LIBC_ASM_LINKAGE_DECLARE(siglongjmp);

}  // namespace LIBC_NAMESPACE_DECL

#endif  // PREEMPT_SRC_SETJMP_LONGJMP_H_
