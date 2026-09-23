// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_KERNEL_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_KERNEL_H_

// This file contains Fuchsia-specific kernel support code, including those kernel structs and
// routines that are typically provided by the Linux kernel API.

#include <lib/async/time.h>
#include <limits.h>
#include <netinet/if_ether.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/listnode.h>
#include <zircon/syscalls.h>
#include <zircon/time.h>

#include "third_party/iwlwifi/platform/banjo/common.h"
#include "third_party/iwlwifi/platform/compiler.h"
#include "third_party/iwlwifi/platform/ieee80211.h"

#if defined(__cplusplus)
extern "C" {
#endif  // defined(__cplusplus)

typedef uint32_t netdev_features_t;

typedef uint64_t dma_addr_t;

typedef char* acpi_string;

#define ETHTOOL_FWVERS_LEN 32

// IS_ENABLED is a macro for conditional compilation.
//
// The given compile flag shall be either:
//
//   + defined as empty string, or
//   + defined as 1
//
// in the BUILD.gn file.
//
#define __COND__ 0   // for the empty string case.
#define __COND__1 1  // for the =1 case.
#define POSTFIX(flag) __COND__##flag
#define IS_ENABLED(flag) POSTFIX(flag)

#define iwl_assert_lock_held(x) ZX_DEBUG_ASSERT(mtx_trylock(x) == thrd_busy)

// NEEDS_TYPES: how to guarantee this?
#define READ_ONCE(x) (x)

#define DMA_BIT_MASK(n) (((n) >= 64) ? ~0ULL : ((1ULL << (n)) - 1))

// Converts a WiFi time unit (1024 us) to zx_duration_t.
//
// "IEEE Std 802.11-2007" 2007-06-12. p. 14. Retrieved 2010-07-20.
// time unit (TU): A measurement of time equal to 1024 μs.
//
#define TU_TO_DURATION(time_unit) (ZX_USEC(1024) * time_unit)

// Convert a WiFi time unit (1024 us) to jiffies (assume 1000 Hz per second).
//
#define TU_TO_JIFFIES(tu) (TU_TO_DURATION(tu) / ZX_MSEC(1))

#define msecs_to_jiffies(ms) (ZX_MSEC(ms) / ZX_MSEC(1))

struct async_dispatcher;
struct driver_inspector;
struct rcu_manager;

// This struct is analogous to the Linux device struct, and contains all the Fuchsia-specific data
// fields relevant to generic device functionality.
struct device {
  void* load_firmware_ctx;
  zx_status_t (*load_firmware_callback)(void* ctx, const char* name, zx_handle_t* vmo,
                                        size_t* size);

  // The BTI handle used to map IO buffers for this device.
  zx_handle_t bti;

  // The dispatcher used to dispatch work queue tasks, equivalent to the Linux workqueue.  On Linux,
  // these are run in process context, in contrast with timer tasks that are run in interrupt
  // context.  Fuchsia drivers have no separate interrupt context, but to maintain similar
  // performance characteristics we will maintain a dedicated work queue dispatcher here.
  struct async_dispatcher* task_dispatcher;

  // The dispatcher used to dispatch IRQ tasks.  On Linux, these are run in interrupt context.
  // Fuchsia has no separate interrupt context, but to maintain similar performance characteristics
  // we will maintain a dedicated IRQ dispatcher here.
  struct async_dispatcher* irq_dispatcher;

  // The RCU manager instance used to manage RCU-based synchronization.
  struct rcu_manager* rcu_manager;

  // The inspector used to publish the component inspection tree from the driver.
  struct driver_inspector* inspector;
};

// This struct is analogous to the Linux pci_device_id struct, and contains PCI device ID values.
struct iwl_pci_device_id {
  uint32_t vendor;
  uint16_t device;
  uint32_t subvendor;
  uint32_t subdevice;
  unsigned long driver_data;
};

// An opaque struct that hides a C++ FIDL client, allowing us to use FIDL
// without rewriting the whole driver in C++.
struct iwl_pci_fidl;

// This struct is analogous to the Linux pci_dev struct, and contans Fuchsia-specific PCI bus
// interface data.
struct iwl_pci_dev {
  struct device dev;
  struct iwl_pci_fidl* fidl;
  unsigned short device;
  unsigned short subsystem_device;
  struct iwl_trans* drvdata;
};

// This struct holds a VMO reference to a firmware binary.
struct firmware {
  zx_handle_t vmo;
  uint8_t* data;
  size_t size;
};

struct page;

// NEEDS_TYPES: Below structures are only referenced in function prototype.
//                Doesn't need a dummy byte.
struct dentry;
struct wait_queue;
struct wiphy;

// NEEDS_TYPES: Below structures are used in code but not ported yet.
// A dummy byte is required to suppress the C++ warning message for empty
// struct.

struct work_struct {
  char dummy;
};

struct delayed_work {
  struct work_struct work;
};

struct ewma_rate {
  char dummy;
};

struct inet6_dev;

struct mac_address {
  uint8_t addr[ETH_ALEN];
};

struct napi_struct {
  char dummy;
};

struct rcu_head {
  char dummy;
};

struct sk_buff_head {
  char dummy;
};

struct wait_queue_head {
  char dummy;
};

struct wireless_dev {
  wlan_mac_role_t iftype;
};

struct cfg80211_sched_scan_plan {
  u32 interval;
  u32 iterations;
};

struct cfg80211_scan_6ghz_params {
  u32 short_ssid;
  u32 channel_idx;
  u8 bssid[ETH_ALEN];
  bool unsolicited_probe;
  bool short_ssid_valid;
  bool psc_no_listen;
};

////
// Typedefs
////
typedef struct wait_queue wait_queue_t;
typedef struct wait_queue_head wait_queue_head_t;

static inline void* vmalloc(unsigned long size) { return malloc(size); }

static inline void* kmemdup(const void* src, size_t len) {
  ZX_ASSERT(src);
  void* dst = malloc(len);
  if (dst) {
    memcpy(dst, src, len);
  }
  return dst;
}

static inline void vfree(const void* addr) { free((void*)addr); }

static inline void kfree(void* addr) { free(addr); }

static inline bool IS_ERR_OR_NULL(const void* ptr) {
  return !ptr || (((unsigned long)ptr) >= (unsigned long)-4095);
}

static inline void list_splice_after_tail(list_node_t* splice_from, list_node_t* pos) {
  if (list_is_empty(pos)) {
    list_move(splice_from, pos);
  } else {
    list_splice_after(splice_from, list_peek_tail(pos));
  }
}

// The following MAC addresses are invalid addresses.
//
//   00:00:00:00:00:00
//   *1:**:**:**:**:**  (multicast: the LSB of first byte is 1)
//   FF:FF:FF:FF:FF:FF  (also a nulticast address)
static inline bool is_valid_ether_addr(const uint8_t* mac) {
  return !((!mac[0] && !mac[1] && !mac[2] && !mac[3] && !mac[4] && !mac[5]) ||  // 00:00:00:00:00:00
           (mac[0] & 1));                                                       // multicast
}

// Fill the MAC address with broadcast address (all-0xff).
//
static inline void eth_broadcast_addr(uint8_t* addr) {
  for (size_t i = 0; i < ETH_ALEN; i++) {
    addr[i] = 0xff;
  }
}

static inline bool is_broadcast_addr(const uint8_t* mac) {
  uint8_t bcast[ETH_ALEN];
  eth_broadcast_addr(bcast);
  return !memcmp(bcast, mac, ETH_ALEN);
}

static inline bool is_multicast_addr(const uint8_t* mac) { return mac[0] & 1; }

// Fuchsia doesn't really have bottom-half to disable.
static inline void local_bh_disable(void) {}
static inline void local_bh_enable(void) {}

static inline unsigned int jiffies_to_msecs(zx_duration_t duration) {
  return (unsigned int)zx_nsec_from_duration(duration) / 1000 / 1000;
}

static inline void udelay(int usec) { zx_nanosleep(zx_deadline_after(ZX_USEC(usec))); }

static inline void mdelay(int msec) { zx_nanosleep(zx_deadline_after(ZX_MSEC(msec))); }

static inline void msleep(int msec) { zx_nanosleep(zx_deadline_after(ZX_MSEC(msec))); }

// We may redefine this struct to use `list_node_t` in the future.
// TODO(fxbug.dev/119415): clean-up after uprev.
struct list_head {
  char dummy;
};

// Returns the size of the given struct 'str' and its tailing variant-length array 'member' (which
// the element count is 'count').
//
// Note that the 'str' is NOT neccesary pointing to an existing instance. A random pointer (which
// its type is the the structure) is good enough.
//
#define struct_size(str, member, count) (sizeof(*str) + sizeof(*((str)->member)) * count)

// Fuchsia dones't support spin lock. We use the mutex lock as the workaround.
//
// spin_lock_bh is very similar to spin_lock, but used in the bottom-half task. It is to prevent
// a softirq running on the same CPU because it can introduce a dead-lock that the userspace program
// is waiting for a lock which already has been locked by the bottom-half.
//
// Since we don't use spin lock in fuchsia, there is no deadlock concern. So, we just use mtx_lock.
//
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wthread-safety-analysis"

static inline void spin_lock(mtx_t* lock) { mtx_lock(lock); }

static inline void spin_lock_bh(mtx_t* lock) { mtx_lock(lock); }

static inline void spin_unlock(mtx_t* lock) { mtx_unlock(lock); }

static inline void spin_unlock_bh(mtx_t* lock) { mtx_unlock(lock); }

// flags is not needed in Fuchsia. Discard it.
#define spin_lock_irqsave(lock, flags) \
  do {                                 \
    mtx_lock(lock);                    \
  } while (0)

#define spin_unlock_irqrestore(lock, flags) \
  do {                                      \
    mtx_unlock(lock);                       \
  } while (0)

#pragma GCC diagnostic pop

#define WARN_ONCE(condition, format...) \
  do {                                  \
    static bool warned = false;         \
    bool ret = !!(condition);           \
                                        \
    if (ret) {                          \
      if (!warned) {                    \
        IWL_WARN(NULL, format);         \
        warned = true;                  \
      }                                 \
    }                                   \
                                        \
    ret;                                \
  } while (0)

// Returns true if the 'chan' is Preferred Scanning Channels (PSC) in 6GHz.
static inline bool cfg80211_channel_is_psc(struct ieee80211_channel* chan) {
  if (chan->band != NL80211_BAND_6GHZ)
    return false;

  static uint8_t pscs[] = {
      5, 21, 37, 53, 69, 85, 101, 117, 133, 149, 165, 181, 197, 213, 229,
  };
  for (size_t i = 0; i < sizeof(pscs) / sizeof(pscs[0]); i++) {
    if (chan->hw_value == pscs[i])
      return true;
    if (chan->hw_value < pscs[i])
      return false;
  }
  return false;
}

typedef mtx_t spinlock_t;
#define spin_lock_bh(x) mtx_lock(x)
#define spin_unlock_bh(x) mtx_unlock(x)
#define mutex_lock(x) mtx_lock(x)
#define mutex_unlock(x) mtx_unlock(x)
#define lockdep_assert_held(x) \
  do {                         \
  } while (false)  // Do nothing for now.

#define GFP_KERNEL 0  // Any flag value is okay. Not used anyway.
#define kcalloc(num, size, flag) calloc(num, size)
#define kfree(p) free(p)

#if defined(__cplusplus)
}  // extern "C"
#endif  // defined(__cplusplus)

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_KERNEL_H_
