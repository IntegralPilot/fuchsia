// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Some of the compiler <*intrin.h> headers included by libc code include
// <mm_malloc.h> to declare some functions never needed in practice.  The
// compilers' versions of that header do some problematic declarations.  So
// this file exists just to preempt the compiler header and do nothing.
