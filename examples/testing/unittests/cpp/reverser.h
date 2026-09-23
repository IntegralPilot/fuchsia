// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef EXAMPLES_TESTING_UNITTESTS_CPP_REVERSER_H_
#define EXAMPLES_TESTING_UNITTESTS_CPP_REVERSER_H_

#include <string>
#include <string_view>
namespace reverser {
/// Return the input string with its characters reversed.
std::string reverse_string(std::string_view input);
}  // namespace reverser

#endif  // EXAMPLES_TESTING_UNITTESTS_CPP_REVERSER_H_
