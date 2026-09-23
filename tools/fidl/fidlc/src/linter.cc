// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/src/linter.h"

#include <lib/fit/function.h>
#include <zircon/assert.h>

#include <algorithm>
#include <fstream>
#include <set>
#include <sstream>
#include <string_view>
#include <utility>

#include "tools/fidl/fidlc/src/findings.h"
#include "tools/fidl/fidlc/src/raw_ast.h"
#include "tools/fidl/fidlc/src/utils.h"

namespace fidlc {

namespace {

// Special, Zircon FIDL libraries dealing in kernel ABI. These libraries are
// exempt from the general platform library naming policies.
constexpr std::string_view kZirconLibraryZx = "zx";
constexpr std::string_view kZirconLibraryZbi = "zbi";

// Tests whether a check id is considered experimental in this version of the
// linter. Experimental checks only appear if they are explicitly included, even
// if they are not excluded.
constexpr bool IsCheckExperimental(std::string_view check_id) {
  return check_id == "explicit-flexible-method-modifier" ||
         check_id == "explicit-openness-modifier" ||
         // This check does currently highlight some potential issues with
         // formatting and with 2-slash comments that will be converted to
         // 3-slash Doc-Comments, but the rule cannot currently check 3-slash
         // Doc-Comments (they are stripped out before they reach the linter,
         // and converted to Attributes), and trailing non-Doc comments are
         // supposed to be allowed. Therefore, the rule will eventually be
         // removed, once the valid issues it currently surfaces have been
         // addressed.
         check_id == "no-trailing-comment";
}

constexpr bool IsZirconLibrary(std::string_view name) {
  return name == kZirconLibraryZx || name == kZirconLibraryZbi;
}

// Convert the SourceElement (start- and end-tokens within the SourceFile)
// to a std::string_view, spanning from the beginning of the start token, to the end
// of the end token. The three methods support classes derived from
// SourceElement, by reference, pointer, or unique_ptr.
std::string_view to_string_view(const SourceElement& element) { return element.span().data(); }

template <typename SourceElementSubtype>
std::string_view to_string_view(const std::unique_ptr<SourceElementSubtype>& element_ptr) {
  static_assert(std::is_base_of<SourceElement, SourceElementSubtype>::value,
                "template parameter type is not derived from SourceElement");
  return to_string_view(*element_ptr);
}

// Convert the SourceElement to a std::string, using the method described above
// for std::string_view.
std::string to_string(const SourceElement& element) { return std::string(to_string_view(element)); }

template <typename SourceElementSubtype>
std::string to_string(const std::unique_ptr<SourceElementSubtype>& element_ptr) {
  static_assert(std::is_base_of<SourceElement, SourceElementSubtype>::value,
                "template parameter type is not derived from SourceElement");
  return to_string(*element_ptr);
}

std::string name_layout_kind(const RawLayout& layout) {
  switch (layout.kind) {
    case RawLayout::Kind::kBits: {
      return "bitfield";
    }
    case RawLayout::Kind::kEnum: {
      return "enum";
    }
    case RawLayout::Kind::kStruct: {
      return "struct";
    }
    case RawLayout::Kind::kTable: {
      return "table";
    }
    case RawLayout::Kind::kUnion: {
      return "union";
    }
    case RawLayout::Kind::kOverlay: {
      return "overlay";
    }
  }
}

// Checks if the given modifier type is included. Note: this pays no attention
// to availabilities. For example, if ModifierType = Strictness, then this
// returns true for `strict(removed=2)`, even though that relies on the default
// of flexible after 2, whereas `strict(removed=2) flexible(added=2)` is fully
// explicit and does not rely on defaults. To enforce the latter, we'd need to
// lint the compiled flat AST instead of the raw AST.
template <typename ModifierType>
bool HasModifier(const std::unique_ptr<RawModifierList>& modifiers) {
  if (!modifiers)
    return false;
  for (auto& modifier : modifiers->modifiers) {
    if (std::holds_alternative<ModifierType>(modifier->value))
      return true;
  }
  return false;
}

}  // namespace

std::string Linter::MakeCopyrightBlock() {
  std::string copyright_block;
  for (const auto& line : kCopyrightLines) {
    copyright_block.append("\n");
    copyright_block.append(line);
  }
  return copyright_block;
}

const std::set<std::string>& Linter::permitted_library_prefixes() const {
  return kPermittedLibraryPrefixes;
}

std::string Linter::kPermittedLibraryPrefixesas_string() const {
  std::ostringstream ss;
  bool first = true;
  for (auto& prefix : permitted_library_prefixes()) {
    if (!first) {
      ss << " | ";
    }
    ss << prefix;
    first = false;
  }
  return ss.str();
}

// Returns itself. Overloaded to support alternative type references by
// pointer and unique_ptr as needed.
static const SourceElement& GetElementAsRef(const SourceElement& source_element) {
  return source_element;
}

static const SourceElement& GetElementAsRef(const SourceElement* element) {
  return GetElementAsRef(*element);
}

// Returns the pointed-to element as a reference.
template <typename SourceElementSubtype>
const SourceElement& GetElementAsRef(const std::unique_ptr<SourceElementSubtype>& element_ptr) {
  static_assert(std::is_base_of<SourceElement, SourceElementSubtype>::value,
                "template parameter type is not derived from SourceElement");
  return GetElementAsRef(element_ptr.get());
}
// Add a finding with |Finding| constructor arguments.
// This function is const because the Findings (TreeVisitor) object
// is not modified. It's Findings object (not owned) is updated.
Finding* Linter::AddFinding(SourceSpan span, std::string check_id, std::string message) {
  auto [it, inserted] = current_findings_.emplace(
      std::make_unique<Finding>(span, std::move(check_id), std::move(message)));
  ZX_ASSERT_MSG(inserted, "duplicate linter finding");
  return it->get();
}

// Add a finding with optional suggestion and replacement
const Finding* Linter::AddFinding(SourceSpan span, const CheckDef& check,
                                  const Substitutions& substitutions,
                                  std::string suggestion_template,
                                  std::string replacement_template) {
  auto* finding =
      AddFinding(span, std::string(check.id()), check.message_template().Substitute(substitutions));
  if (finding == nullptr) {
    return nullptr;
  }
  if (!suggestion_template.empty()) {
    if (replacement_template.empty()) {
      finding->SetSuggestion(
          TemplateString(std::move(suggestion_template)).Substitute(substitutions));
    } else {
      finding->SetSuggestion(
          TemplateString(std::move(suggestion_template)).Substitute(substitutions),
          TemplateString(std::move(replacement_template)).Substitute(substitutions));
    }
  }
  return finding;
}

// Add a finding from a SourceElement
template <typename SourceElementSubtypeRefOrPtr>
const Finding* Linter::AddFinding(const SourceElementSubtypeRefOrPtr& element,
                                  const CheckDef& check, Substitutions substitutions,
                                  std::string suggestion_template,
                                  std::string replacement_template) {
  return AddFinding(GetElementAsRef(element).span(), check, substitutions, suggestion_template,
                    replacement_template);
}

CheckDef Linter::DefineCheck(std::string_view check_id, std::string message_template) {
  auto [it, inserted] = checks_.emplace(check_id, TemplateString(std::move(message_template)));
  ZX_ASSERT_MSG(inserted, "DefineCheck called with a duplicate check_id");
  return *it;
}

// Returns true if no new findings were generated
bool Linter::Lint(const std::unique_ptr<File>& parsed_source, Findings* findings,
                  std::set<std::string>* excluded_checks_not_found) {
  auto initial_findings_size = findings->size();
  callbacks_.Visit(parsed_source);
  for (auto& finding_ptr : current_findings_) {
    auto check_id = finding_ptr->subcategory();
    if (excluded_checks_not_found && !excluded_checks_not_found->empty()) {
      excluded_checks_not_found->erase(check_id);
    }
    bool is_included = included_check_ids_.find(check_id) != included_check_ids_.end();
    bool is_excluded =
        exclude_by_default_ || excluded_check_ids_.find(check_id) != excluded_check_ids_.end();
    bool is_experimental = IsCheckExperimental(check_id);
    if ((!is_excluded && !is_experimental) || is_included) {
      findings->emplace_back(std::move(*finding_ptr));
    }
  }
  current_findings_.clear();
  return findings->size() == initial_findings_size;
}

void Linter::NewFile(const File& element) {
  // Reset file state variables (for a new file)
  line_comments_checked_ = 0;
  added_invalid_copyright_finding_ = false;
  good_copyright_lines_found_ = 0;
  copyright_date_ = "";

  auto& prefix_component = element.library_decl->path->components.front();
  library_prefix_ = to_string(prefix_component);

  library_is_platform_source_library_ =
      IsZirconLibrary(library_prefix_) ||
      (kPermittedLibraryPrefixes.find(library_prefix_) != kPermittedLibraryPrefixes.end());

  filename_ = element.span().source_file().filename();

  file_is_in_platform_source_tree_ = false;

  if (RE2::PartialMatch(filename_, R"REGEX(\bfuchsia/)REGEX")) {
    file_is_in_platform_source_tree_ = true;
  } else {
    file_is_in_platform_source_tree_ = std::ifstream(filename_.c_str()).good();
  }
  invalid_case_for_decl_name_ =
      DefineCheck("invalid-case-for-decl-name", "${TYPE} must be named in UpperCamelCase");

  if (!library_is_platform_source_library_) {
    // TODO(https://fxbug.dev/42158866): Implement more specific test,
    // comparing proposed library prefix to actual
    // source path.
    std::string replacement = "fuchsia, perhaps?";
    AddFinding(element.library_decl->path, kLibraryPrefixCheck,
               {
                   {"ORIGINAL", library_prefix_},
                   {"REPLACEMENT", replacement},
               },
               "change '${ORIGINAL}' to ${REPLACEMENT}", "${REPLACEMENT}");
  }

  // Library names should not have more than four components.
  if (element.library_decl->path->components.size() > 4) {
    AddFinding(element.library_decl->path, kLibraryNameDepthCheck);
  }

  if (!IsZirconLibrary(library_prefix_)) {
    for (const auto& component : element.library_decl->path->components) {
      if (RE2::FullMatch(to_string(component), kDisallowedLibraryComponentRegex)) {
        AddFinding(component, kLibraryNameComponentCheck);
        break;
      }
    }
  }
  EnterContext("library");
}

const Finding* Linter::CheckCase(std::string type, const std::unique_ptr<RawIdentifier>& identifier,
                                 const CheckDef& check_def, const CaseType& case_type) {
  std::string id = to_string(identifier);
  if (!case_type.matches(id)) {
    return AddFinding(identifier, check_def,
                      {
                          {"TYPE", std::move(type)},
                          {"IDENTIFIER", id},
                          {"REPLACEMENT", case_type.convert(id)},
                      },
                      "change '${IDENTIFIER}' to '${REPLACEMENT}'", "${REPLACEMENT}");
  }
  return nullptr;
}

std::string Linter::GetCopyrightSuggestion() {
  auto copyright_block = kCopyrightBlock;
  if (!copyright_date_.empty()) {
    copyright_block = TemplateString(copyright_block).Substitute({{"YYYY", copyright_date_}});
  }
  if (good_copyright_lines_found_ == 0) {
    return "Insert missing header:\n" + copyright_block;
  }
  return "Update your header with:\n" + copyright_block;
}

void Linter::AddInvalidCopyrightFinding(SourceSpan span) {
  if (!added_invalid_copyright_finding_) {
    added_invalid_copyright_finding_ = true;
    AddFinding(span, kInvalidCopyrightCheck, {}, GetCopyrightSuggestion());
  }
}

void Linter::CheckInvalidCopyright(SourceSpan span, std::string line_comment,
                                   std::string line_to_match) {
  if (line_comment == line_to_match ||
      // TODO(https://fxbug.dev/42145767): Remove this branch once all platform FIDL files are
      // updated.
      line_comment == line_to_match + " All rights reserved.") {
    good_copyright_lines_found_++;
    return;
  }
  if (CopyrightCheckIsComplete()) {
    return;
  }
  auto end_it = line_comment.end();
  if (line_comment.size() > line_to_match.size()) {
    end_it = line_comment.begin() + static_cast<ssize_t>(line_to_match.size());
  }
  auto first_mismatch = std::mismatch(line_comment.begin(), end_it, line_to_match.begin());
  auto index = first_mismatch.first - line_comment.begin();
  if (index > 0) {
    std::string_view error_view = span.data();
    error_view.remove_prefix(index);
    auto& source_file = span.source_file();
    span = SourceSpan(error_view, source_file);
  }
  AddInvalidCopyrightFinding(span);
}

bool Linter::CopyrightCheckIsComplete() {
  return !file_is_in_platform_source_tree_ || added_invalid_copyright_finding_ ||
         good_copyright_lines_found_ >= kCopyrightLines.size();
}

void Linter::ExitContext() { type_stack_.pop(); }

Linter::Linter()
    : kLibraryNameDepthCheck(DefineCheck("too-many-nested-libraries",
                                         "Avoid library names with more than three dots")),
      kLibraryNameComponentCheck(
          DefineCheck("disallowed-library-name-component",
                      "Library names must not contain the following components: common, service, "
                      "util, base, f<letter>l, zx<word>")),
      kLibraryPrefixCheck(DefineCheck("wrong-prefix-for-platform-source-library",
                                      "FIDL library name is not currently allowed")),
      kInvalidCopyrightCheck(
          DefineCheck("invalid-copyright-for-platform-source-library",
                      "FIDL files defined in the Platform Source Tree (i.e., defined in "
                      "fuchsia.googlesource.com) must begin with the standard copyright notice")),
      kCopyrightLines({
          // First line may also contain " All rights reserved."
          "// Copyright ${YYYY} The Fuchsia Authors.",
          "// Use of this source code is governed by a BSD-style license that can be",
          "// found in the LICENSE file.",
      }),
      kCopyrightBlock(MakeCopyrightBlock()),
      kYearRegex(R"(\b(\d{4})\b)"),
      kDisallowedLibraryComponentRegex(R"(^(common|service|util|base|f[a-z]l|zx\w*)$)"),
      kPermittedLibraryPrefixes({
          "fdf",
          "fidl",
          "fuchsia",
          "test",
      }) {
  auto copyright_should_not_be_doc_comment =
      DefineCheck("copyright-should-not-be-doc-comment",
                  "Copyright notice should use non-flow-through comment markers");
  auto explicit_flexible_modifier = DefineCheck(
      "explicit-flexible-modifier", "${TYPE} must have an explicit 'flexible' modifier");
  auto explicit_flexible_method_modifier = DefineCheck(
      "explicit-flexible-method-modifier", "${METHOD} must have an explicit 'flexible' modifier");
  auto invalid_case_for_constant =
      DefineCheck("invalid-case-for-constant", "${TYPE} must be named in ALL_CAPS_SNAKE_CASE");
  auto invalid_case_for_decl_member =
      DefineCheck("invalid-case-for-decl-member", "${TYPE} must be named in lower_snake_case");
  auto modifiers_order = DefineCheck(
      "modifier-order", "Strictness modifier on ${TYPE} must always precede the resource modifier");
  auto todo_should_not_be_doc_comment =
      DefineCheck("todo-should-not-be-doc-comment",
                  "TODO comment should use a non-flow-through comment marker");
  auto string_bounds_not_specified =
      DefineCheck("string-bounds-not-specified", "Specify bounds for string");
  auto vector_bounds_not_specified =
      DefineCheck("vector-bounds-not-specified", "Specify bounds for vector");

  // clang-format off
  callbacks_.OnFile(
    [& linter = *this]
    //
    (const File& element) {
      linter.NewFile(element);
    });
  // clang-format on

  callbacks_.OnComment(
      [&linter = *this]
      //
      (const cpp20::span<const SourceSpan> spans) {
        for (const auto& span : spans) {
          linter.line_comments_checked_++;
          if (linter.CopyrightCheckIsComplete() &&
              linter.line_comments_checked_ > linter.kCopyrightLines.size()) {
            return;
          }
          // span.position() is not a lightweight operation, but as long as
          // the conditions above are checked first, the line number only needs
          // to be computed a minimum number of times.
          size_t line_number = span.position().line;
          std::string line_comment = std::string(span.data());
          if (line_number > linter.kCopyrightLines.size()) {
            if (!linter.CopyrightCheckIsComplete()) {
              linter.AddInvalidCopyrightFinding(span);
            }
            return;
          }
          if (linter.copyright_date_.empty()) {
            std::string year;
            if (RE2::PartialMatch(line_comment, linter.kYearRegex, &year)) {
              linter.copyright_date_ = year;
            }
          }
          auto line_to_match = linter.kCopyrightLines[line_number - 1];
          if (!linter.copyright_date_.empty()) {
            line_to_match =
                TemplateString(line_to_match).Substitute({{"YYYY", linter.copyright_date_}});
          }
          linter.CheckInvalidCopyright(span, line_comment, line_to_match);
        }
      });

  callbacks_.OnExitFile([&linter = *this]
                        //
                        (const File& element) {
                          if (!linter.CopyrightCheckIsComplete()) {
                            auto& source_file = element.span().source_file();
                            std::string_view error_view = source_file.data();
                            error_view.remove_suffix(source_file.data().size());
                            linter.AddInvalidCopyrightFinding(SourceSpan(error_view, source_file));
                          }
                          linter.ExitContext();
                        });

  callbacks_.OnUsing([&linter = *this,
                      case_check = DefineCheck("invalid-case-for-using-alias",
                                               "Using aliases must be named in lower_snake_case"),
                      &case_type = lower_snake_]
                     //
                     (const RawUsing& element) {
                       if (element.maybe_alias != nullptr) {
                         linter.CheckCase("using alias", element.maybe_alias, case_check,
                                          case_type);
                       }
                     });

  callbacks_.OnConstDeclaration(
      [&linter = *this, case_check = invalid_case_for_constant, &case_type = upper_snake_]
      //
      (const RawConstDeclaration& element) {
        linter.CheckCase("constants", element.identifier, case_check, case_type);
        linter.in_const_declaration_ = true;
      });

  callbacks_.OnExitConstDeclaration(
      [&linter = *this]
      //
      (const RawConstDeclaration& element) { linter.in_const_declaration_ = false; });

  callbacks_.OnProtocolDeclaration(
      [&linter = *this,
       name_contains_service_check = DefineCheck("protocol-name-includes-service",
                                                 "Protocols must not include the name 'service.'"),
       explicit_openness_modifier_check = DefineCheck(
           "explicit-openness-modifier", "${PROTOCOL} must have an explicit openness modifier")]
      //
      (const RawProtocolDeclaration& element) {
        linter.CheckCase("protocols", element.identifier, linter.invalid_case_for_decl_name(),
                         linter.upper_camel_);
        for (const auto& word : SplitIdentifierWords(to_string(element.identifier))) {
          if (word == "service") {
            linter.AddFinding(element.identifier, name_contains_service_check);
            break;
          }
        }
        // This does not always prevent reliance on default openness. See the HasModifier docs.
        if (!HasModifier<Openness>(element.modifiers)) {
          std::string id = to_string(element.identifier);
          linter.AddFinding(
              element.identifier, explicit_openness_modifier_check,
              {
                  {"PROTOCOL", id},
              },
              "Add 'open', 'ajar', or 'closed' as appropriate. See the FIDL API Rubric for guidance "
              "on which one to choose: https://fuchsia.dev/fuchsia-src/development/api/fidl#open-ajar-closed",
              "");
        }
        linter.EnterContext("protocol");
      });
  callbacks_.OnMethod(
      [&linter = *this, explicit_flexible_method_modifier_check = explicit_flexible_method_modifier]
      //
      (const RawProtocolMethod& element) {
        linter.CheckCase("methods", element.identifier, linter.invalid_case_for_decl_name(),
                         linter.upper_camel_);
        // This does not always prevent reliance on default stricntess. See the HasModifier docs.
        if (!HasModifier<Strictness>(element.modifiers)) {
          std::string id = to_string(element.identifier);
          linter.AddFinding(
              element.identifier, explicit_flexible_method_modifier_check,
              {
                  {"METHOD", id},
              },
              "Add 'flexible' or 'strict' as appropriate. See the FIDL API Rubric for guidance on "
              "which one to choose: https://fuchsia.dev/fuchsia-src/development/api/fidl#strict-flexible-method",
              "");
        }
      });
  callbacks_.OnEvent(
      [&linter = *this,
       event_check =
           DefineCheck("event-names-must-start-with-on", "Event names must start with 'On'"),
       explicit_flexible_method_modifier_check = explicit_flexible_method_modifier]
      //
      (const RawProtocolMethod& element) {
        std::string id = to_string(element.identifier);
        auto finding = linter.CheckCase("events", element.identifier,
                                        linter.invalid_case_for_decl_name(), linter.upper_camel_);
        if (finding && finding->suggestion().has_value()) {
          auto& suggestion = finding->suggestion().value();
          if (suggestion.replacement().has_value()) {
            id = suggestion.replacement().value();
          }
        }
        if ((id.compare(0, 2, "On") != 0) || !isupper(id[2])) {
          std::string replacement = "On" + id;
          linter.AddFinding(element.identifier, event_check,
                            {
                                {"IDENTIFIER", id},
                                {"REPLACEMENT", replacement},
                            },
                            "change '${IDENTIFIER}' to '${REPLACEMENT}'", "${REPLACEMENT}");
        }
        // This does not always prevent reliance on default strictness. See the HasModifier docs.
        if (!HasModifier<Strictness>(element.modifiers)) {
          linter.AddFinding(
              element.identifier, explicit_flexible_method_modifier_check,
              {
                  {"METHOD", id},
              },
              "Add 'flexible' or 'strict' as appropriate. See the FIDL API Rubric for guidance on "
              "which one to choose: https://fuchsia.dev/fuchsia-src/development/api/fidl#strict-flexible-method",
              "");
        }
      });
  callbacks_.OnExitProtocolDeclaration(
      [&linter = *this]
      //
      (const RawProtocolDeclaration& element) { linter.ExitContext(); });

  auto copyright_regex = std::make_unique<re2::RE2>(R"REGEX((?i)^[ \t]*Copyright \d\d\d\d\W)REGEX");
  auto todo_regex = std::make_unique<re2::RE2>(R"REGEX(^[ \t]*TODO\W)REGEX");
  callbacks_.OnAttribute(
      [&linter = *this, check = copyright_should_not_be_doc_comment,
       copyright_regex = std::move(copyright_regex), todo_check = todo_should_not_be_doc_comment,
       todo_regex = std::move(todo_regex)]
      //
      (const RawAttribute& element) {
        if (element.provenance == RawAttribute::Provenance::kDocComment) {
          auto constant = static_cast<RawLiteralConstant*>(element.args.front()->value.get());
          auto doc_comment = static_cast<RawDocCommentLiteral*>(constant->literal.get());
          if (re2::RE2::PartialMatch(doc_comment->value, *copyright_regex)) {
            linter.AddFinding(element, check, {}, "change '///' to '//'", "//");
          }
          if (re2::RE2::PartialMatch(doc_comment->value, *todo_regex)) {
            linter.AddFinding(element, todo_check, {}, "change '///' to '//'", "//");
          }
        }
      });
  callbacks_.OnTypeDeclaration(
      [&linter = *this]
      //
      (const RawTypeDeclaration& element) {
        auto* layout_ref = element.type_ctor->layout_ref.get();

        // TODO(https://fxbug.dev/42158155): Delete this check once new-types are supported.
        // Instead, we should have new-type specific language to report the invalid naming case to
        // the user.
        if (layout_ref->kind == RawLayoutReference::Kind::kNamed) {
          return;
        }
        auto* inline_layout = static_cast<RawInlineLayoutReference*>(layout_ref);
        std::string layout_kind = name_layout_kind(*inline_layout->layout);
        linter.CheckCase(layout_kind + "s", element.identifier, linter.invalid_case_for_decl_name(),
                         linter.upper_camel_);
      });
  callbacks_.OnAliasDeclaration([&linter = *this]
                                //
                                (const RawAliasDeclaration& element) {
                                  linter.CheckCase("alias", element.alias,
                                                   linter.invalid_case_for_decl_name(),
                                                   linter.upper_camel_);
                                });
  callbacks_.OnLayout(
      [&linter = *this, explicit_flexible_modifier_check = explicit_flexible_modifier,
       modifiers_order_check = modifiers_order]
      //
      (const RawLayout& element) {
        std::string layout_kind = name_layout_kind(element);
        linter.EnterContext(layout_kind);

        // This does not always prevent reliance on default strictness. See the HasModifier docs.
        if (layout_kind != "table" && layout_kind != "struct" &&
            !HasModifier<Strictness>(element.modifiers)) {
          linter.AddFinding(element, explicit_flexible_modifier_check,
                            {
                                {"TYPE", layout_kind},
                            },
                            "add 'flexible' modifier before ${TYPE} keyword", "");
        }

        std::optional<Token> misplaced_strictness_token;
        if (element.modifiers) {
          bool saw_resource = false;
          for (auto& modifier : element.modifiers->modifiers) {
            if (saw_resource) {
              if (std::holds_alternative<Strictness>(modifier->value)) {
                misplaced_strictness_token = modifier->token;
                break;
              }
            } else if (std::holds_alternative<Resourceness>(modifier->value)) {
              saw_resource = true;
            }
          }
        }

        if (misplaced_strictness_token) {
          linter.AddFinding(
              element, modifiers_order_check,
              {
                  {"TYPE", layout_kind},
                  {"STRICTNESS", std::string(misplaced_strictness_token.value().span().data())},
              },
              "move '${STRICTNESS}' modifier before resource modifier for ${TYPE}", "");
        }
      });
  callbacks_.OnOrdinaledLayoutMember(
      [&linter = *this, case_check = invalid_case_for_decl_member, &case_type = lower_snake_]
      //
      (const RawOrdinaledLayoutMember& element) {
        std::string parent_type = linter.type_stack_.top();
        linter.CheckCase(parent_type + " members", element.identifier, case_check, case_type);
      });
  callbacks_.OnStructLayoutMember(
      [&linter = *this, case_check = invalid_case_for_decl_member, &case_type = lower_snake_]
      //
      (const RawStructLayoutMember& element) {
        std::string parent_type = linter.type_stack_.top();
        if (parent_type == "protocol") {
          linter.CheckCase("parameters", element.identifier, case_check, case_type);
          return;
        }

        linter.CheckCase("struct members", element.identifier, case_check, case_type);
      });
  callbacks_.OnValueLayoutMember(
      [&linter = *this, case_check = invalid_case_for_constant, &case_type = upper_snake_]
      //
      (const RawValueLayoutMember& element) {
        std::string parent_type = linter.type_stack_.top();
        linter.CheckCase(parent_type + " members", element.identifier, case_check, case_type);
      });
  callbacks_.OnExitLayout([&linter = *this]
                          //
                          (const RawLayout& element) { linter.ExitContext(); });

  // clang-format off
  callbacks_.OnIdentifierLayoutParameter(
      [& linter = *this,
          string_bounds_check = string_bounds_not_specified,
          vector_bounds_check = vector_bounds_not_specified]
          //
          (const RawIdentifierLayoutParameter& element) {
        if (element.identifier->span().data() == "string") {
          linter.AddFinding(element.identifier, string_bounds_check);
        }
      });
  callbacks_.OnTypeConstructor(
      [& linter = *this,
          string_bounds_check = string_bounds_not_specified,
          vector_bounds_check = vector_bounds_not_specified]
          //
          (const RawTypeConstructor& element) {
        if (element.layout_ref->kind != RawLayoutReference::Kind::kNamed)
          return;
        const auto as_named = static_cast<RawNamedLayoutReference*>(element.layout_ref.get());

        if (as_named->identifier->components.size() != 1) {
          return;
        }
        auto type = to_string((as_named->identifier->components[0]));
        if (!linter.in_const_declaration_) {
          // If there is a size attached to this type, it will always be the first numeric value in
          // the constraints list.
          bool has_size = false;
          if (element.constraints != nullptr && !element.constraints->items.empty()) {
            const auto& first_constraint = element.constraints->items.front();
            if (first_constraint->kind == RawConstant::Kind::kLiteral) {
              const auto as_lit_const = static_cast<RawLiteralConstant*>(first_constraint.get());
              if (as_lit_const->literal->kind == RawLiteral::Kind::kNumeric) {
                has_size = true;
              }
            } else if (first_constraint->kind == RawConstant::Kind::kIdentifier && first_constraint->span().data() != "optional") {
              // TODO(https://fxbug.dev/42157590): This check currently fails to recognize a shadowing const
              //  named optional, like:
              //
              //    const optional uint16 = 1234;
              //    type MyStruct = struct {
              //      this_will_trigger_incorrect_linter_warning string:optional;
              //    };
              has_size = true;
            }
          }

          if (type == "string" && !has_size) {
            linter.AddFinding(as_named->identifier, string_bounds_check);
          }
          if (type == "vector" && !has_size) {
            linter.AddFinding(as_named->identifier, vector_bounds_check);
          }
        }
      });
  // clang-format on
}

}  // namespace fidlc
