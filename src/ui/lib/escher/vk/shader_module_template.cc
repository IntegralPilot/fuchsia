// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/vk/shader_module_template.h"

#include "src/ui/lib/escher/shaders/util/spirv_file_util.h"
#include "src/ui/lib/escher/util/hasher.h"
#include "src/ui/lib/escher/util/trace_macros.h"
#ifdef __Fuchsia__
#include <fidl/fuchsia.io/cpp/fidl.h>
#endif

#if ESCHER_USE_RUNTIME_GLSL
#include <shaderc/shaderc.hpp>  // nogncheck
#endif

namespace escher {

#if ESCHER_USE_RUNTIME_GLSL
ShaderModuleTemplate::ShaderModuleTemplate(vk::Device device, shaderc::Compiler* compiler,
                                           ShaderStage shader_stage, HackFilePath path,
                                           HackFilesystemPtr filesystem)
    : device_(device),
      compiler_(compiler),
      shader_stage_(shader_stage),
      path_(std::move(path)),
      filesystem_(std::move(filesystem)) {}
#else
ShaderModuleTemplate::ShaderModuleTemplate(vk::Device device, ShaderStage shader_stage,
                                           HackFilePath path, HackFilesystemPtr filesystem)
    : device_(device),
      shader_stage_(shader_stage),
      path_(std::move(path)),
      filesystem_(std::move(filesystem)) {}
#endif  // ESCHER_USE_RUNTIME_GLSL

ShaderModuleTemplate::~ShaderModuleTemplate() { FX_DCHECK(variants_.empty()); }

ShaderModulePtr ShaderModuleTemplate::GetShaderModuleVariant(const ShaderVariantArgs& args) {
  if (Variant* variant = variants_[args]) {
    return ShaderModulePtr(variant);
  }

  auto variant = new Variant(this, args);

  auto module_ptr = fxl::AdoptRef<ShaderModule>(variant);
  RegisterVariant(variant);
  variant->ScheduleCompilation();
  return module_ptr;
}

void ShaderModuleTemplate::RegisterVariant(Variant* variant) {
  FX_DCHECK(variants_.find(variant->args()) != variants_.end()) << "Variant already registered.";
  variants_[variant->args()] = variant;
}

void ShaderModuleTemplate::UnregisterVariant(Variant* variant) {
  auto it = variants_.find(variant->args());
  FX_DCHECK(it != variants_.end());
  FX_DCHECK(it->second == variant);
  variants_.erase(it);
}

void ShaderModuleTemplate::ScheduleVariantCompilation(fxl::WeakPtr<Variant> variant) {
  // TODO(https://fxbug.dev/42098032): Recompile immediately.  Eventually we might want to
  // momentarily defer this, so that we don't recompile multiple times if
  // several files are changing at once (as when all changed files are pushed to
  // the device in rapid succession).
  if (variant) {
    variant->UpdateModule();
  }
}

#if ESCHER_USE_RUNTIME_GLSL
bool ShaderModuleTemplate::CompileVariantToSpirv(const ShaderVariantArgs& args,
                                                 std::vector<uint32_t>* output) {
  FX_CHECK(output);
  Variant* variant = nullptr;
  if (variants_[args]) {
    variant = variants_[args];
  } else {
    variant = new Variant(this, args);
    RegisterVariant(variant);
  }

  // Variant only has the method |GenerateSpirV| when ESCHER_USE_RUNTIME_GLSL is true.
  return variant->GenerateSpirV(output);
}
#endif

ShaderModuleTemplate::Variant::Variant(ShaderModuleTemplate* tmplate, ShaderVariantArgs args)
    : ShaderModule(tmplate->device_, tmplate->shader_stage_),
      template_(tmplate),
      args_(std::move(args)),
      weak_factory_(this) {
  // Cannot do this as an initializer, because weak_factory_ must have already
  // been initialized, and weak_factory_ must be initialized last (at least if
  // we don't want to invite trouble).
  auto& fs = template_->filesystem_;
  filesystem_watcher_ =
      fs->RegisterWatcher([weak = weak_factory_.GetWeakPtr()](HackFilePath changed_path) {
        if (weak) {
          weak->template_->ScheduleVariantCompilation(weak);
        }
      });
}

ShaderModuleTemplate::Variant::~Variant() { template_->UnregisterVariant(this); }

void ShaderModuleTemplate::Variant::ScheduleCompilation() {
  template_->ScheduleVariantCompilation(weak_factory_.GetWeakPtr());
}

#if ESCHER_USE_RUNTIME_GLSL

// Generates the spirv for a compiled shader and returns it via the |output| parameter.
// Returns true if the compilation was successful and false otherwise.
bool ShaderModuleTemplate::Variant::GenerateSpirV(std::vector<uint32_t>* output) {
  TRACE_DURATION("gfx", "ShaderModuleTemplate::GenerateSpirV");
  return shader_util::CompileGlslToSpirv(template_->compiler_, shader_stage(), template_->path_,
                                         args_, filesystem_watcher_.get(), output);
}

// Generates the spirv  for the shader and recreates the vk shader module with it.
void ShaderModuleTemplate::Variant::UpdateModule() {
  std::vector<uint32_t> spirv;
  bool result = GenerateSpirV(&spirv);
  FX_CHECK(result) << "Shader compilation failed!";
  RecreateModuleFromSpirvAndNotifyListeners(spirv);
}
#else

void ShaderModuleTemplate::Variant::UpdateModule() {
  std::vector<uint32_t> spirv;
  const std::optional<std::string>& base_path = template_->filesystem_->base_path();
  const auto& base_dir = template_->filesystem_->base_dir();
  if (!base_path.has_value() && !base_dir.has_value()) {
    // Mimic ReadSpirvFromDisk
    auto path = template_->path_ + std::to_string(args_.hash().val);
    std::replace(path.begin(), path.end(), '.', '_');
    std::replace(path.begin(), path.end(), '/', '_');
    path = "/data/shaders/" + path + ".spirv";
    auto contents = template_->filesystem_->ReadFile(path);
    FX_CHECK(!contents.empty()) << "module " << path << " is empty or non-existent.\n"
                                << "Update //src/ui/lib/escher/{BUILD.gn,test/gtest_escher.cc}";
    const size_t num = (contents.size() + 3) / sizeof(uint32_t);
    spirv.resize(num);
    memcpy(spirv.data(), contents.data(), num * sizeof(uint32_t));
  } else if (base_path.has_value()) {
    bool result =
        shader_util::ReadSpirvFromDisk(args_, *base_path + "/shaders/", template_->path_, &spirv);
    FX_CHECK(result) << "Read SPIR-V file failed!";
  } else if (base_dir.has_value()) {
#ifdef __Fuchsia__
    zx::channel client, server;
    zx::channel::create(0, &client, &server);
    fuchsia_io::OpenableOpenRequest request;
    request.path("shaders");
    request.flags(fuchsia_io::Flags::kProtocolDirectory | fuchsia_io::kPermReadable);
    request.options(fuchsia_io::Options{});
    request.object(std::move(server));
    auto _res = (*base_dir)->Open(std::move(request));
    fidl::SyncClient<fuchsia_io::Directory> shader_base_dir(
        fidl::ClientEnd<fuchsia_io::Directory>(std::move(client)));
    const bool result =
        shader_util::ReadSpirvFromDiskAtDir(args_, shader_base_dir, template_->path_, &spirv);
    FX_CHECK(result) << "Read SPIR-V file from dir failed!";
#else
    FX_CHECK(false) << "Found base_dir but it is not on Fuchsia";
#endif
  } else {
    FX_CHECK(false) << "Unreachable";
  }
  RecreateModuleFromSpirvAndNotifyListeners(spirv);
}

#endif  // ESCHER_USE_RUNTIME_GLSL
}  // namespace escher
