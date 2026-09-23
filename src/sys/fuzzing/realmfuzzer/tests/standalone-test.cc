// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This "test" appears minimal; but is actually used to test building and running an executable that
// has been instrumented for fuzzing and linked against the realmfuzzer target runtime. See BUILD.gn
// for additional details.
int main() { return 0; }
