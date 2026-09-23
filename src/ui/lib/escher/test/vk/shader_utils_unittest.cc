// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/third_party/granite/vk/shader_utils.h"

#include <cstdint>
#include <cstring>
#include <string>
#include <vector>

#include <gtest/gtest.h>

#include "src/ui/lib/escher/third_party/granite/vk/shader_module_resource_layout.h"
#include "src/ui/lib/escher/vk/vulkan_limits.h"

namespace {

using escher::ShaderStage;
using escher::VulkanLimits;
using escher::impl::GenerateShaderModuleResourceLayoutFromSpirv;
using escher::impl::ShaderModuleResourceLayout;

// Escher only ever reflects on SPIR-V that was compiled from shaders in its own
// package, so there is no compiler available here that would emit the
// out-of-range decorations that these tests need.  Instead we hand-assemble
// minimal SPIR-V modules; they are not valid enough to hand to a driver, but
// they are exactly as much as SPIRV-Cross needs in order to reflect on them.
//
// See https://registry.khronos.org/SPIR-V/specs/unified1/SPIRV.html for the
// binary layout and the numeric values used below.
constexpr uint32_t kMagic = 0x07230203;
constexpr uint32_t kVersion1_0 = 0x00010000;

constexpr uint32_t kOpMemoryModel = 14;
constexpr uint32_t kOpEntryPoint = 15;
constexpr uint32_t kOpExecutionMode = 16;
constexpr uint32_t kOpCapability = 17;
constexpr uint32_t kOpTypeVoid = 19;
constexpr uint32_t kOpTypeFloat = 22;
constexpr uint32_t kOpTypeVector = 23;
constexpr uint32_t kOpTypeImage = 25;
constexpr uint32_t kOpTypeSampledImage = 27;
constexpr uint32_t kOpTypeStruct = 30;
constexpr uint32_t kOpTypePointer = 32;
constexpr uint32_t kOpTypeFunction = 33;
constexpr uint32_t kOpFunction = 54;
constexpr uint32_t kOpFunctionEnd = 56;
constexpr uint32_t kOpVariable = 59;
constexpr uint32_t kOpDecorate = 71;
constexpr uint32_t kOpMemberDecorate = 72;
constexpr uint32_t kOpLabel = 248;
constexpr uint32_t kOpReturn = 253;

constexpr uint32_t kCapabilityShader = 1;
constexpr uint32_t kAddressingModelLogical = 0;
constexpr uint32_t kMemoryModelGlsl450 = 1;
constexpr uint32_t kExecutionModelVertex = 0;
constexpr uint32_t kExecutionModelFragment = 4;
constexpr uint32_t kExecutionModeOriginUpperLeft = 7;
constexpr uint32_t kStorageClassUniformConstant = 0;
constexpr uint32_t kStorageClassInput = 1;
constexpr uint32_t kStorageClassOutput = 3;
constexpr uint32_t kStorageClassPushConstant = 9;
constexpr uint32_t kDim2D = 1;
constexpr uint32_t kImageFormatUnknown = 0;
constexpr uint32_t kDecorationBlock = 2;
constexpr uint32_t kDecorationLocation = 30;
constexpr uint32_t kDecorationBinding = 33;
constexpr uint32_t kDecorationDescriptorSet = 34;
constexpr uint32_t kDecorationOffset = 35;

// Assembles a SPIR-V module word by word.
class SpirvBuilder {
 public:
  // Reserves and returns a fresh result <id>.
  uint32_t NextId() { return next_id_++; }

  void Add(uint32_t opcode, const std::vector<uint32_t>& operands) {
    words_.push_back(static_cast<uint32_t>((operands.size() + 1) << 16) | opcode);
    words_.insert(words_.end(), operands.begin(), operands.end());
  }

  // Encodes a null-terminated SPIR-V literal string as a sequence of words.
  static std::vector<uint32_t> LiteralString(const std::string& str) {
    std::vector<uint32_t> words((str.size() + 4) / 4, 0);
    std::memcpy(words.data(), str.data(), str.size());
    return words;
  }

  std::vector<uint32_t> Build() const {
    std::vector<uint32_t> result = {kMagic, kVersion1_0, /*generator=*/0, /*bound=*/next_id_,
                                    /*schema=*/0};
    result.insert(result.end(), words_.begin(), words_.end());
    return result;
  }

 private:
  std::vector<uint32_t> words_;
  uint32_t next_id_ = 1;
};

// Emits the module preamble and returns the <id> of the (still undefined)
// entry point function.
uint32_t AddPreamble(SpirvBuilder* b, uint32_t execution_model) {
  const uint32_t entry_point = b->NextId();
  b->Add(kOpCapability, {kCapabilityShader});
  b->Add(kOpMemoryModel, {kAddressingModelLogical, kMemoryModelGlsl450});

  std::vector<uint32_t> entry_operands = {execution_model, entry_point};
  for (uint32_t word : SpirvBuilder::LiteralString("main")) {
    entry_operands.push_back(word);
  }
  b->Add(kOpEntryPoint, entry_operands);

  if (execution_model == kExecutionModelFragment) {
    b->Add(kOpExecutionMode, {entry_point, kExecutionModeOriginUpperLeft});
  }
  return entry_point;
}

// Emits `void main() {}` for the previously reserved |entry_point|.
void AddEmptyEntryPointFunction(SpirvBuilder* b, uint32_t entry_point, uint32_t void_type,
                                uint32_t function_type) {
  b->Add(kOpFunction, {void_type, entry_point, /*function control=*/0, function_type});
  b->Add(kOpLabel, {b->NextId()});
  b->Add(kOpReturn, {});
  b->Add(kOpFunctionEnd, {});
}

// A fragment shader declaring a single `sampler2D` at the given set/binding.
std::vector<uint32_t> FragmentShaderWithSampledImage(uint32_t set, uint32_t binding) {
  SpirvBuilder b;
  const uint32_t entry_point = AddPreamble(&b, kExecutionModelFragment);

  const uint32_t sampler = b.NextId();
  b.Add(kOpDecorate, {sampler, kDecorationDescriptorSet, set});
  b.Add(kOpDecorate, {sampler, kDecorationBinding, binding});

  const uint32_t void_type = b.NextId();
  b.Add(kOpTypeVoid, {void_type});
  const uint32_t function_type = b.NextId();
  b.Add(kOpTypeFunction, {function_type, void_type});
  const uint32_t float_type = b.NextId();
  b.Add(kOpTypeFloat, {float_type, 32});
  const uint32_t image_type = b.NextId();
  b.Add(kOpTypeImage, {image_type, float_type, kDim2D, /*depth=*/0, /*arrayed=*/0, /*ms=*/0,
                       /*sampled=*/1, kImageFormatUnknown});
  const uint32_t sampled_image_type = b.NextId();
  b.Add(kOpTypeSampledImage, {sampled_image_type, image_type});
  const uint32_t pointer_type = b.NextId();
  b.Add(kOpTypePointer, {pointer_type, kStorageClassUniformConstant, sampled_image_type});
  b.Add(kOpVariable, {pointer_type, sampler, kStorageClassUniformConstant});

  AddEmptyEntryPointFunction(&b, entry_point, void_type, function_type);
  return b.Build();
}

// A shader declaring a single vec4 stage input (vertex) or output (fragment) at
// the given location.
std::vector<uint32_t> ShaderWithStageVariableAtLocation(ShaderStage stage, uint32_t location) {
  const bool is_vertex = stage == ShaderStage::kVertex;
  const uint32_t storage_class = is_vertex ? kStorageClassInput : kStorageClassOutput;

  SpirvBuilder b;
  const uint32_t entry_point =
      AddPreamble(&b, is_vertex ? kExecutionModelVertex : kExecutionModelFragment);

  const uint32_t variable = b.NextId();
  b.Add(kOpDecorate, {variable, kDecorationLocation, location});

  const uint32_t void_type = b.NextId();
  b.Add(kOpTypeVoid, {void_type});
  const uint32_t function_type = b.NextId();
  b.Add(kOpTypeFunction, {function_type, void_type});
  const uint32_t float_type = b.NextId();
  b.Add(kOpTypeFloat, {float_type, 32});
  const uint32_t vec4_type = b.NextId();
  b.Add(kOpTypeVector, {vec4_type, float_type, 4});
  const uint32_t pointer_type = b.NextId();
  b.Add(kOpTypePointer, {pointer_type, storage_class, vec4_type});
  b.Add(kOpVariable, {pointer_type, variable, storage_class});

  AddEmptyEntryPointFunction(&b, entry_point, void_type, function_type);
  return b.Build();
}

// A fragment shader with a push constant block containing a single float member
// at |offset| bytes.
std::vector<uint32_t> FragmentShaderWithPushConstantAtOffset(uint32_t offset) {
  SpirvBuilder b;
  const uint32_t entry_point = AddPreamble(&b, kExecutionModelFragment);

  const uint32_t struct_type = b.NextId();
  b.Add(kOpDecorate, {struct_type, kDecorationBlock});
  b.Add(kOpMemberDecorate, {struct_type, /*member=*/0, kDecorationOffset, offset});

  const uint32_t void_type = b.NextId();
  b.Add(kOpTypeVoid, {void_type});
  const uint32_t function_type = b.NextId();
  b.Add(kOpTypeFunction, {function_type, void_type});
  const uint32_t float_type = b.NextId();
  b.Add(kOpTypeFloat, {float_type, 32});
  b.Add(kOpTypeStruct, {struct_type, float_type});
  const uint32_t pointer_type = b.NextId();
  b.Add(kOpTypePointer, {pointer_type, kStorageClassPushConstant, struct_type});
  b.Add(kOpVariable, {pointer_type, b.NextId(), kStorageClassPushConstant});

  AddEmptyEntryPointFunction(&b, entry_point, void_type, function_type);
  return b.Build();
}

TEST(ShaderUtils, ReflectsSampledImageWithinLimits) {
  ShaderModuleResourceLayout layout;
  GenerateShaderModuleResourceLayoutFromSpirv(FragmentShaderWithSampledImage(/*set=*/1,
                                                                             /*binding=*/3),
                                              ShaderStage::kFragment, &layout);
  EXPECT_EQ(layout.sets[1].sampled_image_mask, 1u << 3);
  EXPECT_EQ(layout.sets[0].sampled_image_mask, 0u);
}

TEST(ShaderUtils, ReflectsStageVariablesWithinLimits) {
  ShaderModuleResourceLayout vertex_layout;
  GenerateShaderModuleResourceLayoutFromSpirv(
      ShaderWithStageVariableAtLocation(ShaderStage::kVertex,
                                        VulkanLimits::kNumVertexAttributes - 1),
      ShaderStage::kVertex, &vertex_layout);
  EXPECT_EQ(vertex_layout.attribute_mask, 1u << (VulkanLimits::kNumVertexAttributes - 1));

  ShaderModuleResourceLayout fragment_layout;
  GenerateShaderModuleResourceLayoutFromSpirv(
      ShaderWithStageVariableAtLocation(ShaderStage::kFragment,
                                        VulkanLimits::kNumColorAttachments - 1),
      ShaderStage::kFragment, &fragment_layout);
  EXPECT_EQ(fragment_layout.render_target_mask, 1u << (VulkanLimits::kNumColorAttachments - 1));
}

TEST(ShaderUtils, ReflectsPushConstantsWithinLimits) {
  ShaderModuleResourceLayout layout;
  GenerateShaderModuleResourceLayoutFromSpirv(FragmentShaderWithPushConstantAtOffset(16),
                                              ShaderStage::kFragment, &layout);
  EXPECT_EQ(layout.push_constant_offset, 16u);
  EXPECT_EQ(layout.push_constant_range, 4u);
}

// The remaining tests all exercise limits which are enforced with FX_CHECK.
// Escher's shaders are first-party, so exceeding a limit is an authoring error
// rather than something a client can trigger; crashing is preferable to
// silently indexing out of the fixed-size arrays and masks in
// ShaderModuleResourceLayout.

TEST(ShaderUtilsDeathTest, RejectsOutOfRangeDescriptorSet) {
  ShaderModuleResourceLayout layout;
  EXPECT_DEATH(GenerateShaderModuleResourceLayoutFromSpirv(
                   FragmentShaderWithSampledImage(
                       /*set=*/VulkanLimits::kNumDescriptorSets, /*binding=*/0),
                   ShaderStage::kFragment, &layout),
               "descriptor set");
}

TEST(ShaderUtilsDeathTest, RejectsOutOfRangeBinding) {
  ShaderModuleResourceLayout layout;
  EXPECT_DEATH(GenerateShaderModuleResourceLayoutFromSpirv(
                   FragmentShaderWithSampledImage(
                       /*set=*/0, /*binding=*/VulkanLimits::kNumBindings),
                   ShaderStage::kFragment, &layout),
               "binding");
}

TEST(ShaderUtilsDeathTest, RejectsOutOfRangeVertexAttributeLocation) {
  ShaderModuleResourceLayout layout;
  EXPECT_DEATH(GenerateShaderModuleResourceLayoutFromSpirv(
                   ShaderWithStageVariableAtLocation(ShaderStage::kVertex,
                                                     VulkanLimits::kNumVertexAttributes),
                   ShaderStage::kVertex, &layout),
               "vertex attributes");
}

TEST(ShaderUtilsDeathTest, RejectsOutOfRangeRenderTargetLocation) {
  ShaderModuleResourceLayout layout;
  EXPECT_DEATH(GenerateShaderModuleResourceLayoutFromSpirv(
                   ShaderWithStageVariableAtLocation(ShaderStage::kFragment,
                                                     VulkanLimits::kNumColorAttachments),
                   ShaderStage::kFragment, &layout),
               "color attachments");
}

TEST(ShaderUtilsDeathTest, RejectsOversizedPushConstantBlock) {
  ShaderModuleResourceLayout layout;
  EXPECT_DEATH(GenerateShaderModuleResourceLayoutFromSpirv(
                   FragmentShaderWithPushConstantAtOffset(VulkanLimits::kPushConstantSize),
                   ShaderStage::kFragment, &layout),
               "push constant block");
}

}  // namespace
