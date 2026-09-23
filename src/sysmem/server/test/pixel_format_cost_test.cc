// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <sys/stat.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/sysmem/server/usage_pixel_format_cost.h"

namespace sysmem_service {

namespace {

std::vector<uint8_t> LoadFile(const char* filename) {
  int fd = open(filename, O_RDONLY);
  struct stat file_stat;
  int fstat_result = fstat(fd, &file_stat);
  ZX_ASSERT(fstat_result == 0);
  size_t size = file_stat.st_size;
  std::vector<uint8_t> data(size);
  ssize_t read_size = read(fd, data.data(), data.size());
  ZX_ASSERT(read_size == static_cast<ssize_t>(size));
  close(fd);
  return data;
}

std::vector<fuchsia_sysmem2::FormatCostEntry> LoadCostEntries(std::vector<const char*> filenames) {
  std::vector<fuchsia_sysmem2::FormatCostEntry> result;

  for (auto cost_filename : filenames) {
    auto data = LoadFile(cost_filename);

    auto unpersist_result = fidl::Unpersist<fuchsia_sysmem2::FormatCosts>(data);
    ZX_ASSERT(unpersist_result.is_ok());
    auto& format_costs = unpersist_result.value();
    auto format_cost_entries = std::move(format_costs.format_costs().value());

    for (auto& entry : format_cost_entries) {
      result.emplace_back(std::move(entry));
    }
  }

  return result;
}

std::vector<fuchsia_sysmem2::FormatCostEntry> LoadArmCostEntries() {
  return LoadCostEntries(std::vector{
      "/pkg/data/format_costs/intel.format_costs_persistent_fidl",
      "/pkg/data/format_costs/arm_mali.format_costs_persistent_fidl",
      "/pkg/data/format_costs/video_decoder_nv12.format_costs_persistent_fidl",
  });
}

std::vector<fuchsia_sysmem2::FormatCostEntry> LoadGenericCostEntries() {
  return LoadCostEntries(std::vector{
      // For historical reasons the "intel" format costs are treated as "generic" for tests.
      "/pkg/data/format_costs/intel.format_costs_persistent_fidl",
  });
}

TEST(PixelFormatCost, Afbc) {
  UsagePixelFormatCost arm_costs(LoadArmCostEntries());
  UsagePixelFormatCost generic_costs(LoadGenericCostEntries());

  for (uint32_t is_x = 0; is_x < 2; ++is_x) {
    fuchsia_sysmem2::BufferCollectionConstraints constraints;
    constraints.image_format_constraints().emplace(2);
    {
      fuchsia_sysmem2::ImageFormatConstraints image_format_constraints;
      {
        image_format_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
        if (is_x) {
          image_format_constraints.is_alpha_present() = false;
        }
      }
      constraints.image_format_constraints()->at(0) = std::move(image_format_constraints);
    }
    {
      fuchsia_sysmem2::ImageFormatConstraints image_format_constraints;
      {
        image_format_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
        image_format_constraints.pixel_format_modifier() =
            fuchsia_images2::PixelFormatModifier::kArmAfbc32X8;
        if (is_x) {
          image_format_constraints.is_alpha_present() = false;
        }
      }
      constraints.image_format_constraints()->at(1) = std::move(image_format_constraints);
    }

    EXPECT_LT(0, arm_costs.Compare(constraints, 0, 1));
    EXPECT_GT(0, arm_costs.Compare(constraints, 1, 0));
    EXPECT_EQ(0, generic_costs.Compare(constraints, 0, 1));
    EXPECT_EQ(0, generic_costs.Compare(constraints, 1, 0));
  }
}

TEST(PixelFormatCost, IntelTiling) {
  UsagePixelFormatCost generic_costs(LoadGenericCostEntries());

  for (uint32_t is_x = 0; is_x < 2; ++is_x) {
    fuchsia_sysmem2::BufferCollectionConstraints constraints;

    constraints.image_format_constraints().emplace(2);
    fuchsia_images2::PixelFormatModifier tiling_types[] = {
        fuchsia_images2::PixelFormatModifier::kIntelI915XTiled,
        fuchsia_images2::PixelFormatModifier::kIntelI915YfTiled,
        fuchsia_images2::PixelFormatModifier::kIntelI915YTiled};
    for (auto modifier : tiling_types) {
      {
        fuchsia_sysmem2::ImageFormatConstraints image_format_constraints;
        {
          image_format_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
          image_format_constraints.pixel_format_modifier() =
              fuchsia_images2::PixelFormatModifier::kLinear;
          if (is_x) {
            image_format_constraints.is_alpha_present() = false;
          }
        }
        constraints.image_format_constraints()->at(0) = std::move(image_format_constraints);
      }
      {
        fuchsia_sysmem2::ImageFormatConstraints image_format_constraints;
        {
          image_format_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
          image_format_constraints.pixel_format_modifier() = modifier;
          if (is_x) {
            image_format_constraints.is_alpha_present() = false;
          }
        }
        constraints.image_format_constraints()->at(1) = std::move(image_format_constraints);
      }
      EXPECT_LT(0, generic_costs.Compare(constraints, 0, 1));
      EXPECT_GT(0, generic_costs.Compare(constraints, 1, 0));

      // Explicit linear should be treated the same as no format modifier value.
      constraints.image_format_constraints()->at(0).pixel_format_modifier() =
          fuchsia_images2::PixelFormatModifier::kLinear;

      EXPECT_LT(0, generic_costs.Compare(constraints, 0, 1));
      EXPECT_GT(0, generic_costs.Compare(constraints, 1, 0));

      // Explicit linear should be treated the same as no format modifier value.
      {
        constraints.image_format_constraints()->at(0).pixel_format() =
            fuchsia_images2::PixelFormat::kB8G8R8A8;
        if (is_x) {
          constraints.image_format_constraints()->at(0).is_alpha_present() = false;
        }
      }
      EXPECT_LT(0, generic_costs.Compare(constraints, 0, 1));
      EXPECT_GT(0, generic_costs.Compare(constraints, 1, 0));
    }

    // Formats are in ascending preference order (descending cost order).
    std::array modifier_list = {
        fuchsia_images2::PixelFormatModifier::kLinear,
        fuchsia_images2::PixelFormatModifier::kIntelI915XTiled,
        fuchsia_images2::PixelFormatModifier::kIntelI915YTiled,
        fuchsia_images2::PixelFormatModifier::kIntelI915YfTiled,
        fuchsia_images2::PixelFormatModifier::kIntelI915YTiledCcs,
        fuchsia_images2::PixelFormatModifier::kIntelI915YfTiledCcs,
    };
    constraints.image_format_constraints().emplace(modifier_list.size());

    for (uint32_t i = 0; i < modifier_list.size(); ++i) {
      {
        fuchsia_sysmem2::ImageFormatConstraints image_format_constraints;
        {
          image_format_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
          image_format_constraints.pixel_format_modifier() = modifier_list[i];
          if (is_x) {
            image_format_constraints.is_alpha_present() = false;
          }
        }
        constraints.image_format_constraints()->at(i) = std::move(image_format_constraints);
      }
    }

    for (uint32_t i = 1; i < modifier_list.size(); ++i) {
      EXPECT_LT(0, generic_costs.Compare(constraints, i - 1, i)) << "i=" << i;
      EXPECT_GT(0, generic_costs.Compare(constraints, i, i - 1)) << "i=" << i;
    }
  }
}

TEST(PixelFormatCost, ArmTransactionElimination) {
  auto arm_costs = UsagePixelFormatCost(LoadArmCostEntries());
  auto generic_costs = UsagePixelFormatCost(LoadGenericCostEntries());

  for (uint32_t is_x = 0; is_x < 2; ++is_x) {
    fuchsia_sysmem2::BufferCollectionConstraints constraints;
    constraints.image_format_constraints().emplace(2);
    {
      fuchsia_sysmem2::ImageFormatConstraints image_format_constraints;
      {
        image_format_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
        image_format_constraints.pixel_format_modifier() =
            fuchsia_images2::PixelFormatModifier::kArmAfbc32X8;
        if (is_x) {
          image_format_constraints.is_alpha_present() = false;
        }
      }
      constraints.image_format_constraints()->at(0) = std::move(image_format_constraints);
    }
    {
      fuchsia_sysmem2::ImageFormatConstraints image_format_constraints;
      {
        image_format_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
        image_format_constraints.pixel_format_modifier() =
            fuchsia_images2::PixelFormatModifier::kArmAfbc32X8Te;
        if (is_x) {
          image_format_constraints.is_alpha_present() = false;
        }
      }
      constraints.image_format_constraints()->at(1) = std::move(image_format_constraints);
    }

    EXPECT_LT(0, arm_costs.Compare(constraints, 0, 1));
    EXPECT_GT(0, arm_costs.Compare(constraints, 1, 0));
    EXPECT_EQ(0, generic_costs.Compare(constraints, 0, 1));
    EXPECT_EQ(0, generic_costs.Compare(constraints, 1, 0));
  }
}

TEST(PixelFormatCost, AfbcWithFlags) {
  auto arm_costs = UsagePixelFormatCost(LoadArmCostEntries());
  auto generic_costs = UsagePixelFormatCost(LoadGenericCostEntries());

  for (uint32_t is_x = 0; is_x < 2; ++is_x) {
    // Formats are in ascending preference order (descending cost order).
    std::array modifier_list = {
        fuchsia_images2::PixelFormatModifier::kLinear,
        fuchsia_images2::PixelFormatModifier::kArmAfbc16X16,
        fuchsia_images2::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuv,
        fuchsia_images2::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTiledHeader,
        fuchsia_images2::PixelFormatModifier::kArmAfbc16X16Te,
        fuchsia_images2::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTe,
        fuchsia_images2::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTeTiledHeader,
    };
    fuchsia_sysmem2::BufferCollectionConstraints constraints;
    constraints.image_format_constraints().emplace(modifier_list.size());

    for (uint32_t i = 0; i < modifier_list.size(); ++i) {
      {
        fuchsia_sysmem2::ImageFormatConstraints image_format_constraints;
        {
          image_format_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
          image_format_constraints.pixel_format_modifier() = modifier_list[i];
          if (is_x) {
            image_format_constraints.is_alpha_present() = false;
          }
        }
        constraints.image_format_constraints()->at(i) = std::move(image_format_constraints);
      }
    }

    for (uint32_t i = 1; i < modifier_list.size(); ++i) {
      EXPECT_LT(0, arm_costs.Compare(constraints, i - 1, i)) << "i=" << i;
      EXPECT_GT(0, arm_costs.Compare(constraints, i, i - 1)) << "i=%d" << i;
      EXPECT_EQ(0, generic_costs.Compare(constraints, i - 1, i));
      EXPECT_EQ(0, generic_costs.Compare(constraints, i, i - 1));
    }
  }
}

}  // namespace
}  // namespace sysmem_service
