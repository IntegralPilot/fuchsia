// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

// The compiler headers included by <lib/arch/intrin.h> include <mm_malloc.h>
// to declare some functions never needed in practice.  The compilers' versions
// of that header do some problematic declarations.  So this file exists just
// to preempt the compiler header and do nothing.
