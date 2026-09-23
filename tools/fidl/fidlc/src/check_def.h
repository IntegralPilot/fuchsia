// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_CHECK_DEF_H_
#define TOOLS_FIDL_FIDLC_SRC_CHECK_DEF_H_

#include <string>
#include <utility>

#include "tools/fidl/fidlc/src/template_string.h"

namespace fidlc {

// Each CheckDef corresponds to some associated linting logic that verifies code
// meets or fails to meet a FIDL Readability requirement.
class CheckDef {
 public:
  // A check includes an ID (in kebab-case) and a string message or
  // message_template (with optional placeholders for customizing the message,
  // if any). The check logic (code) is external to this class.
  CheckDef(std::string_view id, TemplateString message_template)
      : id_(id), message_template_(std::move(message_template)) {}

  inline std::string_view id() const { return id_; }

  inline const TemplateString& message_template() const { return message_template_; }

  // A |std::set| of |CheckDef| will sort by |id|.
  inline bool operator<(const CheckDef& rhs) const { return id_ < rhs.id_; }

  // A |std::set| of |CheckDef| will not insert a CheckDef if it does not
  // have a unique |id|.
  inline bool operator==(const CheckDef& rhs) { return id_ == rhs.id_; }

 private:
  std::string id_;  // dash-separated (kebab-case), and URL suffixable
  TemplateString message_template_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_CHECK_DEF_H_
