// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/power-management/bandwidth-reservation-cache.h>

#include <gtest/gtest.h>

namespace {

using power_management::BandwidthReservationCache;
using power_management::Time;
using power_management::Utilization;

TEST(BandwidthReservationCacheTest, Add) {
  BandwidthReservationCache<4> bandwdith_reservation_cache;
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{0});

  // Fill up the cache with reservations. Since there are no evictions, no
  // deferred utilization should be removed in the process of adding an entry.
  EXPECT_EQ(bandwdith_reservation_cache.Add(1, Time{10}, Utilization{1}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(2, Time{20}, Utilization{2}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(3, Time{30}, Utilization{3}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(4, Time{40}, Utilization{4}), Utilization{0});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{10});

  // Adding another reservation with an earlier finish time than at least one
  // existing entry should evict the entry with the latest finish time and
  // return the reservation that was removed.
  EXPECT_EQ(bandwdith_reservation_cache.Add(5, Time{15}, Utilization{5}), Utilization{4});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{11});

  // Attempting to add another reservation with a later finish time than any
  // existing entry in the cache should have no effect.
  EXPECT_EQ(bandwdith_reservation_cache.Add(6, Time{60}, Utilization{6}), Utilization{6});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{11});

  // Adding an existing tid should update the entry and return the previous
  // reservation.
  EXPECT_EQ(bandwdith_reservation_cache.Add(5, Time{25}, Utilization{6}), Utilization{5});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{12});
}

TEST(BandwidthReservationCacheTest, Prune) {
  BandwidthReservationCache<4> bandwdith_reservation_cache;
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{0});

  EXPECT_EQ(bandwdith_reservation_cache.Add(1, Time{10}, Utilization{1}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(2, Time{20}, Utilization{2}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(3, Time{30}, Utilization{3}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(4, Time{40}, Utilization{4}), Utilization{0});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{10});

  // Pruning to earlier than any finish time should have no effect.
  EXPECT_EQ(bandwdith_reservation_cache.Prune(Time{5}), Utilization{0});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{10});

  EXPECT_EQ(bandwdith_reservation_cache.Prune(Time{10}), Utilization{1});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{9});

  EXPECT_EQ(bandwdith_reservation_cache.Prune(Time{25}), Utilization{2});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{7});

  // Pruning should remove all entries with earlier finish times.
  EXPECT_EQ(bandwdith_reservation_cache.Prune(Time{50}), Utilization{7});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{0});
}

TEST(BandwidthReservationCacheTest, Clear) {
  BandwidthReservationCache<4> bandwdith_reservation_cache;
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{0});

  EXPECT_EQ(bandwdith_reservation_cache.Add(1, Time{10}, Utilization{1}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(2, Time{20}, Utilization{2}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(3, Time{30}, Utilization{3}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(4, Time{40}, Utilization{4}), Utilization{0});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{10});

  // Clearing the cache should remove all reservations and deferred utilization.
  EXPECT_EQ(bandwdith_reservation_cache.Clear(), Utilization{10});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{0});
}

TEST(BandwidthReservationCacheTest, ClampToNextFinishTime) {
  BandwidthReservationCache<4> bandwdith_reservation_cache;
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{0});

  EXPECT_EQ(bandwdith_reservation_cache.Add(1, Time{10}, Utilization{1}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(2, Time{20}, Utilization{2}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(3, Time{30}, Utilization{3}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(4, Time{40}, Utilization{4}), Utilization{0});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{10});

  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{5}), Time{5});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{10}), Time{10});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{20}), Time{10});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{30}), Time{10});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{40}), Time{10});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{50}), Time{10});

  EXPECT_EQ(bandwdith_reservation_cache.Remove(1), Utilization{1});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{9});

  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{5}), Time{5});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{10}), Time{10});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{20}), Time{20});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{30}), Time{20});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{40}), Time{20});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{50}), Time{20});

  EXPECT_EQ(bandwdith_reservation_cache.Clear(), Utilization{9});

  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{5}), Time{5});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{10}), Time{10});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{20}), Time{20});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{30}), Time{30});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{40}), Time{40});
  EXPECT_EQ(bandwdith_reservation_cache.ClampToNextFinishTime(Time{50}), Time{50});
}

TEST(BandwidthReservationCacheTest, Mix) {
  BandwidthReservationCache<4> bandwdith_reservation_cache;
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{0});

  EXPECT_EQ(bandwdith_reservation_cache.Add(1, Time{10}, Utilization{1}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(2, Time{20}, Utilization{2}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(3, Time{30}, Utilization{3}), Utilization{0});
  EXPECT_EQ(bandwdith_reservation_cache.Add(4, Time{40}, Utilization{4}), Utilization{0});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{10});

  // Adding another reservation with an earlier finish time than at least one
  // existing entry should evict the entry with the latest finish time and
  // return the reservation that was removed.
  EXPECT_EQ(bandwdith_reservation_cache.Add(5, Time{15}, Utilization{5}), Utilization{4});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{11});

  // Attempting to add another reservation with a later finish time than any
  // existing entry in the cache should have no effect.
  EXPECT_EQ(bandwdith_reservation_cache.Add(6, Time{60}, Utilization{6}), Utilization{6});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{11});

  // Prune the reservations with finish times less than or equal to 20,
  // returning the total bandwidth reservation removed.
  EXPECT_EQ(bandwdith_reservation_cache.Prune(Time{20}), Utilization{8});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{3});

  // Removing an entry that is not in the cache should have no effect.
  EXPECT_EQ(bandwdith_reservation_cache.Remove(6), Utilization{0});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{3});

  // Removing an existing entry should return the reservation that was removed.
  EXPECT_EQ(bandwdith_reservation_cache.Remove(3), Utilization{3});
  ASSERT_EQ(bandwdith_reservation_cache.total_deferred_utilization(), Utilization{0});
}

}  // anonymous namespace
