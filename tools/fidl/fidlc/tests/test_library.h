// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_FIDL_FIDLC_TESTS_TEST_LIBRARY_H_
#define TOOLS_FIDL_FIDLC_TESTS_TEST_LIBRARY_H_

#include <zircon/assert.h>

#include <cstring>
#include <type_traits>

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/compiler.h"
#include "tools/fidl/fidlc/src/diagnostic_types.h"
#include "tools/fidl/fidlc/src/experimental_flags.h"
#include "tools/fidl/fidlc/src/findings.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/json_generator.h"
#include "tools/fidl/fidlc/src/source_file.h"
#include "tools/fidl/fidlc/src/versioning_types.h"
#include "tools/fidl/fidlc/src/virtual_source_file.h"

#define ASSERT_COMPILED(library)                         \
  {                                                      \
    TestLibrary& library_ref = (library);                \
    if (!library_ref.Compile()) {                        \
      EXPECT_EQ(library_ref.errors().size(), 0u);        \
      for (const auto& error : library_ref.errors()) {   \
        EXPECT_EQ(error->def.msg, "");                   \
      }                                                  \
      FAIL() << "stopping test, compilation failed";     \
    }                                                    \
    EXPECT_EQ(library_ref.warnings().size(), 0u);        \
    for (const auto& warning : library_ref.warnings()) { \
      EXPECT_EQ(warning->def.msg, "");                   \
    }                                                    \
  }

#define ASSERT_COMPILER_DIAGNOSTICS(library) \
  {                                          \
    TestLibrary& library_ref = (library);    \
    if (!library_ref.CheckCompile()) {       \
      FAIL() << "Diagnostics mismatch";      \
    }                                        \
  }

namespace fidlc {

struct LintArgs {
 public:
  const std::set<std::string>& included_check_ids = {};
  const std::set<std::string>& excluded_check_ids = {};
  bool exclude_by_default = false;
  std::set<std::string>* excluded_checks_not_found = nullptr;
};

// Wrapper around std::set<Version> for convenience in parameterized tests.
struct TargetVersions {
  TargetVersions(std::initializer_list<Version> set) : set(set) {}
  explicit TargetVersions(std::string_view string);

  // Returns a string suitable for GTest parameterized test name suffixes.
  std::string ToString() const;

  struct CmpProxy {
    bool operator==(Version rhs) const { return Compare(std::equal_to{}, rhs); }
    bool operator!=(Version rhs) const { return Compare(std::not_equal_to{}, rhs); }
    bool operator<(Version rhs) const { return Compare(std::less{}, rhs); }
    bool operator>(Version rhs) const { return Compare(std::greater{}, rhs); }
    bool operator<=(Version rhs) const { return Compare(std::less_equal{}, rhs); }
    bool operator>=(Version rhs) const { return Compare(std::greater_equal{}, rhs); }
    template <typename Cmp>
    bool Compare(Cmp cmp, Version rhs) const {
      auto pred = [&](auto lhs) { return cmp(lhs, rhs); };
      return all ? std::all_of(set.begin(), set.end(), pred)
                 : std::any_of(set.begin(), set.end(), pred);
    }
    const std::set<Version>& set;
    bool all;
  };

  // Any() and All() return a proxy object with comparison operators. For
  // example, `Any() > V3` is true if any version in the set is greater than V3.
  CmpProxy Any() const { return CmpProxy{set, false}; }
  CmpProxy All() const { return CmpProxy{set, true}; }

  std::set<Version> set;
};

// Behavior that applies to SharedAmongstLibraries, but that is also provided on
// TestLibrary for convenience in single-library tests.
class SharedInterface {
 public:
  virtual Reporter* reporter() = 0;
  virtual Libraries* all_libraries() = 0;
  virtual VersionSelection* version_selection() = 0;
  virtual ExperimentalFlagSet& experimental_flags() = 0;
  virtual MethodHasher& method_hasher() = 0;

  // Adds and compiles a library similar to //zircon/vsdo/zx, defining "Handle",
  // "ObjType", and "Rights".
  virtual void UseLibraryZx() = 0;
  // Adds and compiles a library defining fdf.handle and fdf.obj_type.
  virtual void UseLibraryFdf() = 0;

  const std::vector<std::unique_ptr<Diagnostic>>& errors() { return reporter()->errors(); }
  const std::vector<std::unique_ptr<Diagnostic>>& warnings() { return reporter()->warnings(); }
  std::vector<Diagnostic*> Diagnostics() { return reporter()->Diagnostics(); }
  void set_warnings_as_errors(bool value) { reporter()->set_warnings_as_errors(value); }
  void PrintReports() { reporter()->PrintReports(/*enable_color=*/false); }
  void SelectVersion(std::string platform, std::string_view version) {
    SelectVersion(std::move(platform), Version::Parse(version).value());
  }
  void SelectVersion(std::string platform, Version version) {
    version_selection()->Insert(Platform::Parse(std::move(platform)).value(), {version});
  }
  void SelectVersions(std::string platform, TargetVersions versions) {
    version_selection()->Insert(Platform::Parse(std::move(platform)).value(),
                                std::move(versions).set);
  }
  void EnableFlag(ExperimentalFlag flag) { experimental_flags().Enable(flag); }
};

// Stores data structures that are shared amongst all libraries being compiled
// together (i.e. the dependencies and the final library).
class SharedAmongstLibraries final : public SharedInterface {
 public:
  SharedAmongstLibraries() : all_libraries_(&reporter_, &virtual_file_) {}
  // Unsafe to copy/move because all_libraries_ stores a pointer to reporter_.
  SharedAmongstLibraries(const SharedAmongstLibraries&) = delete;
  SharedAmongstLibraries(SharedAmongstLibraries&&) = delete;

  Reporter* reporter() override { return &reporter_; }
  Libraries* all_libraries() override { return &all_libraries_; }
  VersionSelection* version_selection() override { return &version_selection_; }
  ExperimentalFlagSet& experimental_flags() override { return experimental_flags_; }
  MethodHasher& method_hasher() override { return method_hasher_; }
  void UseLibraryZx() override;
  void UseLibraryFdf() override;

  std::vector<std::unique_ptr<SourceFile>>& all_sources_of_all_libraries() {
    return all_sources_of_all_libraries_;
  }

 private:
  Reporter reporter_;
  VirtualSourceFile virtual_file_{"generated"};
  MethodHasher method_hasher_ = Sha256MethodHasher;
  Libraries all_libraries_;
  std::vector<std::unique_ptr<SourceFile>> all_sources_of_all_libraries_;
  VersionSelection version_selection_;
  ExperimentalFlagSet experimental_flags_;
};

// Helper template to allow passing either a string literal or `const Arg&`.
template <typename Arg>
struct StringOrArg {
  // NOLINTNEXTLINE(google-explicit-constructor)
  StringOrArg(const char* string) : string(string) {}
  template <typename From, typename = std::enable_if_t<std::is_convertible_v<From, Arg>>>
  // NOLINTNEXTLINE(google-explicit-constructor)
  StringOrArg(const From& arg) : string(internal::Display(static_cast<const Arg&>(arg))) {}
  std::string string;
};

// Test harness for a single library. To compile multiple libraries together,
// first default construct a SharedAmongstLibraries and then pass it to each
// TestLibrary, and compile them one at a time in dependency order.
class TestLibrary final : public SharedInterface {
 public:
  // Constructor for a single-library, single-file test.
  explicit TestLibrary(const std::string& raw_source_code) : TestLibrary() {
    AddSource("example.fidl", raw_source_code);
  }

  // Constructor for a single-library, multi-file test (call AddSource after).
  TestLibrary() {
    owned_shared_.emplace();
    shared_ = &owned_shared_.value();
  }

  // Constructor for a multi-library, single-file test.
  TestLibrary(SharedAmongstLibraries* shared, const std::string& filename,
              const std::string& raw_source_code)
      : TestLibrary(shared) {
    AddSource(filename, raw_source_code);
  }

  // Constructor for a multi-library, multi-file test (call AddSource after).
  explicit TestLibrary(SharedAmongstLibraries* shared) : shared_(shared) {}

  ~TestLibrary();

  Reporter* reporter() override { return shared_->reporter(); }
  Libraries* all_libraries() override { return shared_->all_libraries(); }
  VersionSelection* version_selection() override { return shared_->version_selection(); }
  ExperimentalFlagSet& experimental_flags() override { return shared_->experimental_flags(); }
  MethodHasher& method_hasher() override { return shared_->method_hasher(); }
  void UseLibraryZx() override { shared_->UseLibraryZx(); }
  void UseLibraryFdf() override { shared_->UseLibraryFdf(); }

  void AddSource(const std::string& filename, const std::string& raw_source_code);

  AttributeSchema& AddAttributeSchema(std::string name) {
    return all_libraries()->AddAttributeSchema(std::move(name));
  }

  static std::string TestFilePath(const std::string& name);

  // Read the source from an associated external file.
  void AddFile(const std::string& name);

  // Record that a particular error is expected during the compile.
  // The args can either match the ErrorDef's argument types, or they can be string literals.
  template <ErrorId Id, typename... Args>
  void ExpectFail(const ErrorDef<Id, Args...>& def,
                  cpp20::type_identity_t<StringOrArg<Args>>... args) {
    expected_diagnostics_.push_back(internal::FormatDiagnostic(def.msg, args.string...));
  }

  // Record that a particular warning is expected during the compile.
  // The args can either match the WarningDef's argument types, or they can be string literals.
  template <ErrorId Id, typename... Args>
  void ExpectWarn(const WarningDef<Id, Args...>& def,
                  cpp20::type_identity_t<StringOrArg<Args>>... args) {
    expected_diagnostics_.push_back(internal::FormatDiagnostic(def.msg, args.string...));
  }

  // Check that the diagnostics expected with ExpectFail and ExpectWarn were recorded, in that order
  // by the compilation. This prints information about diagnostics mismatches and returns false if
  // any were found.
  bool CheckDiagnostics();

  // TODO(https://fxbug.dev/42069446): remove (or rename this class to be more general), as this
  // does not use a library.
  bool Parse(std::unique_ptr<File>* out_ast_ptr);

  // Compiles the library. Must have compiled all dependencies first, using the
  // same SharedAmongstLibraries object for all of them.
  bool Compile();

  // Compiles the library and checks that the diagnostics asserted with
  bool CheckCompile();

  bool Lint(LintArgs args = {});

  std::string GenerateJSON() {
    auto json_generator = JSONGenerator(compilation_.get(), experimental_flags());
    auto out = json_generator.Produce();
    return out.str();
  }

  const Bits* LookupBits(std::string_view name);
  const Const* LookupConstant(std::string_view name);
  const Enum* LookupEnum(std::string_view name);
  const Resource* LookupResource(std::string_view name);
  const Service* LookupService(std::string_view name);
  const Struct* LookupStruct(std::string_view name);
  const NewType* LookupNewType(std::string_view name);
  const Table* LookupTable(std::string_view name);
  const Alias* LookupAlias(std::string_view name);
  const Union* LookupUnion(std::string_view name);
  const Overlay* LookupOverlay(std::string_view name);
  const Protocol* LookupProtocol(std::string_view name);

  bool HasBits(std::string_view name) { return LookupBits(name) != nullptr; }
  bool HasConstant(std::string_view name) { return LookupConstant(name) != nullptr; }
  bool HasEnum(std::string_view name) { return LookupEnum(name) != nullptr; }
  bool HasResource(std::string_view name) { return LookupResource(name) != nullptr; }
  bool HasService(std::string_view name) { return LookupService(name) != nullptr; }
  bool HasStruct(std::string_view name) { return LookupStruct(name) != nullptr; }
  bool HasNewType(std::string_view name) { return LookupNewType(name) != nullptr; }
  bool HasTable(std::string_view name) { return LookupTable(name) != nullptr; }
  bool HasAlias(std::string_view name) { return LookupAlias(name) != nullptr; }
  bool HasUnion(std::string_view name) { return LookupUnion(name) != nullptr; }
  bool HasOverlay(std::string_view name) { return LookupOverlay(name) != nullptr; }
  bool HasProtocol(std::string_view name) { return LookupProtocol(name) != nullptr; }

  const SourceFile& source_file() const {
    ZX_ASSERT_MSG(all_sources_.size() == 1, "convenience method only possible with single source");
    return *all_sources_.at(0);
  }

  std::vector<const SourceFile*> source_files() const;
  SourceSpan source_span(size_t start, size_t size) const;
  SourceSpan find_source_span(std::string_view span_text);

  const Findings& findings() const { return findings_; }
  const std::vector<std::string>& lints() const { return lints_; }

  std::string_view name() const { return compilation_->library_name; }
  const Platform& platform() const { return *compilation_->platform; }
  Version version_added() const { return compilation_->version_added; }
  const AttributeList* attributes() { return compilation_->library_attributes; }
  const std::vector<const Struct*>& external_structs() const {
    return compilation_->external_structs;
  }
  const std::vector<const Decl*>& declaration_order() const {
    return compilation_->declaration_order;
  }
  const std::vector<Compilation::Dependency>& direct_and_composed_dependencies() const {
    return compilation_->direct_and_composed_dependencies;
  }

 private:
  std::optional<SharedAmongstLibraries> owned_shared_;
  SharedAmongstLibraries* shared_;
  Findings findings_;
  std::vector<std::string> lints_;
  std::vector<SourceFile*> all_sources_;
  std::unique_ptr<Compilation> compilation_;
  std::vector<std::string> expected_diagnostics_;
  bool used_ = false;
};

}  // namespace fidlc

#endif  // TOOLS_FIDL_FIDLC_TESTS_TEST_LIBRARY_H_
