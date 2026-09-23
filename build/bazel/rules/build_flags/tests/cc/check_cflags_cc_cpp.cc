// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Must have received CPPFLAG
#if !defined(CPPFLAG)
#error "Missing CPPFLAG definition"
#elif CPPFLAG != 1
#error "Invalid CPPFLAG definition"
#endif

int main() { return 0; }
