// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/fidl/fidlc/tests/test_library.h"

#include <zircon/assert.h>

#include <fstream>

#include "tools/fidl/fidlc/src/lexer.h"
#include "tools/fidl/fidlc/src/linter.h"
#include "tools/fidl/fidlc/src/parser.h"

namespace fidlc {

TargetVersions::TargetVersions(std::string_view string) {
  for (size_t i = 0, j = 0; j != std::string::npos; i = j + 1) {
    j = string.find(',', i);
    auto length = j == std::string::npos ? j : j - i;
    set.insert(Version::Parse(string.substr(i, length)).value());
  }
}

std::string TargetVersions::ToString() const {
  std::string result;
  for (auto version : set) {
    if (!result.empty())
      result.push_back('_');
    result.append(version.ToString());
  }
  return result;
}

void SharedAmongstLibraries::UseLibraryZx() {
  TestLibrary zx_lib(this, "zx.fidl", R"FIDL(
library zx;

type ObjType = enum : uint32 {
    NONE = 0;
    PROCESS = 1;
    THREAD = 2;
    VMO = 3;
    CHANNEL = 4;
    EVENT = 5;
    PORT = 6;
};

type Rights = bits : uint32 {
    DUPLICATE = 0x00000001;
    TRANSFER = 0x00000002;
};

resource_definition Handle : uint32 {
    properties {
        subtype ObjType;
        rights Rights;
    };
};
)FIDL");
  ZX_ASSERT_MSG(zx_lib.Compile(), "failed to compile library zx");
}

void SharedAmongstLibraries::UseLibraryFdf() {
  TestLibrary fdf_lib(this, "fdf.fidl", R"FIDL(
library fdf;

type ObjType = enum : uint32 {
  CHANNEL = 1;
};

resource_definition handle : uint32 {
    properties {
        subtype ObjType;
    };
};
)FIDL");
  ZX_ASSERT_MSG(fdf_lib.Compile(), "failed to compile library fdf");
}

TestLibrary::~TestLibrary() {
  ZX_ASSERT_MSG(used_,
                "TestLibrary appears unused; did you forget to call Parse, Compile, or Lint?");
  ZX_ASSERT_MSG(
      expected_diagnostics_.empty(),
      "TestLibrary has expected diagnostics; did you forget to call ASSERT_COMPILER_DIAGNOSTICS?");
}

void TestLibrary::AddSource(const std::string& filename, const std::string& raw_source_code) {
  std::string source_code(raw_source_code);
  // NUL terminate the string.
  source_code.resize(source_code.size() + 1);
  auto file = std::make_unique<SourceFile>(filename, source_code);
  all_sources_.push_back(file.get());
  shared_->all_sources_of_all_libraries().push_back(std::move(file));
}

// static
std::string TestLibrary::TestFilePath(const std::string& name) {
#ifndef TEST_DATA_ROOT
#error "TEST_DATA_ROOT must be defined"
#else
  // TEST_DATA_ROOT is expected to be the toolchain's output root path,
  // relative to the build root, e.g. "host_x64"
  return TEST_DATA_ROOT "/fidlc-tests/" + name;
#endif
}

void TestLibrary::AddFile(const std::string& name) {
  auto path = TestFilePath(name);
  const std::ifstream reader(path);
  if (!reader) {
    ZX_PANIC("AddFile failed to read %s: errno = %s\n", path.c_str(), strerror(errno));
  }
  std::stringstream buffer;
  buffer << reader.rdbuf();
  AddSource(name, buffer.str());
}

bool TestLibrary::CheckDiagnostics() {
  bool ok = true;
  size_t num_expected = expected_diagnostics_.size();
  size_t num_found = Diagnostics().size();
  for (size_t i = 0; i < std::max(num_expected, num_found); i++) {
    if (i < num_expected && i < num_found) {
      const std::string& expected = expected_diagnostics_[i];
      const std::string& found = Diagnostics()[i]->msg;
      const std::string found_at = Diagnostics()[i]->span.position_str();
      if (expected != found) {
        if (!ok) {
          fprintf(stderr, "\n");
        }
        fprintf(stderr, "Expected: %s\n   Found: %s\n      At: %s", expected.c_str(), found.c_str(),
                found_at.c_str());
        ok = false;
      }
    } else if (i < num_found) {
      const std::string& found = Diagnostics()[i]->msg;
      const std::string found_at = Diagnostics()[i]->span.position_str();
      if (!ok) {
        fprintf(stderr, "\n");
      }
      fprintf(stderr, "Unexpected: %s\n        At: %s\n", found.c_str(), found_at.c_str());
      ok = false;
    } else if (i < num_expected) {
      const std::string& expected = expected_diagnostics_[i];
      if (!ok) {
        fprintf(stderr, "\n");
      }
      fprintf(stderr, "Expected: %s\n", expected.c_str());
      ok = false;
    }
  }
  expected_diagnostics_.clear();
  return ok;
}

bool TestLibrary::Parse(std::unique_ptr<File>* out_ast_ptr) {
  ZX_ASSERT_MSG(all_sources_.size() == 1, "parse can only be used with one source");
  used_ = true;
  auto source_file = all_sources_.at(0);
  Lexer lexer(*source_file, reporter());
  Parser parser(&lexer, reporter(), experimental_flags());
  out_ast_ptr->reset(parser.Parse().release());
  return parser.Success();
}

// Compiles the library. Must have compiled all dependencies first, using the
// same SharedAmongstLibraries object for all of them.
bool TestLibrary::Compile() {
  used_ = true;
  Compiler compiler(all_libraries(), version_selection(), method_hasher(), experimental_flags());
  for (auto source_file : all_sources_) {
    Lexer lexer(*source_file, reporter());
    Parser parser(&lexer, reporter(), experimental_flags());
    auto ast = parser.Parse();
    if (!parser.Success())
      return false;
    if (!compiler.ConsumeFile(std::move(ast)))
      return false;
  }
  if (!compiler.Compile())
    return false;
  compilation_ = all_libraries()->Filter(version_selection());
  return true;
}

// Compiles the library and checks that the diagnostics asserted with
bool TestLibrary::CheckCompile() {
  bool compiled_ok = Compile();
  bool diagnostics_ok = CheckDiagnostics();
  // If the compile succeeded there should be no errors.
  ZX_ASSERT(compiled_ok == errors().empty());
  return diagnostics_ok;
}

bool TestLibrary::Lint(LintArgs args) {
  used_ = true;
  findings_ = Findings();

  bool passed = [&]() {
    ZX_ASSERT_MSG(all_sources_.size() == 1, "lint can only be used with one source");
    auto source_file = all_sources_.at(0);
    Lexer lexer(*source_file, reporter());
    Parser parser(&lexer, reporter(), experimental_flags());
    auto ast = parser.Parse();
    if (!parser.Success()) {
      std::string_view beginning(source_file->data().data(), 0);
      SourceSpan span(beginning, *source_file);
      const auto& error = errors().at(0);
      auto error_msg = Reporter::Format("error", error->span, error->Format(), /*color=*/false);
      findings_.emplace_back(span, "parser-error", error_msg + "\n");
      return false;
    }
    Linter linter;
    if (!args.included_check_ids.empty()) {
      linter.set_included_checks(args.included_check_ids);
    }
    if (!args.excluded_check_ids.empty()) {
      linter.set_excluded_checks(args.excluded_check_ids);
    }
    linter.set_exclude_by_default(args.exclude_by_default);
    return linter.Lint(ast, &findings_, args.excluded_checks_not_found);
  }();

  lints_ = FormatFindings(findings_, false);
  return passed;
}

const Bits* TestLibrary::LookupBits(std::string_view name) {
  for (const auto& bits_decl : compilation_->declarations.bits) {
    if (bits_decl->name.decl_name() == name) {
      return bits_decl;
    }
  }
  return nullptr;
}

const Const* TestLibrary::LookupConstant(std::string_view name) {
  for (const auto& const_decl : compilation_->declarations.consts) {
    if (const_decl->name.decl_name() == name) {
      return const_decl;
    }
  }
  return nullptr;
}

const Enum* TestLibrary::LookupEnum(std::string_view name) {
  for (const auto& enum_decl : compilation_->declarations.enums) {
    if (enum_decl->name.decl_name() == name) {
      return enum_decl;
    }
  }
  return nullptr;
}

const Resource* TestLibrary::LookupResource(std::string_view name) {
  for (const auto& resource_decl : compilation_->declarations.resources) {
    if (resource_decl->name.decl_name() == name) {
      return resource_decl;
    }
  }
  return nullptr;
}

const Service* TestLibrary::LookupService(std::string_view name) {
  for (const auto& service_decl : compilation_->declarations.services) {
    if (service_decl->name.decl_name() == name) {
      return service_decl;
    }
  }
  return nullptr;
}

const Struct* TestLibrary::LookupStruct(std::string_view name) {
  for (const auto& struct_decl : compilation_->declarations.structs) {
    if (struct_decl->name.decl_name() == name) {
      return struct_decl;
    }
  }
  return nullptr;
}

const NewType* TestLibrary::LookupNewType(std::string_view name) {
  for (const auto& new_type_decl : compilation_->declarations.new_types) {
    if (new_type_decl->name.decl_name() == name) {
      return new_type_decl;
    }
  }
  return nullptr;
}

const Table* TestLibrary::LookupTable(std::string_view name) {
  for (const auto& table_decl : compilation_->declarations.tables) {
    if (table_decl->name.decl_name() == name) {
      return table_decl;
    }
  }
  return nullptr;
}

const Alias* TestLibrary::LookupAlias(std::string_view name) {
  for (const auto& alias_decl : compilation_->declarations.aliases) {
    if (alias_decl->name.decl_name() == name) {
      return alias_decl;
    }
  }
  return nullptr;
}

const Union* TestLibrary::LookupUnion(std::string_view name) {
  for (const auto& union_decl : compilation_->declarations.unions) {
    if (union_decl->name.decl_name() == name) {
      return union_decl;
    }
  }
  return nullptr;
}
const Overlay* TestLibrary::LookupOverlay(std::string_view name) {
  for (const auto& overlay_decl : compilation_->declarations.overlays) {
    if (overlay_decl->name.decl_name() == name) {
      return overlay_decl;
    }
  }
  return nullptr;
}

const Protocol* TestLibrary::LookupProtocol(std::string_view name) {
  for (const auto& protocol_decl : compilation_->declarations.protocols) {
    if (protocol_decl->name.decl_name() == name) {
      return protocol_decl;
    }
  }
  return nullptr;
}

std::vector<const SourceFile*> TestLibrary::source_files() const {
  std::vector<const SourceFile*> out;
  out.reserve(all_sources_.size());
  for (const auto& source : all_sources_) {
    out.push_back(source);
  }
  return out;
}

SourceSpan TestLibrary::source_span(size_t start, size_t size) const {
  ZX_ASSERT_MSG(all_sources_.size() == 1, "convenience method only possible with single source");
  std::string_view data = all_sources_.at(0)->data();
  data.remove_prefix(start);
  data.remove_suffix(data.size() - size);
  return SourceSpan(data, *all_sources_.at(0));
}

SourceSpan TestLibrary::find_source_span(std::string_view span_text) {
  ZX_ASSERT_MSG(all_sources_.size() == 1, "convenience method only possible with single source");
  std::string_view data = all_sources_.at(0)->data();
  size_t pos = data.find(span_text);
  ZX_ASSERT_MSG(pos != std::string_view::npos, "source span text not found");
  return source_span(pos, span_text.size());
}

}  // namespace fidlc
