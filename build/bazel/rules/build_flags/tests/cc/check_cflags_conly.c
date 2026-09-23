// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Must have received CFLAGS
#if !defined(CFLAGS)
#error "Missing CFLAGS definition"
#elif CFLAGS != 1
#error "Invalid CFLAGS definition"
#endif

int main() { return 0; }
