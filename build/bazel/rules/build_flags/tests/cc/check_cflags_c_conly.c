// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Must have received CONLYFLAG
#if !defined(CONLYFLAG)
#error "Missing CONLYFLAG definition"
#elif CONLYFLAG != 1
#error "Invalid CONLYFLAG definition"
#endif

int main() { return 0; }
