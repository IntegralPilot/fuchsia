// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Must NOT receive CPPFLAG
#if defined(CPPFLAG)
#error "Unexpected CPPFLAG definition"
#endif

int main() { return 0; }
