// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_DIAGNOSTICS_H_
#define LIB_LD_DIAGNOSTICS_H_

#include <lib/elfldltl/diagnostics.h>
#include <lib/symbolizer-markup/writer.h>

#include <string_view>
#include <utility>

namespace ld {

// This produces symbolizer context markup describing one module.  The
// BufferSize template parameter is required and gives the size of a local
// stack buffer that will be used to minimize repeated calls to the Log
// function.  The Log object must be callable as `void(std::string_view)`.
// It will get one call per line (if `.back() == '\n'`) or for partial
// lines if the buffer fills.  The Module object can be an ld::LoadModule
// subclass or something that has similar `.name()`, `.module()`, and
// `.load_info()` methods with a `.module().build_id` and where the
// `.module().vaddr_start` is a final value including the load bias.
template <size_t BufferSize, typename Log, class Module>
constexpr void ModuleSymbolizerContext(Log&& log, const Module& module) {
  char buffer[BufferSize];
  size_t pos = 0;
  auto line_buffered_log = [&buffer, &pos, &log](std::string_view str) {
    while (!str.empty()) {
      size_t n = str.copy(&buffer[pos], sizeof(buffer) - pos);
      str.remove_prefix(n);
      pos += n;
      if (pos == sizeof(buffer) || buffer[pos - 1] == '\n') {
        log(std::string_view{buffer, pos});
        pos = 0;
      }
    }
  };

  std::string_view name = module.name().str();
  if (name.empty()) {
    name = "<application>";
  }

  symbolizer_markup::Writer writer(line_buffered_log);
  module.load_info().SymbolizerContext(writer, module.module().symbolizer_modid, name,
                                       module.module().build_id, module.module().vaddr_start);

  // Since writer.Newline() is always called last, the last call to
  // line_buffered_log will always have been with a trailing newline so the
  // buffer will have been flushed.
  assert(pos == 0);
}

// This is a CRTP base class to define a Report type to use with the
// elfldltl::Diagnostics template (see <lib/elfldltl/diagnostics.h>).  It
// holds a current module name that can be changed; when set, it's used as a
// prefix on messages.  The derived class must define one method:
// ```
// class MyReport : public ld::ModuleDiagnosticsReportBase<MyReport> {
// public:
//   template <typename... Args>
//   bool Report(Args&&... args);
// };
// ```
// This is called just like the elfldltl::Diagnostics<...>::report() callable
// object gets called.  When the current module name is set, the Report method
// calls get leading std::string_view arguments of `module(), ": "`.
template <class Derived>
class ModuleDiagnosticsReportBase {
 public:
  constexpr std::string_view module() const { return module_; }

  constexpr void set_module(std::string_view module) { module_ = module; }

  constexpr void clear_module() { module_ = {}; }

  template <typename... Args>
  constexpr bool operator()(Args&&... args) {
    auto report = [&](auto... prefix) -> bool {
      Derived& self = *static_cast<Derived*>(this);
      return self.Report(prefix..., std::forward<Args>(args)...);
    };
    return module_.empty() ? report() : report(module_, kColon);
  }

 private:
  static constexpr std::string_view kColon = ": ";

  std::string_view module_;
};

// This is a specialized version of ModuleDiagnosticsReportBase for using
// elfldltl::PrintfDiagnosticsReport.  Instead of defining a Report method, the
// derived class must define the method `void Printf(const char*, ...);` taking
// arguments like printf:
// ```
// struct MyReport : ModuleDiagnosticsPrintfReportBase<MyReport> {
//   void Printf(const char* format, ...);
// };
// ```
template <class Derived>
struct ModuleDiagnosticsPrintfReportBase : ModuleDiagnosticsReportBase<Derived> {
  template <typename... Args>
  constexpr bool Report(Args&&... args) {
    Derived& self = *static_cast<Derived*>(this);
    auto printf = [&self](auto... args) { self.Printf(args...); };
    auto report = elfldltl::PrintfDiagnosticsReport(printf);
    return report(std::forward<Args>(args)...);
  }
};

// This is an RAII type for use with some Diagnostics type whose Report type is
// an ld::ModuleDiagnosticsReportBase subclass.  It exists to temporarily set
// the module name in the Diagnostics::report() object.  The report().module()
// must be empty when this is constructed; it's cleared when this is destroyed.
// So it's used as in:
// ```
// bool LinkingStuffForModule(Diagnostics& diag, Module& module) {
//   ld::ScopedModuleDiagnostics module_diag{diag, module.name().str()};
//   if (!DoSomeLinking(diag, module)) {
//     return false;
//   }
//   return DoSomeMoreLinking(diag, module);
// }
// ```
// That is, the `ld::ModuleDiagnostics` object is allowed to go out of scope
// and be destroyed before using the Diagnostics object for things that
// shouldn't be prefixed with this module name.
template <class Diagnostics>
class ScopedModuleDiagnostics {
 public:
  constexpr explicit ScopedModuleDiagnostics(Diagnostics& diag, std::string_view name)
      : diag_(diag) {
    assert(diag_.report().module().empty());
    diag_.report().set_module(name);
  }

  ~ScopedModuleDiagnostics() { diag_.report().clear_module(); }

 private:
  Diagnostics& diag_;
};

// Deduction guide.
template <class Diagnostics>
ScopedModuleDiagnostics(Diagnostics&, std::string_view) -> ScopedModuleDiagnostics<Diagnostics>;

}  // namespace ld

#endif  // LIB_LD_DIAGNOSTICS_H_
