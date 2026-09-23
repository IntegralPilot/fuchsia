// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/feedback_data/system_log_recorder/controller.h"

namespace forensics {
namespace feedback_data {
namespace system_log_recorder {

void Controller::SetStop(::fit::closure stop) { stop_ = std::move(stop); }

void Controller::Stop() { stop_(); }

}  // namespace system_log_recorder
}  // namespace feedback_data
}  // namespace forensics
