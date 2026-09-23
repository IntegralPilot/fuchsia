// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <unistd.h>

#include <iostream>

int main() {
  std::cout << "launch_input started is running";
  // Run until interrupted, to let Starnix connect to the `GraphicalPresenter`
  // protocol on behalf of this component.
  pause();
  std::cout << "launch_input started is exiting";
}
