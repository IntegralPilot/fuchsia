// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Must NOT receive CFLAGS
#if defined(CFLAGS)
#error "Unexpected CFLAGS definition"
#endif

// Must NOT receive CPPFLAG
#if defined(CPPFLAG)
#error "Unexpected CPPFLAG definition"
#endif

// Must NOT receive CONLYFLAG
#if defined(CONLYFLAG)
#error "Unexpected CONLYFLAG definition"
#endif

// Must NOT receive A_MACRO
#if defined(A_MACRO)
#error "Unexpected A_MACRO definition"
#endif

int main() { return 0; }
