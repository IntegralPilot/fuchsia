// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_PROGRAM_SHORTNAME_H_
#define SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_PROGRAM_SHORTNAME_H_

#include <optional>
#include <string>

namespace forensics::crash_reports {

class ProgramShortname {
 public:
  // Returns std::nullopt if the shortened program name is empty.
  static std::optional<ProgramShortname> Create(std::string program_name);

  // Extract the component name without the ".cm" suffix from |name|, if one is present.
  //
  // For example `fuchsia-pkg://fuchsia.com/foo-bar#meta/foo_bar.cm` becomes
  // `foo_bar`.
  std::string Logname() const;

  const std::string& Value() const { return value_; }

 private:
  explicit ProgramShortname(std::string value) : value_(std::move(value)) {}

  std::string value_;
};

}  // namespace forensics::crash_reports

#endif  // SRC_DEVELOPER_FORENSICS_CRASH_REPORTS_PROGRAM_SHORTNAME_H_
