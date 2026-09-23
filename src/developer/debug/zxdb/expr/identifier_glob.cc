// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/expr/identifier_glob.h"

#include <optional>

#include "src/developer/debug/zxdb/expr/expr_parser.h"

namespace zxdb {

namespace {

// Matches the template parameters of one component. Return value is the same as
// IdentifierGlob::Matches.
std::optional<int> MatchTemplateParams(const std::vector<std::string>& glob,
                                       const std::vector<std::string>& type) {
  if (type.size() < glob.size())
    return std::nullopt;  // Glob trying to match more parameters than the type has.

  int last_score = 0;
  for (size_t i = 0; i < glob.size(); i++) {
    if (glob[i] == "*") {
      if (i == glob.size() - 1) {
        // Last glob "*" matches all remaining type parameters.
        last_score = static_cast<int>(type.size()) - i;
        break;
      } else {
        last_score = 1;
      }
    } else {
      // Non-wildcards must be an exact match.
      if (glob[i] != type[i])
        return std::nullopt;

      // Last glob template was not a wildcard, and there are still more type params to match.
      if (i == glob.size() - 1 && type.size() > glob.size())
        return std::nullopt;
    }
  }

  return last_score;
}

std::optional<int> MatchNamespaceComponents(const std::vector<ParsedIdentifierComponent>& glob,
                                            const std::vector<ParsedIdentifierComponent>& type) {
  if (glob.back().name() == type.back().name()) {
    return type.size() - 1;
  }

  return std::nullopt;
}

}  // namespace

Err IdentifierGlob::Init(const std::string& glob) { return Init(glob, false); }

Err IdentifierGlob::Init(const std::string& glob, bool match_any_namespace) {
  match_any_namespace_ = match_any_namespace;
  return ExprParser::ParseIdentifier(glob, &parsed_);
}

std::optional<int> IdentifierGlob::Matches(const ParsedIdentifier& type) const {
  if (type.components().empty())
    return std::nullopt;

  // Without any template parameters or qualifications, we match the last component of the actual
  // type on the parsed name.
  if (match_any_namespace_) {
    if (!type.components().back().has_template() && !parsed_.components().back().has_template()) {
      // We should never have |match_any_namespace_| be true for a glob with more than one
      // component.
      FX_DCHECK(parsed_.components().size() == 1);

      return MatchNamespaceComponents(parsed_.components(), type.components());
    }
  }

  if (parsed_.components().size() != type.components().size())
    return std::nullopt;

  int max_component_score = 0;
  for (size_t i = 0; i < type.components().size(); i++) {
    // The name must match exactly.
    if (parsed_.components()[i].name() != type.components()[i].name() ||
        parsed_.components()[i].has_template() != type.components()[i].has_template())
      return std::nullopt;

    if (parsed_.components()[i].has_template()) {
      // Check template parameters.
      if (auto score = MatchTemplateParams(parsed_.components()[i].template_contents(),
                                           type.components()[i].template_contents())) {
        // Want the largest score.
        if (*score > max_component_score)
          max_component_score = *score;
      } else {
        return std::nullopt;  // Template params don't match.
      }
    }
  }

  return max_component_score;
}

}  // namespace zxdb
