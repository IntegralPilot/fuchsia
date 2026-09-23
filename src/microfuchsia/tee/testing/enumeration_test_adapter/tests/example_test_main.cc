// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdlib.h>

#include <string_view>

int main(int argc, char** argv) {
  if (argc != 2) {
    abort();
  }

  if (argv[1] == std::string_view("pass")) {
    return 0;
  } else if (argv[1] == std::string_view("fail")) {
    return 1;
  } else if (argv[1] == std::string_view("crash")) {
    abort();
  }
  return 2;
}
