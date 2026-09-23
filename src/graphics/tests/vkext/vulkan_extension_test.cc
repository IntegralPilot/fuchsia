// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vulkan_extension_test.h"

#include <fuchsia/sysmem2/cpp/fidl.h>
#include <lib/fdio/directory.h>

#include <cstddef>
#include <cstdint>

#include <fbl/algorithm.h>

#include "src/graphics/tests/common/utils.h"
#include "src/lib/fsl/handles/object_info.h"

#include <vulkan/vulkan.hpp>

constexpr vk::SysmemColorSpaceFUCHSIA kDefaultRgbColorSpace(
    static_cast<uint32_t>(fuchsia::images2::ColorSpace::SRGB));
constexpr vk::SysmemColorSpaceFUCHSIA kDefaultYuvColorSpace(
    static_cast<uint32_t>(fuchsia::images2::ColorSpace::REC709));

vk::ImageFormatConstraintsInfoFUCHSIA GetDefaultImageFormatConstraintsInfo(bool yuv) {
  return vk::ImageFormatConstraintsInfoFUCHSIA()
      .setSysmemPixelFormat(0u)
      .setFlags({})
      .setPColorSpaces(yuv ? &kDefaultYuvColorSpace : &kDefaultRgbColorSpace)
      .setColorSpaceCount(1u)
      .setRequiredFormatFeatures(vk::FormatFeatureFlagBits::eTransferDst);
}

vk::ImageCreateInfo GetDefaultImageCreateInfo(bool use_protected_memory, VkFormat format,
                                              uint32_t width, uint32_t height, bool linear) {
  return vk::ImageCreateInfo()
      .setFlags(use_protected_memory ? vk::ImageCreateFlagBits::eProtected
                                     : vk::ImageCreateFlagBits())
      .setImageType(vk::ImageType::e2D)
      .setFormat(vk::Format(format))
      .setExtent(vk::Extent3D(width, height, 1))
      .setMipLevels(1)
      .setArrayLayers(1)
      .setSamples(vk::SampleCountFlagBits::e1)
      .setTiling(linear ? vk::ImageTiling::eLinear : vk::ImageTiling::eOptimal)
      // Only use TransferDst, because on Mali some other usages (like color attachment) aren't
      // supported for NV12, and some others (implementation-dependent) aren't supported with
      // AFBC, and sampled aren't supported with SwiftShader (linear images).
      .setUsage(vk::ImageUsageFlagBits::eTransferDst)
      .setSharingMode(vk::SharingMode::eExclusive)
      .setInitialLayout(vk::ImageLayout::eUndefined);
}

vk::ImageFormatConstraintsInfoFUCHSIA GetDefaultRgbImageFormatConstraintsInfo() {
  return GetDefaultImageFormatConstraintsInfo(false);
}

vk::ImageFormatConstraintsInfoFUCHSIA GetDefaultYuvImageFormatConstraintsInfo() {
  return GetDefaultImageFormatConstraintsInfo(true);
}

fuchsia::sysmem2::ImageFormatConstraints GetDefaultSysmemImageFormatConstraints() {
  fuchsia::sysmem2::ImageFormatConstraints bgra_image_constraints;
  bgra_image_constraints.set_required_min_size(fuchsia::math::SizeU{.width = 1024, .height = 1024});
  bgra_image_constraints.set_required_max_size(fuchsia::math::SizeU{.width = 1024, .height = 1024});
  bgra_image_constraints.set_max_size(fuchsia::math::SizeU{.width = 8192, .height = 8192});
  bgra_image_constraints.set_max_bytes_per_row(0xffffffff);
  bgra_image_constraints.set_pixel_format(fuchsia::images2::PixelFormat::B8G8R8A8);
  bgra_image_constraints.mutable_color_spaces()->emplace_back(fuchsia::images2::ColorSpace::SRGB);
  return bgra_image_constraints;
}

VulkanExtensionTest::~VulkanExtensionTest() {}

bool VulkanExtensionTest::Initialize() {
  if (is_initialized_) {
    return false;
  }

  if (!InitVulkan()) {
    RTN_MSG(false, "InitVulkan failed.\n");
  }

  if (!InitSysmemAllocator()) {
    RTN_MSG(false, "InitSysmemAllocator failed.\n");
  }

  is_initialized_ = true;

  return true;
}

bool VulkanExtensionTest::InitVulkan() {
  constexpr size_t kPhysicalDeviceIndex = 0;
  vk::ApplicationInfo app_info;
  app_info.pApplicationName = "vkext";
  app_info.apiVersion = VK_API_VERSION_1_1;
  vk::InstanceCreateInfo instance_info;
  instance_info.pApplicationInfo = &app_info;
  ctx_ = std::make_unique<VulkanContext>(kPhysicalDeviceIndex);
  ctx_->set_instance_info(instance_info);
  if (!ctx_->InitInstance()) {
    return false;
  }

  loader_.init(*ctx_->instance(), vkGetInstanceProcAddr);
  if (!ctx_->InitQueueFamily()) {
    return false;
  }

  // Set |device_supports_protected_memory_| flag.
  vk::PhysicalDeviceProtectedMemoryFeatures protected_memory(VK_TRUE);
  vk::PhysicalDeviceProperties physical_device_properties;
  ctx_->physical_device().getProperties(&physical_device_properties);
  if (VK_VERSION_MAJOR(physical_device_properties.apiVersion) != 1 ||
      VK_VERSION_MINOR(physical_device_properties.apiVersion) > 0) {
    vk::PhysicalDeviceFeatures2 features2;
    features2.pNext = &protected_memory;
    ctx_->physical_device().getFeatures2(&features2);
    if (protected_memory.protectedMemory) {
      device_supports_protected_memory_ = true;
    }
  }

  std::vector<const char *> enabled_device_extensions{VK_FUCHSIA_EXTERNAL_MEMORY_EXTENSION_NAME,
                                                      VK_FUCHSIA_BUFFER_COLLECTION_EXTENSION_NAME,
                                                      VK_EXT_QUEUE_FAMILY_FOREIGN_EXTENSION_NAME};
  vk::DeviceCreateInfo device_info;
  device_info.pNext = device_supports_protected_memory_ ? &protected_memory : nullptr;

  auto queue_info = ctx_->queue_info();
  if (device_supports_protected_memory_) {
    // The Mali ICD requires that a protected queue be created before creating protected memory.
    queue_info.flags |= vk::DeviceQueueCreateFlagBits::eProtected;
  }
  device_info.pQueueCreateInfos = &queue_info;
  device_info.queueCreateInfoCount = 1;
  device_info.enabledExtensionCount = static_cast<uint32_t>(enabled_device_extensions.size());
  device_info.ppEnabledExtensionNames = enabled_device_extensions.data();

  ctx_->set_device_info(device_info);
  if (!ctx_->InitDevice()) {
    return false;
  }

  return true;
}

bool VulkanExtensionTest::InitSysmemAllocator() {
  zx_status_t status = fdio_service_connect("/svc/fuchsia.sysmem2.Allocator",
                                            sysmem_allocator_.NewRequest().TakeChannel().release());
  if (status != ZX_OK) {
    RTN_MSG(false, "Fdio_service_connect failed: %d\n", status);
  }
  status = sysmem_allocator_->SetDebugClientInfo(
      std::move(fuchsia::sysmem2::AllocatorSetDebugClientInfoRequest{}
                    .set_name(fsl::GetCurrentProcessName())
                    .set_id(fsl::GetCurrentProcessKoid())));

  if (status != ZX_OK) {
    RTN_MSG(false, "SetDebugClientInfo failed: %d\n", status);
  }
  return true;
}

std::vector<fuchsia::sysmem2::BufferCollectionTokenSyncPtr>
VulkanExtensionTest::MakeSharedCollection(uint32_t token_count) {
  std::vector<fuchsia::sysmem2::BufferCollectionTokenSyncPtr> tokens;
  fuchsia::sysmem2::BufferCollectionTokenSyncPtr token1;
  zx_status_t status = sysmem_allocator_->AllocateSharedCollection(
      std::move(fuchsia::sysmem2::AllocatorAllocateSharedCollectionRequest{}.set_token_request(
          token1.NewRequest())));
  EXPECT_EQ(status, ZX_OK);
  token1->SetName(std::move(fuchsia::sysmem2::NodeSetNameRequest{}.set_priority(1u).set_name(
      ::testing::UnitTest::GetInstance()->current_test_info()->name())));

  for (uint32_t i = 1; i < token_count; ++i) {
    fuchsia::sysmem2::BufferCollectionTokenSyncPtr tokenN;
    status = token1->Duplicate(std::move(fuchsia::sysmem2::BufferCollectionTokenDuplicateRequest{}
                                             .set_rights_attenuation_mask(ZX_RIGHT_SAME_RIGHTS)
                                             .set_token_request(tokenN.NewRequest())));
    EXPECT_EQ(status, ZX_OK);
    tokens.push_back(std::move(tokenN));
  }

  fuchsia::sysmem2::Node_Sync_Result sync_result;
  status = token1->Sync(&sync_result);
  EXPECT_EQ(ZX_OK, status);
  tokens.push_back(std::move(token1));
  return tokens;
}

void VulkanExtensionTest::CheckLinearSubresourceLayout(VkFormat format, uint32_t width) {
  const vk::Device &device = *ctx_->device();
  bool is_yuv = (format == VK_FORMAT_G8_B8R8_2PLANE_420_UNORM_KHR) ||
                (format == VK_FORMAT_G8_B8_R8_3PLANE_420_UNORM_KHR);
  VkImageSubresource subresource = {
      .aspectMask = is_yuv ? VK_IMAGE_ASPECT_PLANE_0_BIT : VK_IMAGE_ASPECT_COLOR_BIT,
      .mipLevel = 0,
      .arrayLayer = 0};
  VkSubresourceLayout layout;
  vkGetImageSubresourceLayout(device, *vk_image_, &subresource, &layout);

  VkDeviceSize min_bytes_per_pixel = 0;
  switch (format) {
    case VK_FORMAT_G8_B8R8_2PLANE_420_UNORM_KHR:
    case VK_FORMAT_G8_B8_R8_3PLANE_420_UNORM_KHR:
      min_bytes_per_pixel = 1;
      break;

    case VK_FORMAT_R8_UNORM:
      min_bytes_per_pixel = 1;
      break;

    case VK_FORMAT_R8G8_UNORM:
    case VK_FORMAT_R5G6B5_UNORM_PACK16:
      min_bytes_per_pixel = 2;
      break;

    case VK_FORMAT_R8G8B8A8_UNORM:
    case VK_FORMAT_B8G8R8A8_UNORM:
    case VK_FORMAT_A2B10G10R10_UNORM_PACK32:
      min_bytes_per_pixel = 4;
      break;

    default:
      ADD_FAILURE();
      break;
  }

  EXPECT_LE(min_bytes_per_pixel * width, layout.rowPitch);
  EXPECT_LE(min_bytes_per_pixel * width * 64, layout.size);

  if (format == VK_FORMAT_G8_B8_R8_3PLANE_420_UNORM_KHR) {
    VkImageSubresource subresource = {
        .aspectMask = VK_IMAGE_ASPECT_PLANE_1_BIT, .mipLevel = 0, .arrayLayer = 0};
    VkSubresourceLayout b_layout;
    vkGetImageSubresourceLayout(device, *vk_image_, &subresource, &b_layout);

    subresource.aspectMask = VK_IMAGE_ASPECT_PLANE_2_BIT;
    VkSubresourceLayout r_layout;
    vkGetImageSubresourceLayout(device, *vk_image_, &subresource, &r_layout);

    // I420 has the U plane (mapped to B) before the V plane (mapped to R)
    EXPECT_LT(b_layout.offset, r_layout.offset);
  }
}

void VulkanExtensionTest::ValidateBufferProperties(const VkMemoryRequirements &requirements,
                                                   const vk::BufferCollectionFUCHSIA collection,
                                                   uint32_t expected_count,
                                                   uint32_t *memory_type_out) {
  vk::BufferCollectionPropertiesFUCHSIA properties;
  vk::Result result1 =
      ctx_->device()->getBufferCollectionPropertiesFUCHSIA(collection, &properties, loader_);
  EXPECT_EQ(result1, vk::Result::eSuccess);

  EXPECT_EQ(expected_count, properties.bufferCount);
  uint32_t viable_memory_types = properties.memoryTypeBits & requirements.memoryTypeBits;
  EXPECT_NE(0u, viable_memory_types);
  uint32_t memory_type = __builtin_ctz(viable_memory_types);

  VkPhysicalDeviceMemoryProperties memory_properties;
  vkGetPhysicalDeviceMemoryProperties(ctx_->physical_device(), &memory_properties);

  EXPECT_LT(memory_type, memory_properties.memoryTypeCount);
  if (use_protected_memory_) {
    for (uint32_t i = 0; i < memory_properties.memoryTypeCount; ++i) {
      if (properties.memoryTypeBits & (1 << i)) {
        // Based only on the buffer collection it should be possible to
        // determine that this is protected memory. viable_memory_types
        // is a subset of these bits, so that should be true for it as
        // well.
        EXPECT_TRUE(memory_properties.memoryTypes[i].propertyFlags &
                    VK_MEMORY_PROPERTY_PROTECTED_BIT);
      }
    }
  } else {
    EXPECT_FALSE(memory_properties.memoryTypes[memory_type].propertyFlags &
                 VK_MEMORY_PROPERTY_PROTECTED_BIT);
  }
  *memory_type_out = memory_type;
}

fuchsia::sysmem2::BufferCollectionInfo VulkanExtensionTest::AllocateSysmemCollection(
    std::optional<fuchsia::sysmem2::BufferCollectionConstraints> constraints,
    fuchsia::sysmem2::BufferCollectionTokenSyncPtr token) {
  fuchsia::sysmem2::BufferCollectionSyncPtr sysmem_collection;
  zx_status_t status = sysmem_allocator_->BindSharedCollection(
      std::move(fuchsia::sysmem2::AllocatorBindSharedCollectionRequest{}
                    .set_token(std::move(token))
                    .set_buffer_collection_request(sysmem_collection.NewRequest())));
  EXPECT_EQ(status, ZX_OK);
  if (constraints) {
    EXPECT_EQ(ZX_OK, sysmem_collection->SetConstraints(std::move(
                         fuchsia::sysmem2::BufferCollectionSetConstraintsRequest{}.set_constraints(
                             std::move(*constraints)))));
    constraints.reset();
  } else {
    EXPECT_EQ(ZX_OK, sysmem_collection->SetConstraints(
                         fuchsia::sysmem2::BufferCollectionSetConstraintsRequest{}));
  }

  fuchsia::sysmem2::BufferCollection_WaitForAllBuffersAllocated_Result wait_result;
  EXPECT_EQ(ZX_OK, sysmem_collection->WaitForAllBuffersAllocated(&wait_result));
  EXPECT_TRUE(wait_result.is_response());
  auto buffer_collection_info = std::move(*wait_result.response().mutable_buffer_collection_info());
  EXPECT_EQ(ZX_OK, sysmem_collection->Release());
  return buffer_collection_info;
}

bool VulkanExtensionTest::InitializeDirectImage(vk::BufferCollectionFUCHSIA collection,
                                                vk::ImageCreateInfo image_create_info) {
  VkBufferCollectionImageCreateInfoFUCHSIA image_format_fuchsia = {
      .sType = VK_STRUCTURE_TYPE_BUFFER_COLLECTION_IMAGE_CREATE_INFO_FUCHSIA,
      .pNext = nullptr,
      .collection = collection,
      .index = 0};
  if (image_create_info.format == vk::Format::eUndefined) {
    // Ensure that the image created matches what was asked for on
    // sysmem_connection.
    image_create_info.extent.width = 1024;
    image_create_info.extent.height = 1024;
    image_create_info.format = vk::Format::eB8G8R8A8Unorm;
  }
  image_create_info.pNext = &image_format_fuchsia;

  auto [result, vk_image] = ctx_->device()->createImageUnique(image_create_info, nullptr);
  if (result != vk::Result::eSuccess) {
    ADD_FAILURE() << "vkCreateImage() failed: " << vk::to_string(result);
    return false;
  }
  vk_image_ = std::move(vk_image);
  return true;
}

std::optional<uint32_t> VulkanExtensionTest::InitializeDirectImageMemory(
    vk::BufferCollectionFUCHSIA collection, uint32_t expected_count) {
  const vk::Device &device = *ctx_->device();
  VkMemoryRequirements requirements;
  vkGetImageMemoryRequirements(device, *vk_image_, &requirements);
  uint32_t memory_type;
  ValidateBufferProperties(requirements, collection, expected_count, &memory_type);

  vk::StructureChain<vk::MemoryAllocateInfo, vk::ImportMemoryBufferCollectionFUCHSIA,
                     vk::MemoryDedicatedAllocateInfoKHR>
      alloc_info(vk::MemoryAllocateInfo()
                     .setAllocationSize(requirements.size)
                     .setMemoryTypeIndex(memory_type),
                 vk::ImportMemoryBufferCollectionFUCHSIA().setCollection(collection).setIndex(0),
                 vk::MemoryDedicatedAllocateInfoKHR().setImage(*vk_image_).setBuffer(*vk_buffer_));

  auto [result, vk_device_memory] =
      ctx_->device()->allocateMemoryUnique(alloc_info.get<vk::MemoryAllocateInfo>());
  if (result != vk::Result::eSuccess) {
    ADD_FAILURE() << "allocateMemoryUnique() failed: " << vk::to_string(result);
    return std::nullopt;
  }
  vk_device_memory_ = std::move(vk_device_memory);
  vk_device_memory_size_ = requirements.size;

  auto bind_result = ctx_->device()->bindImageMemory(*vk_image_, *vk_device_memory_, 0u);
  if (bind_result != vk::Result::eSuccess) {
    ADD_FAILURE() << "vkBindImageMemory() failed: " << vk::to_string(bind_result);
    return std::nullopt;
  }
  return memory_type;
}

VulkanExtensionTest::UniqueBufferCollection VulkanExtensionTest::CreateVkBufferCollectionForImage(
    fuchsia::sysmem2::BufferCollectionTokenSyncPtr token,
    const vk::ImageFormatConstraintsInfoFUCHSIA constraints,
    vk::ImageConstraintsInfoFlagsFUCHSIA flags) {
  vk::BufferCollectionCreateInfoFUCHSIA import_info(token.Unbind().TakeChannel().release());
  auto [result, collection] =
      ctx_->device()->createBufferCollectionFUCHSIAUnique(import_info, nullptr, loader_);
  EXPECT_EQ(result, vk::Result::eSuccess);

  vk::ImageConstraintsInfoFUCHSIA constraints_info;
  constraints_info.formatConstraintsCount = 1;
  constraints_info.pFormatConstraints = &constraints;
  constraints_info.bufferCollectionConstraints.minBufferCount = 1;
  constraints_info.bufferCollectionConstraints.minBufferCountForCamping = 0;
  constraints_info.bufferCollectionConstraints.minBufferCountForSharedSlack = 0;
  constraints_info.flags = flags;

  result = ctx_->device()->setBufferCollectionImageConstraintsFUCHSIA(*collection, constraints_info,
                                                                      loader_);
  EXPECT_EQ(result, vk::Result::eSuccess);
  return std::move(collection);
}

bool VulkanExtensionTest::Exec(
    VkFormat format, uint32_t width, uint32_t height, bool linear,
    bool repeat_constraints_as_non_protected,
    const std::vector<fuchsia::sysmem2::ImageFormatConstraints> &format_constraints) {
  EXPECT_NE(format, VK_FORMAT_UNDEFINED);

  auto [local_token, vulkan_token, non_protected_token] = MakeSharedCollection<3>();

  // This bool suggests that we dup another token to set the same constraints, skipping protected
  // memory requirements. This emulates another participant which does not require protected memory.
  UniqueBufferCollection non_protected_collection;
  bool is_yuv = (format == VK_FORMAT_G8_B8R8_2PLANE_420_UNORM_KHR) ||
                (format == VK_FORMAT_G8_B8_R8_3PLANE_420_UNORM_KHR);
  if (repeat_constraints_as_non_protected) {
    vk::ImageFormatConstraintsInfoFUCHSIA constraints =
        GetDefaultImageFormatConstraintsInfo(is_yuv);
    constraints.imageCreateInfo =
        GetDefaultImageCreateInfo(/*use_protected_memory=*/false, format, width, height, linear);
    non_protected_collection = CreateVkBufferCollectionForImage(
        std::move(non_protected_token), constraints,
        vk::ImageConstraintsInfoFlagBitsFUCHSIA::eProtectedOptional);
  } else {
    // Release the token to prevent sysmem from waiting on it.
    non_protected_token->Release();
    non_protected_token = {};
  }

  auto image_create_info =
      GetDefaultImageCreateInfo(use_protected_memory_, format, width, height, linear);
  vk::ImageFormatConstraintsInfoFUCHSIA constraints = GetDefaultImageFormatConstraintsInfo(is_yuv);
  constraints.imageCreateInfo = image_create_info;
  UniqueBufferCollection collection =
      CreateVkBufferCollectionForImage(std::move(vulkan_token), constraints);

  std::optional<fuchsia::sysmem2::BufferCollectionConstraints> constraints_option;
  if (!format_constraints.empty()) {
    fuchsia::sysmem2::BufferCollectionConstraints constraints;
    // Use the other connection to specify the actual desired format and size,
    // which should be compatible with what the vulkan driver can use.
    constraints.mutable_usage()->set_vulkan(fuchsia::sysmem2::VULKAN_IMAGE_USAGE_TRANSFER_DST);
    // Try multiple format modifiers.
    for (uint32_t i = 0; i < format_constraints.size(); i++) {
      constraints.mutable_image_format_constraints()->emplace_back(
          fidl::Clone(format_constraints[i]));
    }
    constraints_option = std::move(constraints);
  }
  auto buffer_collection_info =
      AllocateSysmemCollection(std::move(constraints_option), std::move(local_token));
  constraints_option.reset();

  EXPECT_EQ(1u, buffer_collection_info.buffers().size());

  if (!InitializeDirectImage(*collection, image_create_info)) {
    ADD_FAILURE() << "InitializeDirectImage() failed";
    return false;
  }

  if (linear) {
    CheckLinearSubresourceLayout(format, width);
  }

  if (!InitializeDirectImageMemory(*collection)) {
    ADD_FAILURE() << "InitializeDirectImageMemory() failed";
    return false;
  }

  return true;
}

bool VulkanExtensionTest::ExecBuffer(uint32_t size) {
  VkResult result;
  const vk::Device &device = *ctx_->device();

  auto [local_token, vulkan_token] = MakeSharedCollection<2>();

  constexpr uint32_t kMinBufferCount = 2;

  vk::BufferCreateInfo buffer_create_info;
  buffer_create_info.flags =
      use_protected_memory_ ? vk::BufferCreateFlagBits::eProtected : vk::BufferCreateFlagBits();
  buffer_create_info.size = size;
  buffer_create_info.usage = vk::BufferUsageFlagBits::eTransferDst;
  buffer_create_info.sharingMode = vk::SharingMode::eExclusive;

  vk::BufferCollectionCreateInfoFUCHSIA import_info(vulkan_token.Unbind().TakeChannel().release());
  vk::BufferCollectionFUCHSIA collection;
  vk::Result result1 =
      ctx_->device()->createBufferCollectionFUCHSIA(&import_info, nullptr, &collection, loader_);
  if (result1 != vk::Result::eSuccess) {
    RTN_MSG(false, "Failed to create buffer collection: %d\n", static_cast<int>(result1));
  }

  vk::BufferConstraintsInfoFUCHSIA constraints;
  constraints.createInfo = buffer_create_info;
  constraints.requiredFormatFeatures = vk::FormatFeatureFlagBits::eVertexBuffer;
  constraints.bufferCollectionConstraints.minBufferCount = kMinBufferCount;

  result1 =
      ctx_->device()->setBufferCollectionBufferConstraintsFUCHSIA(collection, constraints, loader_);

  if (result1 != vk::Result::eSuccess) {
    RTN_MSG(false, "Failed to set buffer constraints: %d\n", static_cast<int>(result1));
  }

  auto buffer_collection_info = AllocateSysmemCollection({}, std::move(local_token));

  VkBufferCollectionBufferCreateInfoFUCHSIA collection_buffer_create_info = {
      .sType = VK_STRUCTURE_TYPE_BUFFER_COLLECTION_BUFFER_CREATE_INFO_FUCHSIA,
      .pNext = nullptr,
      .collection = collection,
      .index = 1};
  buffer_create_info.pNext = &collection_buffer_create_info;

  {
    auto [result, vk_buffer] = ctx_->device()->createBufferUnique(buffer_create_info, nullptr);

    if (result != vk::Result::eSuccess) {
      RTN_MSG(false, "vkCreateBuffer failed: %d\n", static_cast<int>(result));
    }
    vk_buffer_ = std::move(vk_buffer);
  }

  vk::MemoryRequirements requirements;
  ctx_->device()->getBufferMemoryRequirements(*vk_buffer_, &requirements);
  uint32_t memory_type;
  ValidateBufferProperties(requirements, collection, kMinBufferCount, &memory_type);
  vk::BufferCollectionPropertiesFUCHSIA properties;
  EXPECT_EQ(vk::Result::eSuccess,
            ctx_->device()->getBufferCollectionPropertiesFUCHSIA(collection, &properties, loader_));

  vk::StructureChain<vk::MemoryAllocateInfo, vk::ImportMemoryBufferCollectionFUCHSIA,
                     vk::MemoryDedicatedAllocateInfoKHR>
      alloc_info(vk::MemoryAllocateInfo()
                     .setAllocationSize(requirements.size)
                     .setMemoryTypeIndex(memory_type),
                 vk::ImportMemoryBufferCollectionFUCHSIA().setCollection(collection).setIndex(1),
                 vk::MemoryDedicatedAllocateInfoKHR().setImage(*vk_image_).setBuffer(*vk_buffer_));

  auto [vk_result, vk_device_memory] =
      ctx_->device()->allocateMemoryUnique(alloc_info.get<vk::MemoryAllocateInfo>());
  EXPECT_EQ(vk_result, vk::Result::eSuccess);
  vk_device_memory_ = std::move(vk_device_memory);

  result = vkBindBufferMemory(device, *vk_buffer_, *vk_device_memory_, 0u);
  if (result != VK_SUCCESS) {
    RTN_MSG(false, "vkBindBufferMemory failed: %d\n", result);
  }

  ctx_->device()->destroyBufferCollectionFUCHSIA(collection, nullptr, loader_);
  return true;
}

bool VulkanExtensionTest::IsMemoryTypeCoherent(uint32_t memoryTypeIndex) {
  vk::PhysicalDeviceMemoryProperties props = ctx_->physical_device().getMemoryProperties();
  assert(memoryTypeIndex < props.memoryTypeCount);
  return static_cast<bool>(props.memoryTypes[memoryTypeIndex].propertyFlags &
                           vk::MemoryPropertyFlagBits::eHostCoherent);
}

void VulkanExtensionTest::WriteImage(vk::DeviceMemory memory, bool is_coherent,
                                     VkDeviceSize size_in_bytes, uint32_t fill) {
  void *addr;
  vk::Result result =
      ctx_->device()->mapMemory(memory, 0 /* offset */, VK_WHOLE_SIZE, vk::MemoryMapFlags{}, &addr);
  ASSERT_EQ(vk::Result::eSuccess, result);

  VkDeviceSize word_count = size_in_bytes / sizeof(uint32_t);
  ASSERT_EQ(word_count * sizeof(uint32_t), size_in_bytes);

  for (uint32_t i = 0; i < word_count; i++) {
    reinterpret_cast<uint32_t *>(addr)[i] = fill;
  }

  if (!is_coherent) {
    auto range = vk::MappedMemoryRange().setMemory(memory).setSize(VK_WHOLE_SIZE);
    EXPECT_EQ(vk::Result::eSuccess, ctx_->device()->flushMappedMemoryRanges(1, &range));
  }

  ctx_->device()->unmapMemory(memory);
}

void VulkanExtensionTest::WriteLinearColorImageComplete(vk::DeviceMemory memory, vk::Image image,
                                                        bool is_coherent, uint32_t width,
                                                        uint32_t height, uint32_t fill) {
  void *addr;
  vk::Result result =
      ctx_->device()->mapMemory(memory, 0 /* offset */, VK_WHOLE_SIZE, vk::MemoryMapFlags{}, &addr);
  ASSERT_EQ(vk::Result::eSuccess, result);

  auto layout = ctx_->device()->getImageSubresourceLayout(
      image, vk::ImageSubresource(vk::ImageAspectFlagBits::eColor, 0, 0));

  for (uint32_t y = 0; y < height; y++) {
    for (uint32_t x = 0; x < width; x++) {
      uint8_t *ptr =
          static_cast<uint8_t *>(addr) + static_cast<size_t>(x * 4) + y * layout.rowPitch;
      *reinterpret_cast<uint32_t *>(ptr) = fill;
    }
  }

  if (!is_coherent) {
    auto range = vk::MappedMemoryRange().setMemory(memory).setSize(VK_WHOLE_SIZE);
    EXPECT_EQ(vk::Result::eSuccess, ctx_->device()->flushMappedMemoryRanges(1, &range));
  }

  ctx_->device()->unmapMemory(memory);
}

void VulkanExtensionTest::CheckLinearImage(vk::Image image, vk::DeviceMemory memory,
                                           bool is_coherent, uint32_t width, uint32_t height,
                                           uint32_t fill) {
  void *addr;
  vk::Result result =
      ctx_->device()->mapMemory(memory, 0 /* offset */, VK_WHOLE_SIZE, vk::MemoryMapFlags{}, &addr);
  ASSERT_EQ(vk::Result::eSuccess, result);

  if (!is_coherent) {
    auto range = vk::MappedMemoryRange().setMemory(memory).setSize(VK_WHOLE_SIZE);
    EXPECT_EQ(vk::Result::eSuccess, ctx_->device()->invalidateMappedMemoryRanges(1, &range));
  }

  vk::SubresourceLayout layout = vulkan_context().device()->getImageSubresourceLayout(
      image, vk::ImageSubresource(vk::ImageAspectFlagBits::eColor, 0, 0));
  vk::DeviceSize bytes_per_row = layout.rowPitch;

  uint32_t error_count = 0;
  constexpr uint32_t kMaxErrors = 10;
  for (uint32_t y = 0; y < height; y++) {
    auto row_ptr = reinterpret_cast<uint8_t *>(addr) + y * bytes_per_row;
    for (uint32_t x = 0; x < width; x++) {
      EXPECT_EQ(fill, reinterpret_cast<uint32_t *>(row_ptr)[x]) << "x " << x << " y " << y;
      if (reinterpret_cast<uint32_t *>(row_ptr)[x] != fill) {
        error_count++;
        if (error_count > kMaxErrors) {
          printf("Skipping reporting remaining errors\n");
          break;
        }
      }
    }
  }

  ctx_->device()->unmapMemory(memory);
}

// Return the byte offset of a pixel in an image.
size_t GetImageByteOffset(size_t x, size_t y, const fuchsia::sysmem2::BufferCollectionInfo &info,
                          size_t width, size_t height) {
  ZX_DEBUG_ASSERT(info.settings().has_image_format_constraints());
  auto &image_format_constraints = info.settings().image_format_constraints();
  constexpr uint32_t kBytesPerPixel = 4;
  if (image_format_constraints.pixel_format_modifier() ==
      fuchsia::images2::PixelFormatModifier::LINEAR) {
    size_t bytes_per_row = fbl::round_up(
        std::max(width * kBytesPerPixel,
                 static_cast<size_t>(image_format_constraints.has_min_bytes_per_row()
                                         ? image_format_constraints.min_bytes_per_row()
                                         : 0)),
        image_format_constraints.has_bytes_per_row_divisor()
            ? image_format_constraints.bytes_per_row_divisor()
            : 1);
    return y * bytes_per_row + x * kBytesPerPixel;
  } else {
    ZX_DEBUG_ASSERT(image_format_constraints.pixel_format_modifier() ==
                    fuchsia::images2::PixelFormatModifier::INTEL_I915_Y_TILED);
    constexpr uint32_t kTileWidthBytes = 128u;
    constexpr uint32_t kTileHeight = 32u;
    constexpr uint32_t kTileSizeBytes = kTileWidthBytes * kTileHeight;
    // Including padding.
    size_t width_in_tiles =
        fbl::round_up(
            std::max(width * kBytesPerPixel,
                     static_cast<size_t>(image_format_constraints.has_min_bytes_per_row()
                                             ? image_format_constraints.min_bytes_per_row()
                                             : 0)),
            image_format_constraints.has_bytes_per_row_divisor()
                ? image_format_constraints.bytes_per_row_divisor()
                : 1) /
        kTileWidthBytes;

    size_t x_in_bytes = x * kBytesPerPixel;

    size_t x_offset_in_tiles = x_in_bytes / kTileWidthBytes;
    size_t y_offset_in_tiles = y / kTileHeight;
    size_t x_offset_subtile = x_in_bytes % kTileWidthBytes;
    size_t y_offset_subtile = y % kTileHeight;

    size_t tile_index = y_offset_in_tiles * width_in_tiles + x_offset_in_tiles;
    // OWord is 16 bytes.
    constexpr uint32_t kSubtileColumnWidth = 16u;
    size_t subtile_column_offset = (x_offset_subtile / kSubtileColumnWidth) * kTileHeight;
    size_t subtile_line_offset = (subtile_column_offset + y_offset_subtile) * kSubtileColumnWidth;
    return tile_index * kTileSizeBytes + subtile_line_offset + x_in_bytes % kSubtileColumnWidth;
  }
}

// Check that entire 4 byte-per-pixel image is filled with a pattern.
void CheckImageFill(size_t width, size_t height, void *addr,
                    const fuchsia::sysmem2::BufferCollectionInfo &info, uint32_t fill) {
  uint32_t error_count = 0;
  constexpr uint32_t kMaxErrors = 10;
  for (size_t y = 0; y < height; y++) {
    for (size_t x = 0; x < width; x++) {
      size_t byte_offset = GetImageByteOffset(x, y, info, width, height);
      uint32_t *pixel_addr =
          reinterpret_cast<uint32_t *>(reinterpret_cast<uint8_t *>(addr) + byte_offset);
      EXPECT_EQ(fill, *pixel_addr) << "byte_offset " << byte_offset << " x " << x << " y " << y;
      if (*pixel_addr != fill) {
        error_count++;
        if (error_count > kMaxErrors) {
          printf("Skipping reporting remaining errors\n");
          return;
        }
      }
    }
  }
}
