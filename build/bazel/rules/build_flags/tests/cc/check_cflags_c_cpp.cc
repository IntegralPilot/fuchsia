// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Must NOT receive CONLYFLAG
#if defined(CONLYFLAG)
#error "Unexpected CONLYFLAG definition"
#endif

int main() { return 0; }
