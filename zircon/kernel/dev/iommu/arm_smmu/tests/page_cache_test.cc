// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <string.h>

#include <dev/arm_smmu/page_cache.h>
#include <zxtest/zxtest.h>

namespace arm_smmu {
namespace {

bool IsPageZeroed(const vm_page_t* page) {
  auto* p = static_cast<const uint8_t*>(paddr_to_physmap(page->paddr()));
  for (size_t i = 0; i < kPageSize; ++i) {
    if (p[i] != 0)
      return false;
  }
  return true;
}

TEST(PageCacheTest, AddAndGet) {
  PageCache cache;

  EXPECT_EQ(0, cache.in_flight_pages());
  EXPECT_EQ(0, cache.cache_entries());

  auto result1 = cache.GetPage();
  ASSERT_TRUE(result1.is_ok());
  vm_page_t* page1 = result1.value();
  EXPECT_TRUE(IsPageZeroed(page1));
  EXPECT_EQ(1, cache.in_flight_pages());
  EXPECT_EQ(0, cache.cache_entries());

  auto result2 = cache.GetPage();
  ASSERT_TRUE(result2.is_ok());
  vm_page_t* page2 = result2.value();
  EXPECT_TRUE(IsPageZeroed(page2));
  EXPECT_EQ(2, cache.in_flight_pages());
  EXPECT_EQ(0, cache.cache_entries());

  cache.ReturnPage(page1);
  EXPECT_EQ(1, cache.in_flight_pages());
  EXPECT_EQ(1, cache.cache_entries());

  cache.ReturnPage(page2);
  EXPECT_EQ(0, cache.in_flight_pages());
  EXPECT_EQ(2, cache.cache_entries());

  auto get_result1 = cache.GetPage();
  ASSERT_TRUE(get_result1.is_ok());
  EXPECT_EQ(get_result1.value(), page2);  // We expect stack behavior, not queue behavior.
  EXPECT_TRUE(IsPageZeroed(get_result1.value()));
  EXPECT_EQ(1, cache.in_flight_pages());
  EXPECT_EQ(1, cache.cache_entries());

  auto get_result2 = cache.GetPage();
  ASSERT_TRUE(get_result2.is_ok());
  EXPECT_EQ(get_result2.value(), page1);  // We expect stack behavior, not queue behavior.
  EXPECT_TRUE(IsPageZeroed(get_result2.value()));
  EXPECT_EQ(2, cache.in_flight_pages());
  EXPECT_EQ(0, cache.cache_entries());

  // Cleanup: return to cache and Trim(0)
  cache.ReturnPage(page1);
  EXPECT_EQ(1, cache.in_flight_pages());
  EXPECT_EQ(1, cache.cache_entries());

  cache.ReturnPage(page2);
  EXPECT_EQ(0, cache.in_flight_pages());
  EXPECT_EQ(2, cache.cache_entries());

  cache.Trim(0);
  EXPECT_EQ(0, cache.in_flight_pages());
  EXPECT_EQ(0, cache.cache_entries());

  EXPECT_EQ(0u, PmmMock::Get().GetAllocatedPageCount());
}

TEST(PageCacheTest, Trim) {
  PageCache cache;

  vm_page_t* pages[5];
  for (int i = 0; i < 5; ++i) {
    auto result = cache.GetPage();
    ASSERT_TRUE(result.is_ok());
    pages[i] = result.value();
    EXPECT_TRUE(IsPageZeroed(pages[i]));
    EXPECT_EQ(i + 1, cache.in_flight_pages());
    EXPECT_EQ(0, cache.cache_entries());
  }

  for (int i = 0; i < 5;) {
    cache.ReturnPage(pages[i++]);
    EXPECT_EQ(5 - i, cache.in_flight_pages());
    EXPECT_EQ(i, cache.cache_entries());
  }

  for (int i = 5; i > 0;) {
    --i;
    cache.Trim(i);
    EXPECT_EQ(0, cache.in_flight_pages());
    EXPECT_EQ(i, cache.cache_entries());
  }

  EXPECT_EQ(0u, PmmMock::Get().GetAllocatedPageCount());
}

}  // namespace
}  // namespace arm_smmu
