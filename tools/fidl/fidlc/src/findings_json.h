// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_SRC_FINDINGS_JSON_H_
#define TOOLS_FIDL_FIDLC_SRC_FINDINGS_JSON_H_

#include <sstream>
#include <string>

#include "tools/fidl/fidlc/src/findings.h"
#include "tools/fidl/fidlc/src/json_writer.h"
#include "tools/fidl/fidlc/src/source_span.h"

namespace fidlc {

// |JsonWriter| requires the derived type as a template parameter so it can
// match methods declared with parameter overrides in the derived class.
class FindingsJson : public JsonWriter<FindingsJson> {
 public:
  // "using" is required for overridden methods, so the implementations in
  // both the base class and in this derived class are visible when matching
  // parameter types
  using JsonWriter<FindingsJson>::Generate;
  using JsonWriter<FindingsJson>::GenerateArray;

  // Suggested replacement string and span, per the JSON schema used by
  // Tricium for a findings/diagnostics
  struct Replacement {
    const SourceSpan& span;  // From the Finding
    std::string replacement;
  };

  struct SuggestionWithReplacementSpan {
    const SourceSpan& span;  // From the Finding
    Suggestion suggestion;
  };

  explicit FindingsJson(const Findings& findings) : JsonWriter(json_file_), findings_(findings) {}

  ~FindingsJson() = default;

  std::ostringstream Produce();

  void Generate(const Finding& finding);
  void Generate(const SuggestionWithReplacementSpan& suggestion_with_span);
  void Generate(const Replacement& replacement);
  void Generate(const SourceSpan& span);

 private:
  const Findings& findings_;
  std::ostringstream json_file_;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_SRC_FINDINGS_JSON_H_
