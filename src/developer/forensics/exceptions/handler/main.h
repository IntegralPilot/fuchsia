// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_EXCEPTIONS_HANDLER_MAIN_H_
#define SRC_DEVELOPER_FORENSICS_EXCEPTIONS_HANDLER_MAIN_H_

#include <string>

namespace forensics {
namespace exceptions {
namespace handler {

int main(const std::string& process_name, const std::string& suspend_enabled_flag);

}  // namespace handler
}  // namespace exceptions
}  // namespace forensics

#endif  // SRC_DEVELOPER_FORENSICS_EXCEPTIONS_HANDLER_MAIN_H_
