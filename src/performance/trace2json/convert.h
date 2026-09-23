// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_TRACE2JSON_CONVERT_H_
#define SRC_PERFORMANCE_TRACE2JSON_CONVERT_H_

#include <string>
#include <vector>

struct ConvertSettings {
  std::string input_file_name;
  std::string output_file_name;
  std::string system_event_output_file_name;
  std::vector<std::string> patterns;
  std::vector<std::string> categories;
};

bool ConvertTrace(const ConvertSettings& settings);

#endif  // SRC_PERFORMANCE_TRACE2JSON_CONVERT_H_
