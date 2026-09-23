// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdio.h>

void CallOther();
void UnusedFunction();

int main() {
  printf("Hello\n");
  CallOther();
  return 1;
}
