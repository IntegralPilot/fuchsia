// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PREEMPT_SRC_SETJMP_SETJMP_IMPL_H_
#define PREEMPT_SRC_SETJMP_SETJMP_IMPL_H_

// The llvm-libc header has a nonstandard name for reasons that don't apply in
// the Fuchsia build and might get fixed upstream.  In the usual schema, this
// file would be used as `#include "src/setjmp/setjmp.h"` so a companion here
// by that name redirects here.  In future, this file and the llvm-libc file
// should ideally be renamed to "src/setjmp/setjmp.h".

#include "asm-linkage.h"

// The llvm-libc header provides the namespaced declaration.
#include_next "src/setjmp/setjmp_impl.h"  // IWYU pragma: export

namespace LIBC_NAMESPACE_DECL {

// Redeclare it to apply the custom linkage name.
decltype(setjmp) setjmp LIBC_ASM_LINKAGE_DECLARE(setjmp);

// Declare namespaced aliases for the other public aliases, for uniformity.
decltype(setjmp) _setjmp LIBC_ASM_LINKAGE_DECLARE(_setjmp);
decltype(setjmp) sigsetjmp LIBC_ASM_LINKAGE_DECLARE(sigsetjmp);

}  // namespace LIBC_NAMESPACE_DECL

#endif  // PREEMPT_SRC_SETJMP_SETJMP_IMPL_H_
