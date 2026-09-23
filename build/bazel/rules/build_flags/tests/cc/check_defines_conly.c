// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Must have received A_MACRO
#if !defined(A_MACRO)
#error "Missing macro definition for A_MACRO"
#elif A_MACRO != 42
#error "Invalid macro definition for A_MACRO"
#endif

int main() { return 0; }
