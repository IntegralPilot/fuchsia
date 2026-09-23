// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_TEST_SANITIZER_SANITIZER_MEMORY_SNAPSHOT_TEST_DSO_H_
#define LIB_C_TEST_SANITIZER_SANITIZER_MEMORY_SNAPSHOT_TEST_DSO_H_

extern "C" {

void* NeededDsoDataPointer();
void* NeededDsoBssPointer();
const void* NeededDsoRodataPointer();
const void* NeededDsoRelroPointer();
void* NeededDsoThreadLocalDataPointer();
void* NeededDsoThreadLocalBssPointer();

const void* DlopenDsoDataPointer();
const void* DlopenDsoBssPointer();
const void* DlopenDsoRodataPointer();
const void* DlopenDsoRelroPointer();
const void* DlopenDsoThreadLocalDataPointer();
const void* DlopenDsoThreadLocalBssPointer();

}  // extern "C"

#endif  // LIB_C_TEST_SANITIZER_SANITIZER_MEMORY_SNAPSHOT_TEST_DSO_H_
