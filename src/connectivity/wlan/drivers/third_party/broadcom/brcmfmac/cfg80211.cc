/*
 * Copyright (c) 2010 Broadcom Corporation
 *
 * Permission to use, copy, modify, and/or distribute this software for any
 * purpose with or without fee is hereby granted, provided that the above
 * copyright notice and this permission notice appear in all copies.
 *
 * THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
 * WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
 * MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
 * SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
 * WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
 * OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
 * CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */

/* Toplevel file. Relies on dhd_linux.c to send commands to the dongle. */

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/cfg80211.h"

#include <lib/fdf/dispatcher.h>
#include <lib/zx/clock.h>
#include <netinet/if_ether.h>
#include <stdlib.h>
#include <string.h>
#include <threads.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <algorithm>
#include <array>
#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <optional>
#include <vector>

#include <wlan/common/ieee80211.h>
#include <wlan/drivers/macaddr.h>

#include "fidl/fuchsia.wlan.common/cpp/natural_types.h"
#include "fidl/fuchsia.wlan.fullmac/cpp/natural_types.h"
#include "fidl/fuchsia.wlan.fullmac/cpp/wire_types.h"
#include "fidl/fuchsia.wlan.ieee80211/cpp/common_types.h"
#include "fidl/fuchsia.wlan.ieee80211/cpp/wire_types.h"
#include "fuchsia/wlan/ieee80211/cpp/fidl.h"
#include "lib/fidl/cpp/wire/vector_view.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/bcdc.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/bits.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/brcmu_d11.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/brcmu_utils.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/brcmu_wifi.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/btcoex.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/common.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/core.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/debug.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/defs.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/device.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/feature.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/fweh.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/fwil.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/fwil_types.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/inspect/device_inspect.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/linuxisms.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/proto.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/stats.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/workqueue.h"
#include "src/devices/lib/broadcom/commands.h"
#include "third_party/bcmdhd/crossdriver/bcmwifi_channels.h"
#include "third_party/bcmdhd/crossdriver/dhd.h"
#include "third_party/bcmdhd/crossdriver/include/devctrl_if/wlioctl_defs.h"
#include "third_party/bcmdhd/crossdriver/include/proto/802.11.h"
#include "third_party/bcmdhd/crossdriver/wlioctl.h"
#include "wlan/drivers/log.h"
#include "zircon/compiler.h"
#include "zircon/types.h"

#define BRCMF_SCAN_JOIN_ACTIVE_DWELL_TIME_MS 320
#define BRCMF_SCAN_JOIN_PASSIVE_DWELL_TIME_MS 400
#define BRCMF_SCAN_JOIN_PROBE_INTERVAL_MS 20

#define BRCMF_SCAN_CHANNEL_TIME 40
#define BRCMF_SCAN_UNASSOC_TIME 40
#define BRCMF_SCAN_PASSIVE_TIME 120

#define BRCMF_ND_INFO_TIMEOUT_MSEC 2000
// Wait until disconnect is complete. This is somewhat arbitrary. The disconnect process
// involves issuing a few iovars to FW in addition to notifying SME. But SME sometimes
// attempts to reconnect right away and that might preempt the disconnect.
#define BRCMF_WAIT_FOR_DISCONNECT_MSEC ZX_MSEC(500)
// Rate returned by FW (in units of Mbps) is multiplied by 2 to avoid passing fractional value
#define BRCMF_CONVERT_TO_REAL_RATE(fw_rate) (fw_rate / 2.0)

#define EXEC_TIMEOUT_WORKER(worker)                                       \
  {                                                                       \
    if (brcmf_bus_get_bus_type(cfg->pub->bus_if) == BRCMF_BUS_TYPE_SIM) { \
      (*cfg->worker.handler)(&cfg->worker);                               \
    } else {                                                              \
      cfg->pub->default_wq.Schedule(&cfg->worker);                        \
    }                                                                     \
  }

namespace fuchsia_wlan_ieee80211_wire = fuchsia_wlan_ieee80211::wire;
using ::wlan::common::MacAddr;

static bool check_vif_up(struct brcmf_cfg80211_vif* vif) {
  if (!brcmf_test_bit(brcmf_vif_status_bit_t::READY, &vif->sme_state)) {
    BRCMF_INFO("device is not ready : status (%lu)", vif->sme_state.load());
    return false;
  }
  return true;
}

static uint8_t __wl_rates[] = {
    BRCM_RATE_1M,  BRCM_RATE_2M,  BRCM_RATE_5M5, BRCM_RATE_11M, BRCM_RATE_6M,  BRCM_RATE_9M,
    BRCM_RATE_12M, BRCM_RATE_18M, BRCM_RATE_24M, BRCM_RATE_36M, BRCM_RATE_48M, BRCM_RATE_54M,
};

#define wl_g_rates (__wl_rates + 0)
#define wl_g_rates_size std::size(__wl_rates)
#define wl_a_rates (__wl_rates + 4)
#define wl_a_rates_size ((size_t)(wl_g_rates_size - 4))

/* Vendor specific ie. id = 221, oui and type defines exact ie */
struct brcmf_vs_tlv {
  uint8_t id;
  uint8_t len;
  uint8_t oui[3];
  uint8_t oui_type;
};

struct parsed_vndr_ie_info {
  uint8_t* ie_ptr;
  uint32_t ie_len; /* total length including id & length field */
  struct brcmf_vs_tlv vndrie;
};

struct parsed_vndr_ies {
  uint32_t count;
  struct parsed_vndr_ie_info ie_info[VNDR_IE_PARSE_LIMIT];
};

#define X(SCAN_STATUS)                       \
  case brcmf_scan_status_bit_t::SCAN_STATUS: \
    return #SCAN_STATUS;
const char* brcmf_get_scan_status_bit_str(brcmf_scan_status_bit_t scan_status) {
  switch (scan_status) { BRCMF_SCAN_STATUS_LIST };
}
#undef X

#define X(CONNECT_STATUS)                      \
  case brcmf_connect_status_t::CONNECT_STATUS: \
    return #CONNECT_STATUS;
const char* brcmf_get_connect_status_str(brcmf_connect_status_t connect_status) {
  switch (connect_status) { BRCMF_CONNECT_STATUS_LIST };
}
#undef X

static inline void fill_with_broadcast_addr(uint8_t* address) { memset(address, 0xff, ETH_ALEN); }
static inline void fill_with_wildcard_ssid(struct brcmf_ssid_le* ssid_le) { *ssid_le = {}; }

/* Traverse a string of 1-byte tag/1-byte length/variable-length value
 * triples, returning a pointer to the substring whose first element
 * matches tag
 */
static const struct brcmf_tlv* brcmf_parse_tlvs(const void* buf, int buflen, uint key) {
  const struct brcmf_tlv* elt = static_cast<decltype(elt)>(buf);
  int totlen = buflen;

  /* find tagged parameter */
  while (totlen >= TLV_HDR_LEN) {
    int len = elt->len;

    /* validate remaining totlen */
    if ((elt->id == key) && (totlen >= (len + TLV_HDR_LEN))) {
      return elt;
    }

    elt = (struct brcmf_tlv*)((uint8_t*)elt + (len + TLV_HDR_LEN));
    totlen -= (len + TLV_HDR_LEN);
  }

  return nullptr;
}

static zx_status_t brcmf_vif_change_validate(struct brcmf_cfg80211_info* cfg,
                                             struct brcmf_cfg80211_vif* vif,
                                             fuchsia_wlan_common::WlanMacRole new_type) {
  if (new_type.IsUnknown()) {
    BRCMF_ERR("Invalid mac role %u", static_cast<uint32_t>(new_type));
    return ZX_ERR_INVALID_ARGS;
  }

  struct brcmf_cfg80211_vif* pos;
  bool check_combos = false;
  zx_status_t ret = ZX_OK;
  struct iface_combination_params params = {
      .num_different_channels = 1,
  };

  list_for_every_entry (&cfg->vif_list, pos, struct brcmf_cfg80211_vif, list) {
    if (pos == vif) {
      params.iftype_num[uint32_t(new_type)]++;
    } else {
      /* concurrent interfaces so need check combinations */
      check_combos = true;
      params.iftype_num[uint32_t(pos->wdev.iftype)]++;
    }
  }

  if (check_combos) {
    ret = cfg80211_check_combinations(cfg, &params);
  }

  return ret;
}

static zx_status_t brcmf_vif_add_validate(struct brcmf_cfg80211_info* cfg,
                                          fuchsia_wlan_common::WlanMacRole new_type) {
  if (new_type.IsUnknown()) {
    BRCMF_ERR("Invalid mac role %u", static_cast<uint32_t>(new_type));
    return ZX_ERR_INVALID_ARGS;
  }

  struct brcmf_cfg80211_vif* pos;
  struct iface_combination_params params = {
      .num_different_channels = 1,
  };

  list_for_every_entry (&cfg->vif_list, pos, struct brcmf_cfg80211_vif, list) {
    params.iftype_num[uint32_t(pos->wdev.iftype)]++;
  }

  params.iftype_num[uint32_t(new_type)]++;
  return cfg80211_check_combinations(cfg, &params);
}

static void convert_key_from_CPU(struct brcmf_wsec_key* key, struct brcmf_wsec_key_le* key_le) {
  key_le->index = key->index;
  key_le->len = key->len;
  key_le->algo = key->algo;
  key_le->flags = key->flags;
  key_le->rxiv.hi = key->rxiv.hi;
  key_le->rxiv.lo = key->rxiv.lo;
  key_le->iv_initialized = key->iv_initialized;
  memcpy(key_le->data, key->data, sizeof(key->data));
  memcpy(key_le->ea, key->ea, sizeof(key->ea));
}

static zx_status_t send_key_to_dongle(struct brcmf_if* ifp, struct brcmf_wsec_key* key) {
  zx_status_t err;
  struct brcmf_wsec_key_le key_le;

  convert_key_from_CPU(key, &key_le);

  brcmf_netdev_wait_pend8021x(ifp);

  err = brcmf_fil_bsscfg_data_set(ifp, "wsec_key", &key_le, sizeof(key_le));

  if (err != ZX_OK) {
    BRCMF_ERR("wsec_key error (%d)", err);
  }
  return err;
}

static void brcmf_cfg80211_update_proto_addr_mode(struct wireless_dev* wdev) {
  struct brcmf_cfg80211_vif* vif;
  struct brcmf_if* ifp;

  vif = containerof(wdev, struct brcmf_cfg80211_vif, wdev);
  ifp = vif->ifp;

  if (wdev->iftype == fuchsia_wlan_common::WlanMacRole::kAp) {
    brcmf_proto_configure_addr_mode(ifp->drvr, ifp->ifidx, ADDR_DIRECT);
  } else {
    brcmf_proto_configure_addr_mode(ifp->drvr, ifp->ifidx, ADDR_INDIRECT);
  }
}

static int32_t brcmf_get_first_free_bsscfgidx(struct brcmf_pub* drvr) {
  int bsscfgidx;

  for (bsscfgidx = 0; bsscfgidx < BRCMF_MAX_IFS; bsscfgidx++) {
    /* bsscfgidx 1 is reserved for legacy P2P */
    if (bsscfgidx == 1) {
      continue;
    }
    if (!drvr->iflist[bsscfgidx]) {
      return bsscfgidx;
    }
  }

  return -1;
}

static int32_t brcmf_get_prealloced_bsscfgidx(struct brcmf_pub* drvr) {
  int bsscfgidx;
  net_device* ndev;

  for (bsscfgidx = 0; bsscfgidx < BRCMF_MAX_IFS; bsscfgidx++) {
    /* bsscfgidx 1 is reserved for legacy P2P */
    if (bsscfgidx == 1) {
      continue;
    }
    if (drvr->iflist[bsscfgidx]) {
      ndev = drvr->iflist[bsscfgidx]->ndev;
      if (ndev && ndev->needs_free_net_device) {
        return bsscfgidx;
      }
    }
  }

  return -1;
}

static zx_status_t brcmf_cfg80211_request_ap_if(struct brcmf_if* ifp) {
  struct brcmf_mbss_ssid_le mbss_ssid_le;
  int bsscfgidx;
  zx_status_t err;

  memset(&mbss_ssid_le, 0, sizeof(mbss_ssid_le));
  bsscfgidx = brcmf_get_first_free_bsscfgidx(ifp->drvr);
  if (bsscfgidx < 0) {
    return ZX_ERR_NO_MEMORY;
  }

  mbss_ssid_le.bsscfgidx = bsscfgidx;
  mbss_ssid_le.SSID_len = 5;
  sprintf((char*)mbss_ssid_le.SSID, "ssid%d", bsscfgidx);

  err = brcmf_fil_bsscfg_data_set(ifp, "bsscfg:ssid", &mbss_ssid_le, sizeof(mbss_ssid_le));
  if (err != ZX_OK) {
    BRCMF_ERR("setting ssid failed %d", err);
  }

  return err;
}

/*For now this function should always be called when adding iface*/
static zx_status_t brcmf_set_iface_macaddr(net_device* ndev, const MacAddr& mac_addr) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  bcme_status_t fw_err = BCME_OK;
  zx_status_t err = ZX_OK;

  BRCMF_DBG(TRACE, "Enter");
  // If the existing mac_addr of this iface is the same as it is, just return success.
  if (!memcmp(ifp->mac_addr, mac_addr.byte, ETH_ALEN)) {
    return ZX_OK;
  }

  err = brcmf_fil_iovar_data_set(ifp, "cur_etheraddr", mac_addr.byte, ETH_ALEN, &fw_err);
  if (err != ZX_OK) {
    BRCMF_ERR("Setting mac address failed: %s, fw err %s", zx_status_get_string(err),
              brcmf_fil_get_errstr(fw_err));
    return err;
  }

  BRCMF_INFO("Setting mac address of ndev:%s.", ifp->ndev->name);
#if !defined(NDEBUG)
  BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC, FMT_MAC_ARGS(mac_addr.byte));
#endif /* !defined(NDEBUG) */

  memcpy(ifp->mac_addr, mac_addr.byte, sizeof(ifp->mac_addr));

  return err;
}

// Derive the mac address for the SoftAP interface from the system mac address
// (which is used for the client interface).
zx_status_t brcmf_gen_ap_macaddr(struct brcmf_if* ifp, MacAddr& out_mac_addr) {
  bcme_status_t fw_err = BCME_OK;
  uint8_t gen_mac_addr[ETH_ALEN];

  zx_status_t err = brcmf_fil_iovar_data_get(ifp, "cur_etheraddr", gen_mac_addr, ETH_ALEN, &fw_err);
  if (err != ZX_OK) {
    BRCMF_ERR("Retrieving mac address from firmware failed: %s, fw err %s",
              zx_status_get_string(err), brcmf_fil_get_errstr(fw_err));
    return err;
  }

  // Modify the mac address as follows:
  // Mark the address as unicast and locally administered. In addition, modify
  // byte 5 (increment) to ensure that it is different from the original address
  gen_mac_addr[0] &= 0xfe;  // bit 0: 0 = unicast
  gen_mac_addr[0] |= 0x02;  // bit 1: 1 = locally-administered
  gen_mac_addr[5]++;

  out_mac_addr.Set(gen_mac_addr);
  return ZX_OK;
}

static zx_status_t brcmf_set_ap_macaddr(struct brcmf_if* ifp,
                                        const std::optional<MacAddr>& in_mac_addr) {
  MacAddr mac_addr;
  zx_status_t err = ZX_OK;

  // Use the provided mac_addr if it passed.
  if (in_mac_addr) {
    mac_addr = *in_mac_addr;
  } else {
    // If MAC address is not provided, we generate one using the current MAC address.
    // By default it is derived from the system mac address set during init.
    err = brcmf_gen_ap_macaddr(ifp, mac_addr);
    if (err != ZX_OK) {
      BRCMF_ERR("Failed to generate MAC address for AP iface netdev: %s", ifp->ndev->name);
      return err;
    }
  }

  err = brcmf_set_iface_macaddr(ifp->ndev, mac_addr);
  if (err != ZX_OK) {
    BRCMF_ERR("Failed to set MAC address for AP iface netdev: %s.", ifp->ndev->name);
#if !defined(NDEBUG)
    BRCMF_DBG(WLANIF, "  address: " FMT_MAC, FMT_MAC_ARGS(mac_addr.byte));
#endif /* !defined(NDEBUG) */

    return err;
  }

  return ZX_OK;
}

static zx_status_t brcmf_cfg80211_change_iface_mac_role(struct brcmf_cfg80211_info* cfg,
                                                        struct net_device* ndev,
                                                        fuchsia_wlan_common::WlanMacRole mac_role) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_vif* vif = ifp->vif;
  const char* mac_role_str = "OTHER";
  zx_status_t err = ZX_OK;
  bcme_status_t fw_err = BCME_OK;

  BRCMF_DBG(TRACE, "Enter");

  err = brcmf_vif_change_validate(cfg, vif, mac_role);
  if (err != ZX_OK) {
    BRCMF_ERR("iface validation failed: err=%d", err);
    return err;
  }
  switch (mac_role) {
    case fuchsia_wlan_common::WlanMacRole::kClient:
      err = brcmf_fil_cmd_int_set(ifp, BRCMF_C_SET_INFRA, 1, &fw_err);
      if (err != ZX_OK) {
        BRCMF_ERR("WLC_SET_INFRA error: %s, fw err %s", zx_status_get_string(err),
                  brcmf_fil_get_errstr(fw_err));
        err = ZX_ERR_UNAVAILABLE;
        goto done;
      }
      mac_role_str = const_cast<char*>("CLIENT");
      break;
    case fuchsia_wlan_common::WlanMacRole::kAp:
      mac_role_str = const_cast<char*>("AP");
      break;
    default:
      err = ZX_ERR_OUT_OF_RANGE;
      goto done;
  }

  vif->wdev.iftype = mac_role;
  brcmf_cfg80211_update_proto_addr_mode(&vif->wdev);
  BRCMF_IFDBG(WLANIF, ndev, "iface mac role set to \"%s\"", mac_role_str);

done:
  BRCMF_DBG(TRACE, "Exit");

  return err;
}

/**
 * brcmf_ap_add_vif() - create a new AP virtual interface for multiple BSS
 *
 * @cfg: config of new interface.
 * @name: name of the new interface.
 * @dev_out: address of wireless dev pointer
 */
static zx_status_t brcmf_ap_add_vif(struct brcmf_cfg80211_info* cfg, const char* name,
                                    const std::optional<MacAddr>& mac_addr,
                                    struct wireless_dev** dev_out) {
  struct brcmf_if* ifp = cfg_to_if(cfg);
  struct brcmf_cfg80211_vif* vif;
  zx_status_t err;

  // We need to create the SoftAP IF if we are not operating with manufacturing FW.
  if (!brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
    if (brcmf_cfg80211_vif_event_armed(cfg)) {
      return ZX_ERR_UNAVAILABLE;
    }

    BRCMF_INFO("Adding vif \"%s\"", name);

    err = brcmf_alloc_vif(cfg, fuchsia_wlan_common::WlanMacRole::kAp, &vif);
    if (err != ZX_OK) {
      if (dev_out) {
        *dev_out = nullptr;
      }
      return err;
    }

    brcmf_cfg80211_arm_vif_event(cfg, vif, BRCMF_E_IF_ADD);

    err = brcmf_cfg80211_request_ap_if(ifp);
    if (err != ZX_OK) {
      brcmf_cfg80211_disarm_vif_event(cfg);
      goto fail;
    }
    /* wait for firmware event */
    err = brcmf_cfg80211_wait_vif_event(cfg, ZX_MSEC(BRCMF_VIF_EVENT_TIMEOUT_MSEC));
    brcmf_cfg80211_disarm_vif_event(cfg);
    if (err != ZX_OK) {
      BRCMF_ERR("timeout occurred");
      err = ZX_ERR_IO;
      goto fail;
    }
  } else {
    // Else reuse the existing IF itself but change its type
    vif = ifp->vif;
    vif->ifp = ifp;
    err =
        brcmf_cfg80211_change_iface_mac_role(cfg, ifp->ndev, fuchsia_wlan_common::WlanMacRole::kAp);
    if (err != ZX_OK) {
      BRCMF_ERR("failed to change iface mac role to AP: %s", zx_status_get_string(err));
      err = ZX_ERR_IO;
      goto fail;
    }
  }

  /* interface created in firmware */
  ifp = vif->ifp;
  if (!ifp) {
    BRCMF_ERR("no if pointer provided");
    err = ZX_ERR_INVALID_ARGS;
    goto fail;
  }

  strncpy(ifp->ndev->name, name, sizeof(ifp->ndev->name) - 1);
  err = brcmf_net_attach(ifp, true);
  if (err != ZX_OK) {
    BRCMF_ERR("Registering netdevice failed");
    brcmf_free_net_device(ifp->ndev);
    goto fail;
  }

  err = brcmf_set_ap_macaddr(ifp, mac_addr);
  if (err != ZX_OK) {
    BRCMF_ERR("unable to set mac address of ap if");
    goto fail;
  }

  if (dev_out) {
    *dev_out = &ifp->vif->wdev;
  }
  return ZX_OK;

fail:
  brcmf_free_vif(vif);
  if (dev_out) {
    *dev_out = nullptr;
  }
  return err;
}

static bool brcmf_is_apmode(struct brcmf_cfg80211_vif* vif) {
  return vif->wdev.iftype == fuchsia_wlan_common::WlanMacRole::kAp;
}

static bool brcmf_is_existing_macaddr(brcmf_pub* drvr, const uint8_t mac_addr[ETH_ALEN],
                                      bool is_ap) {
  if (is_ap) {
    for (const auto& iface : drvr->iflist) {
      if (iface != nullptr && !memcmp(iface->mac_addr, mac_addr, ETH_ALEN)) {
        return true;
      }
    }
  } else {
    for (const auto& iface : drvr->iflist) {
      if (iface != nullptr &&
          iface->vif->wdev.iftype != fuchsia_wlan_common::WlanMacRole::kClient &&
          !memcmp(iface->mac_addr, mac_addr, ETH_ALEN)) {
        return true;
      }
    }
  }
  return false;
}

zx_status_t brcmf_cfg80211_add_iface(brcmf_pub* drvr, const char* name, struct vif_params* params,
                                     fuchsia_wlan_common::WlanMacRole role,
                                     std::optional<MacAddr> mac_addr, zx::channel mlme_channel,
                                     struct wireless_dev** wdev_out) {
  zx_status_t err;
  net_device* ndev = nullptr;
  wireless_dev* wdev;
  int32_t bsscfgidx;

  BRCMF_DBG(TRACE, "enter: %s type %d", name, fidl::ToUnderlying(role));

  if (wdev_out == nullptr) {
    BRCMF_ERR("cannot write wdev to nullptr");
    return ZX_ERR_INVALID_ARGS;
  }

  err = brcmf_vif_add_validate(drvr->config, role);
  if (err != ZX_OK) {
    BRCMF_ERR("iface validation failed: err=%d", err);
    return err;
  }

  struct brcmf_if* ifp;
  const char* iface_role_name;

  switch (role) {
    case fuchsia_wlan_common::WlanMacRole::kAp:
      iface_role_name = "ap";

      if (mac_addr && brcmf_is_existing_macaddr(drvr, mac_addr->byte, true)) {
        return ZX_ERR_ALREADY_EXISTS;
      }

      err = brcmf_ap_add_vif(drvr->config, name, mac_addr, &wdev);
      if (err != ZX_OK) {
        BRCMF_ERR("add iface %s type %d failed: err=%d", name, (uint32_t)role, err);
        return err;
      }

      brcmf_cfg80211_update_proto_addr_mode(wdev);
      ndev = wdev->netdev;
      wdev->iftype = role;
      if (ndev) {
        ndev->sme_channel = std::move(mlme_channel);
      } else {
        BRCMF_ERR("no ndev found to bind mlme_channel");
        return ZX_ERR_BAD_STATE;
      }

      break;
    case fuchsia_wlan_common::WlanMacRole::kClient: {
      iface_role_name = "client";

      if (mac_addr && brcmf_is_existing_macaddr(drvr, mac_addr->byte, false)) {
        return ZX_ERR_ALREADY_EXISTS;
      }
      bsscfgidx = brcmf_get_prealloced_bsscfgidx(drvr);
      if (bsscfgidx < 0) {
        return ZX_ERR_NO_MEMORY;
      }

      ndev = drvr->iflist[bsscfgidx]->ndev;
      if (strncmp(ndev->name, name, sizeof(ndev->name))) {
        BRCMF_INFO("Reusing netdev:%s for new client iface, but changing its name to netdev:%s.",
                   ndev->name, name);
        brcmf_write_net_device_name(ndev, name);
      }
      ifp = brcmf_get_ifp(drvr, 0);

      if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
        // Since a single IF is shared when operating with manufacturing FW, change
        // IF type.
        err = brcmf_cfg80211_change_iface_mac_role(drvr->config, ifp->ndev,
                                                   fuchsia_wlan_common::WlanMacRole::kClient);
        if (err != ZX_OK) {
          BRCMF_ERR("failed to change iface mac role to CLIENT: %s", zx_status_get_string(err));
          return err;
        }
      }
      wdev = &drvr->iflist[bsscfgidx]->vif->wdev;
      wdev->iftype = role;
      if (ndev) {
        ndev->sme_channel = std::move(mlme_channel);
      } else {
        BRCMF_ERR("no ndev found to bind mlme_channel");
        return ZX_ERR_BAD_STATE;
      }
      ndev->needs_free_net_device = false;

      // Use input mac_addr if it's provided. Otherwise, fallback to the bootloader
      // MAC address. Note that this fallback MAC address is intended for client ifaces only.
      MacAddr client_mac_addr;
      if (mac_addr) {
        client_mac_addr = *mac_addr;
      } else {
        err = brcmf_bus_get_bootloader_macaddr(drvr->bus_if, client_mac_addr.byte);
        if (err != ZX_OK || client_mac_addr.IsZero() || client_mac_addr.IsBcast()) {
          if (err != ZX_OK) {
            BRCMF_WARN("Failed to get mac address from bootloader: %s", zx_status_get_string(err));
          } else {
            BRCMF_WARN("Failed to get valid mac address from bootloader.");
#if !defined(NDEBUG)
            BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC, FMT_MAC_ARGS(client_mac_addr.byte));
#endif /* !defined(NDEBUG) */
          }
          err = brcmf_gen_random_mac_addr(client_mac_addr.byte);
          if (err != ZX_OK) {
            BRCMF_ERR("Failed to generate random MAC address.");
            return err;
          }
          BRCMF_INFO("Falling back to random mac address");
#if !defined(NDEBUG)
          BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC, FMT_MAC_ARGS(client_mac_addr.byte));
#endif /* !defined(NDEBUG) */
        } else {
          BRCMF_IFDBG(WLANIF, ndev, "Retrieved bootloader wifi MAC addresss");
#if !defined(NDEBUG)
          BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC, FMT_MAC_ARGS(client_mac_addr.byte));
#endif /* !defined(NDEBUG) */
        }
      }

      err = brcmf_set_iface_macaddr(ndev, client_mac_addr);
      if (err != ZX_OK) {
        BRCMF_ERR("Failed to set MAC address for client iface netdev:%s", ndev->name);
#if !defined(NDEBUG)
        BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC, FMT_MAC_ARGS(client_mac_addr.byte));
#endif /* !defined(NDEBUG) */
        return err;
      }

      break;
    }
    default:
      return ZX_ERR_INVALID_ARGS;
  }
  *wdev_out = wdev;
  return ZX_OK;
}

static void brcmf_scan_config_mpc(struct brcmf_if* ifp, int mpc) {
  if (brcmf_feat_is_quirk_enabled(ifp, BRCMF_FEAT_QUIRK_NEED_MPC)) {
    brcmf_enable_mpc(ifp, mpc);
  }
}

// This function set "mpc" to the requested value only if SoftAP
// has not been started. Else it sets "mpc" to 0.
void brcmf_enable_mpc(struct brcmf_if* ifp, int mpc) {
  BRCMF_DBG(TRACE, "Enter");

  zx_status_t err = ZX_OK;
  bcme_status_t fw_err = BCME_OK;
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  // If AP has been started, mpc is always 0
  if (cfg->ap_started) {
    mpc = 0;
  }
  err = brcmf_fil_iovar_int_set(ifp, "mpc", mpc, &fw_err);
  if (err != ZX_OK) {
    BRCMF_INFO("fail to set mpc: %s, fw err %s", zx_status_get_string(err),
               brcmf_fil_get_errstr(fw_err));
    return;
  }
  BRCMF_DBG(TRACE, "Exit");
}

static void brcmf_signal_scan_end(struct net_device* ndev, uint64_t txn_id,
                                  fuchsia_wlan_fullmac_wire::WlanScanResult scan_result_code) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped-- skipping signal scan end callback ");
    return;
  }
  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena in WlanFullmacIfc::OnScanEnd() status=%s",
              arena.status_string());
    return;
  }
  auto scan_end_builder =
      fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcOnScanEndRequest::Builder(*arena);
  scan_end_builder.txn_id(txn_id);
  scan_end_builder.code(scan_result_code);
  BRCMF_DBG(SCAN, "Signaling on_scan_end with txn_id %" PRIu64 " and code %u", txn_id,
            static_cast<uint8_t>(scan_result_code));
  BRCMF_IFDBG(
      WLANIF, ndev,
      "Sending scan end event to SME. txn_id: %" PRIu64
      ", result: %s"
      ", number of results: %" PRIu32 "",
      txn_id,
      scan_result_code == fuchsia_wlan_fullmac_wire::WlanScanResult::kSuccess ? "success"
      : scan_result_code == fuchsia_wlan_fullmac_wire::WlanScanResult::kNotSupported
          ? "not supported"
      : scan_result_code == fuchsia_wlan_fullmac_wire::WlanScanResult::kInvalidArgs ? "invalid args"
      : scan_result_code == fuchsia_wlan_fullmac_wire::WlanScanResult::kInternalError
          ? "internal error"
      : scan_result_code == fuchsia_wlan_fullmac_wire::WlanScanResult::kShouldWait ? "should wait"
      : scan_result_code == fuchsia_wlan_fullmac_wire::WlanScanResult::kCanceledByDriverOrFirmware
          ? "canceled by driver or firmware"
          : "unknown",
      ndev->scan_num_results);
  auto result = ndev->if_proto.buffer(*arena)->OnScanEnd(scan_end_builder.Build());
  if (!result.ok()) {
    BRCMF_ERR("Failed to indicate scan end result.status: %s, txn_id=%zu", result.status_string(),
              txn_id);
    return;
  }
}

static zx_status_t brcmf_abort_escan(struct brcmf_if* ifp) {
  /* Do a scan abort to stop the driver's scan engine */
  BRCMF_DBG(SCAN, "ABORT scan in firmware");
  struct brcmf_scan_params_le params_le = {};
  fill_with_broadcast_addr(params_le.bssid);
  params_le.bss_type = DOT11_BSSTYPE_ANY;
  params_le.scan_type = 0;
  params_le.channel_num = 1;
  params_le.nprobes = 1;
  params_le.active_time = -1;
  params_le.passive_time = -1;
  params_le.home_time = -1;
  /* Scan is aborted by setting channel_list[0] to -1 */
  params_le.channel_list[0] = -1;
  /* E-Scan (or anyother type) can be aborted by SCAN */
  bcme_status_t fwerr = BCME_OK;
  zx_status_t err =
      brcmf_fil_cmd_data_set(ifp, BRCMF_C_SCAN, &params_le, sizeof(params_le), &fwerr);
  if (err != ZX_OK) {
    BRCMF_ERR("Scan abort failed: %s (fw err %s)", zx_status_get_string(err),
              brcmf_fil_get_errstr(fwerr));
  }

  return err;
}

static void brcmf_notify_escan_complete(struct brcmf_cfg80211_info* cfg, struct brcmf_if* ifp,
                                        brcmf_fweh_event_status_t status) {
  BRCMF_DBG(SCAN, "Enter");

  struct net_device* ndev = cfg_to_ndev(cfg);
  fuchsia_wlan_fullmac_wire::WlanScanResult scan_result =
      fuchsia_wlan_fullmac_wire::WlanScanResult::kSuccess;

  if (!ndev) {
    BRCMF_WARN("Device does not exist, skipping escan complete notify.");
    return;
  }

  // Canceling if it's inactive is OK. Checking if it's active just invites race conditions.
  cfg->escan_timer->Stop();
  brcmf_scan_config_mpc(ifp, 1);

  if (cfg->scan_in_progress) {
    BRCMF_IFDBG(WLANIF, ndev, "ESCAN Completed scan: %s",
                status == BRCMF_E_STATUS_SUCCESS ? "Done"
                : status == BRCMF_E_STATUS_ABORT ? "Aborted"
                                                 : "Errored");
    cfg->scan_in_progress = false;

    switch (status) {
      case BRCMF_E_STATUS_SUCCESS:
        scan_result = fuchsia_wlan_fullmac_wire::WlanScanResult::kSuccess;
        break;
      case BRCMF_E_STATUS_NEWASSOC:
        // In this case, the scan process has been interrupted by an assoc inside the firwmare.
      case BRCMF_E_STATUS_ABORT:
        BRCMF_INFO("Sending notification of aborted scan: %d", status);
        scan_result = fuchsia_wlan_fullmac_wire::WlanScanResult::kCanceledByDriverOrFirmware;
        break;
      default:
        BRCMF_WARN("Sending notification of failed scan: %d", status);
        scan_result = fuchsia_wlan_fullmac_wire::WlanScanResult::kInternalError;
    }
    brcmf_signal_scan_end(ndev, ndev->scan_txn_id, scan_result);
  }

  if (!brcmf_test_and_clear_bit(brcmf_scan_status_bit_t::BUSY, &cfg->scan_status)) {
    BRCMF_DBG(SCAN, "Scan complete, probably P2P scan");
  }
}

static zx_status_t brcmf_cfg80211_del_ap_iface(struct brcmf_cfg80211_info* cfg,
                                               struct wireless_dev* wdev) {
  struct net_device* ndev = wdev->netdev;
  struct brcmf_if* ifp = nullptr;
  zx_status_t err = ZX_OK;

  if (ndev)
    ifp = ndev_to_if(ndev);
  else {
    BRCMF_ERR("Net device is nullptr");
    return ZX_ERR_IO;
  }

  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
    // If we are operating with manufacturing FW, we just have a single IF. Pretend like it was
    // deleted.
    //
    // For standard APs, if_proto is natively cleared when ndev is deleted during teardown.
    // However, MFG APs recycle ndev instead of deleting it, so we explicitly clear if_proto here.
    {
      std::lock_guard<std::shared_mutex> guard(ndev->if_proto_lock);
      ndev->if_proto = fidl::WireSyncClient<fuchsia_wlan_fullmac::WlanFullmacImplIfc>();
    }
    return ZX_OK;
  }

  // If we are in the process of resetting, then ap interface no longer exists
  // in firmware (since fw has been reloaded). We can skip sending commands
  // related to destorying the interface.
  if (ifp->drvr->drvr_resetting.load()) {
    goto skip_fw_cmds;
  }

  brcmf_cfg80211_arm_vif_event(cfg, ifp->vif, BRCMF_E_IF_DEL);

  err = brcmf_fil_bsscfg_data_set(ifp, "interface_remove", nullptr, 0);
  if (err != ZX_OK) {
    BRCMF_ERR("interface_remove interface %d failed %d", ifp->ifidx, err);
    brcmf_cfg80211_disarm_vif_event(cfg);
    return err;
  }

  /* wait for firmware event */
  err = brcmf_cfg80211_wait_vif_event(cfg, ZX_MSEC(BRCMF_VIF_EVENT_TIMEOUT_MSEC));
  if (err != ZX_OK) {
    BRCMF_ERR("BRCMF_VIF_EVENT timeout occurred");
    brcmf_cfg80211_disarm_vif_event(cfg);
    return ZX_ERR_IO;
  }
  brcmf_cfg80211_disarm_vif_event(cfg);

skip_fw_cmds:
  brcmf_remove_interface(ifp, true);
  return err;
}

static zx_status_t brcmf_dev_escan_set_randmac(struct brcmf_if* ifp) {
  struct brcmf_pno_macaddr_le pfn_mac = {};
  zx_status_t err = ZX_OK;
  bcme_status_t fw_err = BCME_OK;

  pfn_mac.version = BRCMF_PFN_MACADDR_CFG_VER;
  pfn_mac.flags = BRCMF_PFN_USE_FULL_MACADDR;

  err = brcmf_gen_random_mac_addr(pfn_mac.mac);
  if (err != ZX_OK) {
    return err;
  }

  err = brcmf_fil_iovar_data_set(ifp, "pfn_macaddr", &pfn_mac, sizeof(pfn_mac), &fw_err);
  if (err)
    BRCMF_ERR("set escan randmac failed, err=%d, fw_err=%d", err, fw_err);

  return err;
}

static zx_status_t brcmf_escan_prep(
    struct brcmf_cfg80211_info* cfg, struct brcmf_scan_params_le* params_le,
    const fuchsia_wlan_fullmac_wire::WlanFullmacImplStartScanRequest* request) {
  uint32_t n_ssids = 0;
  uint32_t n_channels = 0;
  int32_t offset = 0;
  if (!(request->has_scan_type() && request->has_channels())) {
    BRCMF_ERR("Missing required field, scan_type: %d, channels: %d", request->has_scan_type(),
              request->has_channels());
    return ZX_ERR_INVALID_ARGS;
  }

  fill_with_broadcast_addr(params_le->bssid);

  /* Wildcard SSID serves as a fallback value if ssids_list is empty. Otherwise,
     this field is ignored. */
  fill_with_wildcard_ssid(&params_le->ssid_le);

  // Do not filter scan results based on BSS type.
  params_le->bss_type = DOT11_BSSTYPE_ANY;

  if (request->scan_type() == fuchsia_wlan_fullmac_wire::WlanScanType::kActive) {
    params_le->scan_type = BRCMF_SCANTYPE_ACTIVE;
    params_le->active_time = request->min_channel_time();
    params_le->nprobes = BRCMF_ACTIVE_SCAN_NUM_PROBES;
    params_le->passive_time = -1;
  } else {
    params_le->scan_type = BRCMF_SCANTYPE_PASSIVE;
    params_le->passive_time = request->min_channel_time();
    params_le->active_time = -1;
  }
  params_le->channel_num = 0;
  params_le->home_time = -1;

  /* Copy channel array if applicable */
  n_channels = request->channels().size();
  BRCMF_DBG(SCAN, "### List of channelspecs to scan ### %u", n_channels);
  if (n_channels == 0) {
    BRCMF_ERR("Scan request contains empty channel list.");
    return ZX_ERR_INVALID_ARGS;
  } else {
    uint32_t valid_channels = 0;
    for (uint32_t i = 0; i < n_channels; i++) {
      const auto& channel = request->channels().data()[i];
      // Cbw20 is deliberately hard-coded for scanning purposes.
      const auto chanspec =
          channel_to_chanspec(&cfg->d11inf, channel.number, channel.band,
                              fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20);
      if (chanspec.is_error()) {
        BRCMF_ERR("Failed to get chanspec for channel %u: %s", channel.number,
                  chanspec.status_string());
        continue;
      }
      BRCMF_DBG(SCAN, "Chan : %d, Channel spec: %x", channel.number, chanspec.value());
      params_le->channel_list[valid_channels++] = chanspec.value();
    }
    if (valid_channels == 0) {
      BRCMF_ERR("No valid channels to scan after conversion.");
      return ZX_ERR_INVALID_ARGS;
    }
    n_channels = valid_channels;
  }

  /* Add number of channels to channel_num */
  params_le->channel_num = n_channels & BRCMF_SCAN_PARAMS_COUNT_MASK;

  if (!request->has_ssids()) {
    BRCMF_DBG(SCAN, "No ssids field in the request.");
  } else {
    /* Set SSID fields as applicable */
    n_ssids = request->ssids().size();
    BRCMF_DBG(SCAN, "### List of SSIDs to scan ### %d", n_ssids);

    /* Copy ssids_list if non-empty */
    if (n_ssids > 0) {
      offset = offsetof(struct brcmf_scan_params_le, channel_list) + n_channels * sizeof(uint16_t);
      offset = roundup(offset, sizeof(uint32_t));
      struct brcmf_ssid_le* ssid_le =
          reinterpret_cast<struct brcmf_ssid_le*>(reinterpret_cast<char*>(params_le) + offset);
      for (uint32_t i = 0; i < n_ssids; i++, ssid_le++) {
        if (request->ssids().data()[i].size() > fuchsia_wlan_ieee80211::kMaxSsidByteLen) {
          BRCMF_ERR("SSID in scan request SSID list too long(no longer than %hhu bytes)",
                    fuchsia_wlan_ieee80211::kMaxSsidByteLen);
          return ZX_ERR_INVALID_ARGS;
        }
        ssid_le->SSID_len = request->ssids().data()[i].size();
        memcpy(&ssid_le->SSID, request->ssids().data()[i].data(),
               request->ssids().data()[i].size());
        if (ssid_le->SSID_len == 0) {
          BRCMF_DBG(SCAN, "%d: Broadcast scan", i);
        } else {
          BRCMF_DBG(SCAN, "%d: Targeted scan", i);
#if !defined(NDEBUG)
          BRCMF_DBG(SCAN, "  ssid:" FMT_SSID, FMT_SSID_BYTES(ssid_le->SSID, ssid_le->SSID_len));
#endif /* !defined(NDEBUG) */
        }
      }
    }
  }

  /* Add number of SSIDs to channel_num. See comment at channel_num field declaration. */
  params_le->channel_num |= n_ssids << BRCMF_SCAN_PARAMS_NSSID_SHIFT;

  return ZX_OK;
}

// Calculate the amount of memory needed to hold the escan parameters for a firmware request
static size_t brcmf_escan_params_size(size_t num_channels, size_t num_ssids) {
  size_t size = BRCMF_SCAN_PARAMS_FIXED_SIZE;

  // escan params headers
  size += offsetof(struct brcmf_escan_params_le, params_le);

  // Channel specs
  size += sizeof(uint32_t) * ((num_channels + 1) / 2);

  // SSIDs
  size += sizeof(struct brcmf_ssid_le) * num_ssids;

  return size;
}

static inline uint16_t brcmf_next_sync_id(struct brcmf_cfg80211_info* cfg) {
  return cfg->next_sync_id++;
}

static zx_status_t brcmf_run_escan(
    struct brcmf_cfg80211_info* cfg, struct brcmf_if* ifp,
    const fuchsia_wlan_fullmac_wire::WlanFullmacImplStartScanRequest* request,
    uint16_t* sync_id_out) {
  // Check required fields.
  if (request == nullptr || !(request->has_channels() && request->has_min_channel_time() &&
                              request->has_max_channel_time())) {
    BRCMF_ERR(
        "Missing required field, request: %d, channels: %d, min_channel_time: %d, max_channel_time: %d",
        request ? 1 : 0, request ? request->has_channels() : 0,
        request ? request->has_min_channel_time() : 0,
        request ? request->has_max_channel_time() : 0);
    return ZX_ERR_INVALID_ARGS;
  }

  // Validate dwell times
  if (request->min_channel_time() == 0 ||
      request->max_channel_time() < request->min_channel_time()) {
    BRCMF_ERR("Invalid dwell times in escan request min: %u max: %u", request->min_channel_time(),
              request->max_channel_time());
    return ZX_ERR_INVALID_ARGS;
  }

  size_t ssids_count = request->has_ssids() ? request->ssids().size() : 0;
  // Calculate space needed for parameters
  size_t params_size = brcmf_escan_params_size(request->channels().size(), ssids_count);

  // Validate command size
  size_t total_cmd_size = params_size + sizeof("escan");
  if (total_cmd_size > BCDC_TX_IOCTL_MAX_MSG_SIZE) {
    BRCMF_ERR("Escan params size (%zu) exceeds command max capacity (%lu)", total_cmd_size,
              BCDC_TX_IOCTL_MAX_MSG_SIZE);
    return ZX_ERR_INVALID_ARGS;
  }

  zx_status_t err = ZX_OK;
  bcme_status_t fw_err = BCME_OK;

  BRCMF_DBG(SCAN, "E-SCAN START");

  struct brcmf_escan_params_le* params = static_cast<decltype(params)>(calloc(1, params_size));
  if (!params) {
    err = ZX_ERR_NO_MEMORY;
    goto exit;
  }
  err = brcmf_escan_prep(cfg, &params->params_le, request);
  if (err != ZX_OK) {
    BRCMF_ERR("escan preparation failed");
    goto exit;
  }
  params->version = BRCMF_ESCAN_REQ_VERSION;
  params->action = WL_ESCAN_ACTION_START;
  params->sync_id = brcmf_next_sync_id(cfg);

  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_SCAN_RANDOM_MAC) &&
      (params->params_le.scan_type == BRCMF_SCANTYPE_ACTIVE) &&
      !brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state)) {
    if ((err = brcmf_dev_escan_set_randmac(ifp)) != ZX_OK) {
      BRCMF_ERR("Failed to set random mac for active scan (%s), using interface mac",
                zx_status_get_string(err));
    }
  }

  err = brcmf_fil_iovar_data_set(ifp, "escan", params, params_size, &fw_err);
  if (err == ZX_OK) {
    *sync_id_out = params->sync_id;
  } else {
    if (err == ZX_ERR_UNAVAILABLE) {
      BRCMF_ERR("system busy : escan canceled sme state: 0x%lx", atomic_load(&ifp->vif->sme_state));
    } else if (err == ZX_ERR_SHOULD_WAIT) {
      BRCMF_INFO("firmware is busy, failing the scan, please retry later. %s, fw err %s",
                 zx_status_get_string(err), brcmf_fil_get_errstr(fw_err));
    } else {
      BRCMF_ERR("escan failed: %s, fw err %s", zx_status_get_string(err),
                brcmf_fil_get_errstr(fw_err));
    }
  }

exit:
  free(params);
  return err;
}

static zx_status_t brcmf_do_escan(
    struct brcmf_if* ifp, const fuchsia_wlan_fullmac_wire::WlanFullmacImplStartScanRequest* req,
    uint16_t* sync_id_out) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  zx_status_t err;
  struct escan_info* escan = &cfg->escan_info;

  BRCMF_DBG(SCAN, "Enter");
  escan->ifp = ifp;
  escan->escan_state = WL_ESCAN_STATE_SCANNING;

  brcmf_scan_config_mpc(ifp, 0);

  err = escan->run(cfg, ifp, req, sync_id_out);
  if (err != ZX_OK) {
    brcmf_scan_config_mpc(ifp, 1);
  }
  return err;
}

zx_status_t brcmf_check_scan_status(unsigned long scan_status,
                                    std::string* out_scan_status_report) {
  zx_status_t out_scan_status = ZX_OK;
  std::string scan_status_report;

  for (auto scan_status_bit : BRCMF_ALL_SCAN_STATUS_BITS) {
    if (brcmf_test_bit(scan_status_bit, scan_status)) {
      out_scan_status = ZX_ERR_UNAVAILABLE;
      if (out_scan_status_report == nullptr) {
        return out_scan_status;
      }
      if (out_scan_status_report != nullptr) {
        if (!scan_status_report.empty()) {
          scan_status_report += "+";
        }
        scan_status_report += brcmf_get_scan_status_bit_str(scan_status_bit);
      }
    }
  }

  if (out_scan_status_report != nullptr) {
    if (!scan_status_report.empty()) {
      scan_status_report += " ";
    }
    const char fmt[] = "(%#lx)";
    int sz = snprintf(nullptr, 0, fmt, scan_status);
    if (sz < 0) {
      BRCMF_ERR("Failed to generate scan status report string: %d", sz);
      return out_scan_status;
    }
    char buf[sz + 1];
    snprintf(buf, sizeof buf, fmt, scan_status);
    scan_status_report += buf;

    *out_scan_status_report = scan_status_report;
  }
  return out_scan_status;
}

zx_status_t brcmf_cfg80211_scan(
    struct net_device* ndev, const fuchsia_wlan_fullmac_wire::WlanFullmacImplStartScanRequest* req,
    uint16_t* sync_id_out) {
  zx_status_t err;

  BRCMF_DBG(TRACE, "Enter");
  struct wireless_dev* wdev = ndev_to_wdev(ndev);
  struct brcmf_cfg80211_vif* vif = containerof(wdev, struct brcmf_cfg80211_vif, wdev);
  if (!check_vif_up(vif)) {
    return ZX_ERR_IO;
  }

  struct brcmf_cfg80211_info* cfg = ndev_to_if(ndev)->drvr->config;

  std::string scan_status_report;
  zx_status_t scan_status = brcmf_check_scan_status(cfg->scan_status.load(), &scan_status_report);
  if (scan_status != ZX_OK) {
    BRCMF_ERR("Scanning unavailable: scan_status %s", scan_status_report.c_str());
    return scan_status;
  }

  if (brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTING, &vif->sme_state)) {
    BRCMF_INFO("Scan request suppressed: connect in progress (status: %lu)", vif->sme_state.load());
    return ZX_ERR_SHOULD_WAIT;
  }
  if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &vif->sme_state)) {
    BRCMF_INFO("Scan request suppressed: roam in progress (status: %lu)", vif->sme_state.load());
    return ZX_ERR_SHOULD_WAIT;
  }
  if (brcmf_is_ap_start_pending(cfg)) {
    BRCMF_INFO("AP start request in progress, rejecting scan request, a retry is expected.");
    return ZX_ERR_SHOULD_WAIT;
  }

  BRCMF_DBG(SCAN, "START ESCAN\n");

  cfg->scan_in_progress = true;
  brcmf_set_bit(brcmf_scan_status_bit_t::BUSY, &cfg->scan_status);

  cfg->escan_info.run = brcmf_run_escan;

  err = brcmf_do_escan(vif->ifp, req, sync_id_out);
  if (err != ZX_OK) {
    goto scan_out;
  }

  /* Arm scan timeout timer */
  cfg->escan_timer->Start(ZX_MSEC(BRCMF_ESCAN_TIMER_INTERVAL_MS));
  return ZX_OK;

scan_out:
  if (err != ZX_ERR_SHOULD_WAIT) {
    BRCMF_ERR("scan error (%d)", err);
  }
  brcmf_clear_bit(brcmf_scan_status_bit_t::BUSY, &cfg->scan_status);
  cfg->scan_in_progress = false;
  return err;
}

static void brcmf_init_prof(struct brcmf_cfg80211_profile* prof) { memset(prof, 0, sizeof(*prof)); }

static void brcmf_clear_profile_on_client_disconnect(struct brcmf_cfg80211_profile* prof) {
  // Bssid needs to be preserved for disconnects due to disassoc ind. SME will
  // skip the join and auth steps, and so this will not get repopulated.
  uint8_t bssid[ETH_ALEN];

  memcpy(bssid, prof->bssid, ETH_ALEN);
  brcmf_init_prof(prof);
  memcpy(prof->bssid, bssid, ETH_ALEN);
}

static zx_status_t brcmf_set_pmk(struct brcmf_if* ifp, const uint8_t* pmk_data, uint16_t pmk_len) {
  struct brcmf_wsec_pmk_le pmk;
  int i;
  zx_status_t err;

  /* convert to firmware key format */
  pmk.key_len = pmk_len << 1;
  pmk.flags = BRCMF_WSEC_PASSPHRASE;
  for (i = 0; i < pmk_len; i++) {
    // TODO(cphoenix): Make sure handling of pmk keys is consistent with their being
    // binary values, not ASCII chars.
    snprintf((char*)&pmk.key[2 * i], 3, "%02x", pmk_data[i]);
  }

  /* store psk in firmware */
  err = brcmf_fil_cmd_data_set(ifp, BRCMF_C_SET_WSEC_PMK, &pmk, sizeof(pmk), nullptr);
  if (err != ZX_OK) {
    BRCMF_ERR("failed to change PSK in firmware (len=%u)", pmk_len);
  }

  return err;
}

static void brcmf_notify_deauth(struct net_device* ndev, const uint8_t peer_sta_address[ETH_ALEN]) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping deauth confirm callback");
    return;
  }

  BRCMF_IFDBG(WLANIF, ndev, "Sending deauth confirm to SME.");
#if !defined(NDEBUG)
  BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC "", FMT_MAC_ARGS(peer_sta_address));
#endif /* !defined(NDEBUG) */

  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }

  fidl::Array<uint8_t, ETH_ALEN> address;
  memcpy(address.data(), peer_sta_address, ETH_ALEN);

  auto resp = fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcDeauthConfRequest::Builder(*arena)
                  .peer_sta_address(address)
                  .Build();

  auto result = ndev->if_proto.buffer(*arena)->DeauthConf(resp);
  if (!result.ok()) {
    BRCMF_ERR("Failed to send deauth conf result.status: %s", result.status_string());
  }
}

static void brcmf_notify_disassoc(struct net_device* ndev, zx_status_t status) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping disassoc confirm callback");
    return;
  }
  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }

  auto resp = fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcDisassocConfRequest::Builder(*arena)
                  .status(status)
                  .Build();

  BRCMF_IFDBG(WLANIF, ndev, "Sending disassoc confirm to SME. status: %" PRIu32 "", status);
  auto result = ndev->if_proto.buffer(*arena)->DisassocConf(resp);
  if (!result.ok()) {
    BRCMF_ERR("Failed to send disassoc conf result.status: %s", result.status_string());
  }
}

// Send deauth_ind to SME (can be from client or softap)
static void brcmf_notify_deauth_ind(net_device* ndev, const uint8_t mac_addr[ETH_ALEN],
                                    fuchsia_wlan_ieee80211::ReasonCode reason_code,
                                    bool locally_initiated) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping deauth ind callback");
    return;
  }

  BRCMF_IFDBG(WLANIF, ndev, "Link Down: Sending deauth ind to SME. reason: %d",
              fidl::ToUnderlying(reason_code));
  fidl::Array<uint8_t, ETH_ALEN> peer_sta_address;
  memcpy(peer_sta_address.data(), mac_addr, ETH_ALEN);
#if !defined(NDEBUG)
  BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC "", FMT_MAC_ARGS(mac_addr));
#endif /* !defined(NDEBUG) */
  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }
  auto deauth_ind_builder =
      fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcDeauthIndRequest::Builder(*arena)
          .peer_sta_address(peer_sta_address)
          .reason_code(static_cast<fuchsia_wlan_ieee80211_wire::ReasonCode>(reason_code))
          .locally_initiated(locally_initiated)
          .Build();

  auto result = ndev->if_proto.buffer(*arena)->DeauthInd(deauth_ind_builder);
  if (!result.ok()) {
    BRCMF_ERR("Failed to send deauth ind msg status: %s", result.status_string());
    return;
  }
}

// Send disassoc_ind to SME (can be from client or softap)
static void brcmf_notify_disassoc_ind(net_device* ndev, const uint8_t mac_addr[ETH_ALEN],
                                      fuchsia_wlan_ieee80211::ReasonCode reason_code,
                                      bool locally_initiated) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping disassoc ind callback");
    return;
  }

  BRCMF_IFDBG(WLANIF, ndev, "Link Down: Sending disassoc ind to SME. reason: %d",
              fidl::ToUnderlying(reason_code));
#if !defined(NDEBUG)
  BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC ", ", FMT_MAC_ARGS(mac_addr));
#endif /* !defined(NDEBUG) */

  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }
  auto builder = fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcDisassocIndRequest::Builder(*arena);

  fidl::Array<uint8_t, ETH_ALEN> peer_sta_address;
  memcpy(peer_sta_address.data(), mac_addr, ETH_ALEN);
  auto disassoc_ind = builder.peer_sta_address(peer_sta_address)
                          .reason_code(reason_code)
                          .locally_initiated(locally_initiated)
                          .Build();
  auto result = ndev->if_proto.buffer(*arena)->DisassocInd(disassoc_ind);
  if (!result.ok()) {
    BRCMF_ERR("Failed to send disassoc ind result.status: %s", result.status_string());
    return;
  }
}

// Does the given address match the current BSSID?
static bool is_current_bss(brcmf_cfg80211_info* cfg, const uint8_t addr[ETH_ALEN]) {
  struct net_device* ndev = cfg_to_ndev(cfg);
  struct brcmf_cfg80211_profile* prof = ndev_to_prof(ndev);
  return memcmp(prof->bssid, addr, ETH_ALEN) == 0;
}

// Does the given address match the target BSSID?
// Always returns false if not currently roaming.
static bool is_target_bss(brcmf_cfg80211_info* cfg, const uint8_t addr[ETH_ALEN]) {
  struct brcmf_if* ifp = cfg_to_if(cfg);

  if (!(cfg->target_bssid.has_value() &&
        brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state))) {
    return false;
  }
  return memcmp(cfg->target_bssid->data(), addr, ETH_ALEN) == 0;
}

// Send SME notification(s) after a disconnect event was received from firmware.
static void cfg80211_disconnected(struct brcmf_cfg80211_vif* vif,
                                  fuchsia_wlan_ieee80211::ReasonCode reason_code,
                                  uint16_t event_code, const uint8_t event_addr[ETH_ALEN]) {
  struct net_device* ndev = vif->wdev.netdev;
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping link down callback");
    return;
  }

  struct brcmf_cfg80211_info* cfg = vif->ifp->drvr->config;
  BRCMF_DBG(CONN, "Link Down: address: " FMT_MAC ", SME reason: %d",
            FMT_MAC_ARGS(vif->profile.bssid), fidl::ToUnderlying(reason_code));

  if (event_code == BRCMF_E_DEAUTH || event_code == BRCMF_E_DISASSOC) {
    if (is_current_bss(cfg, event_addr) &&
        brcmf_test_and_clear_bit(brcmf_disconnect_request_bit_t::DEAUTH_CURRENT_BSS,
                                 &cfg->disconnect_request_state)) {
      brcmf_notify_deauth(ndev, vif->profile.bssid);
      return;
    }
    if (is_target_bss(cfg, event_addr) &&
        brcmf_test_and_clear_bit(brcmf_disconnect_request_bit_t::DEAUTH_TARGET_BSS,
                                 &cfg->disconnect_request_state)) {
      cfg->target_bss_authenticated = false;
      brcmf_notify_deauth(ndev, cfg->target_bssid->data());
      return;
    }
    if (brcmf_test_and_clear_bit(brcmf_disconnect_request_bit_t::DISASSOC,
                                 &cfg->disconnect_request_state)) {
      brcmf_notify_disassoc(ndev, ZX_OK);
      return;
    }
  }

  // If we get this far, the disconnect is not SME-initiated. It might have come from
  // an AP, or from the driver itself. Either way, it needs an indication.
  const bool locally_initiated =
      event_code == BRCMF_E_DEAUTH || event_code == BRCMF_E_DISASSOC || event_code == BRCMF_E_LINK;
  // BRCMF_E_DEAUTH is unlikely if not SME-initiated
  if (event_code == BRCMF_E_DEAUTH || event_code == BRCMF_E_DEAUTH_IND ||
      event_code == BRCMF_E_LINK) {
    brcmf_notify_deauth_ind(ndev, vif->profile.bssid, reason_code, locally_initiated);
  } else {
    // This is a catch-all case - could be E_DISASSOC, E_DISASSOC_IND or IF delete
    brcmf_notify_disassoc_ind(ndev, vif->profile.bssid, reason_code, locally_initiated);
  }
}

// Bring the IF down. Synaptics recommends using this to reset the IF after a
// connection attempt.
static zx_status_t brcmf_bss_reset(brcmf_if* ifp) {
  bcme_status_t fw_err;

  zx_status_t status = brcmf_fil_cmd_int_set(ifp, BRCMF_C_DOWN, 1, &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("BRCMF_C_DOWN error %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
  }

  status = brcmf_fil_cmd_int_set(ifp, BRCMF_C_UP, 1, &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("BRCMF_C_UP error: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
  }
  return status;
}

// If connected, disconnect and notify; regardless, clean up after link down.
static void brcmf_link_down(struct brcmf_cfg80211_vif* vif,
                            fuchsia_wlan_ieee80211::ReasonCode reason_code, uint16_t event_code,
                            const uint8_t event_addr[ETH_ALEN]) {
  if (vif == nullptr) {
    BRCMF_ERR("vif is null, ignoring link down");
    return;
  }
  struct brcmf_cfg80211_info* cfg = vif->ifp->drvr->config;
  zx_status_t err = ZX_OK;

  BRCMF_DBG(TRACE, "Enter");

  if (brcmf_test_and_clear_bit(brcmf_vif_status_bit_t::CONNECTED, &vif->sme_state)) {
    BRCMF_INFO("Link down while connected.");
    bcme_status_t fwerr = BCME_OK;

    // Calling BRCMF_C_DISASSOC to stop excess roaming
    err = brcmf_fil_cmd_data_set(vif->ifp, BRCMF_C_DISASSOC, nullptr, 0, &fwerr);
    if (err != ZX_OK) {
      BRCMF_ERR("BRCMF_C_DISASSOC failed: %s, fw err %s", zx_status_get_string(err),
                brcmf_fil_get_errstr(fwerr));
    }
    if (vif->wdev.iftype == fuchsia_wlan_common::WlanMacRole::kClient) {
      cfg80211_disconnected(vif, reason_code, event_code, event_addr);
    }
  }
  brcmf_bss_reset(vif->ifp);

  brcmf_clear_bit(brcmf_vif_status_bit_t::CONNECTING, &vif->sme_state);
  brcmf_clear_bit(brcmf_vif_status_bit_t::CONNECTED, &vif->sme_state);
  brcmf_clear_bit(brcmf_vif_status_bit_t::ROAMING, &vif->sme_state);
  brcmf_clear_bit(brcmf_scan_status_bit_t::SUPPRESS, &cfg->scan_status);
  brcmf_btcoex_set_mode(vif, BRCMF_BTCOEX_ENABLED, 0);
  if (vif->profile.use_fwsup != BRCMF_PROFILE_FWSUP_NONE) {
    brcmf_set_pmk(vif->ifp, nullptr, 0);
    vif->profile.use_fwsup = BRCMF_PROFILE_FWSUP_NONE;
  }
  BRCMF_DBG(TRACE, "Exit");
}

static zx_status_t brcmf_set_auth_type(struct net_device* ndev,
                                       fuchsia_wlan_fullmac_wire::WlanAuthType auth_type) {
  brcmf_if* ifp = ndev_to_if(ndev);
  int32_t val = 0;
  zx_status_t status = ZX_OK;

  switch (auth_type) {
    case fuchsia_wlan_fullmac_wire::WlanAuthType::kOpenSystem:
      val = BRCMF_AUTH_MODE_OPEN;
      break;
    case fuchsia_wlan_fullmac_wire::WlanAuthType::kSharedKey:
      // When asked to use a shared key (which should only happen for WEP), we will direct the
      // firmware to use auto-detect, which will fall back on open WEP if shared WEP fails to
      // succeed. This was chosen to allow us to avoid implementing WEP auto-detection at higher
      // levels of the wlan stack.
      val = BRCMF_AUTH_MODE_AUTO;
      break;
    case fuchsia_wlan_fullmac_wire::WlanAuthType::kSae:
      val = BRCMF_AUTH_MODE_SAE;
      break;
    default:
      return ZX_ERR_NOT_SUPPORTED;
  }

  BRCMF_DBG(CONN, "setting auth to %d", val);
  status = brcmf_fil_bsscfg_int_set(ifp, "auth", val);
  if (status != ZX_OK) {
    BRCMF_ERR("set auth failed (%s)", zx_status_get_string(status));
  }
  return status;
}

static bool brcmf_valid_wpa_oui(uint8_t* oui, bool is_rsn_ie) {
  if (is_rsn_ie) {
    return (memcmp(oui, RSN_OUI, TLV_OUI_LEN) == 0);
  }

  return (memcmp(oui, MSFT_OUI, TLV_OUI_LEN) == 0);
}

static zx_status_t brcmf_configure_wpaie(struct brcmf_if* ifp, const struct brcmf_vs_tlv* wpa_ie,
                                         bool is_rsn_ie, bool is_ap) {
  uint16_t count;
  zx_status_t err = ZX_OK;
  int32_t len;
  uint32_t i;
  uint32_t wsec;
  uint32_t pval = 0;
  uint32_t gval = 0;
  uint32_t wpa_auth = 0;
  uint32_t offset;
  uint8_t* data;
  uint16_t rsn_cap;
  uint32_t wme_bss_disable;
  uint32_t mfp;

  BRCMF_DBG(TRACE, "Enter");
  if (wpa_ie == nullptr) {
    goto exit;
  }
  len = wpa_ie->len + TLV_HDR_LEN;
  data = (uint8_t*)wpa_ie;
  offset = TLV_HDR_LEN;
  if (!is_rsn_ie) {
    offset += VS_IE_FIXED_HDR_LEN;
  } else {
    offset += WPA_IE_VERSION_LEN;
  }

  /* check for multicast cipher suite */
  if ((int32_t)offset + WPA_IE_MIN_OUI_LEN > len) {
    err = ZX_ERR_INVALID_ARGS;
    BRCMF_ERR("no multicast cipher suite");
    goto exit;
  }

  if (!brcmf_valid_wpa_oui(&data[offset], is_rsn_ie)) {
    err = ZX_ERR_INVALID_ARGS;
    BRCMF_ERR("invalid OUI");
    goto exit;
  }
  offset += TLV_OUI_LEN;

  /* pick up multicast cipher */
  switch (data[offset]) {
    case WPA_CIPHER_NONE:
      BRCMF_DBG(CONN, "MCAST WPA CIPHER NONE");
      gval = WSEC_NONE;
      break;
    case WPA_CIPHER_WEP_40:
    case WPA_CIPHER_WEP_104:
      BRCMF_DBG(CONN, "MCAST WPA CIPHER WEP40/104");
      gval = WEP_ENABLED;
      break;
    case WPA_CIPHER_TKIP:
      BRCMF_DBG(CONN, "MCAST WPA CIPHER TKIP");
      gval = TKIP_ENABLED;
      break;
    case WPA_CIPHER_CCMP_128:
      BRCMF_DBG(CONN, "MCAST WPA CIPHER CCMP 128");
      gval = AES_ENABLED;
      break;
    default:
      err = ZX_ERR_INVALID_ARGS;
      BRCMF_ERR("Invalid multi cast cipher info");
      goto exit;
  }

  offset++;
  /* walk thru unicast cipher list and pick up what we recognize */
  count = data[offset] + (data[offset + 1] << 8);
  offset += WPA_IE_SUITE_COUNT_LEN;
  /* Check for unicast suite(s) */
  if ((int32_t)(offset + (WPA_IE_MIN_OUI_LEN * count)) > len) {
    err = ZX_ERR_INVALID_ARGS;
    BRCMF_ERR("no unicast cipher suite");
    goto exit;
  }
  for (i = 0; i < count; i++) {
    if (!brcmf_valid_wpa_oui(&data[offset], is_rsn_ie)) {
      err = ZX_ERR_INVALID_ARGS;
      BRCMF_ERR("ivalid OUI");
      goto exit;
    }
    offset += TLV_OUI_LEN;
    switch (data[offset]) {
      case WPA_CIPHER_NONE:
        BRCMF_DBG(CONN, "UCAST WPA CIPHER NONE");
        break;
      case WPA_CIPHER_WEP_40:
      case WPA_CIPHER_WEP_104:
        BRCMF_DBG(CONN, "UCAST WPA CIPHER WEP 40/104");
        pval |= WEP_ENABLED;
        break;
      case WPA_CIPHER_TKIP:
        BRCMF_DBG(CONN, "UCAST WPA CIPHER TKIP");
        pval |= TKIP_ENABLED;
        break;
      case WPA_CIPHER_CCMP_128:
        BRCMF_DBG(CONN, "UCAST WPA CIPHER CCMP 128");
        pval |= AES_ENABLED;
        break;
      default:
        BRCMF_DBG(CONN, "Invalid unicast security info");
    }
    offset++;
  }
  /* walk thru auth management suite list and pick up what we recognize */
  count = data[offset] + (data[offset + 1] << 8);
  offset += WPA_IE_SUITE_COUNT_LEN;
  /* Check for auth key management suite(s) */
  if ((int32_t)(offset + (WPA_IE_MIN_OUI_LEN * count)) > len) {
    err = ZX_ERR_INVALID_ARGS;
    BRCMF_ERR("no auth key mgmt suite");
    goto exit;
  }
  for (i = 0; i < count; i++) {
    if (!brcmf_valid_wpa_oui(&data[offset], is_rsn_ie)) {
      err = ZX_ERR_INVALID_ARGS;
      BRCMF_ERR("ivalid OUI");
      goto exit;
    }
    offset += TLV_OUI_LEN;
    switch (data[offset]) {
      case RSN_AKM_NONE:
        BRCMF_DBG(CONN, "RSN_AKM_NONE");
        wpa_auth |= WPA_AUTH_NONE;
        break;
      case RSN_AKM_UNSPECIFIED:
        BRCMF_DBG(CONN, "RSN_AKM_UNSPECIFIED");
        is_rsn_ie ? (wpa_auth |= WPA2_AUTH_UNSPECIFIED) : (wpa_auth |= WPA_AUTH_UNSPECIFIED);
        break;
      case RSN_AKM_PSK:
        BRCMF_DBG(CONN, "RSN_AKM_PSK");
        is_rsn_ie ? (wpa_auth |= WPA2_AUTH_PSK) : (wpa_auth |= WPA_AUTH_PSK);
        break;
      case RSN_AKM_SHA256_PSK:
        BRCMF_DBG(CONN, "RSN_AKM_MFP_PSK");
        wpa_auth |= WPA2_AUTH_PSK_SHA256;
        break;
      case RSN_AKM_SHA256_1X:
        BRCMF_DBG(CONN, "RSN_AKM_MFP_1X");
        wpa_auth |= WPA2_AUTH_1X_SHA256;
        break;
      case RSN_AKM_SAE_PSK:
        BRCMF_DBG(CONN, "RSN_AKM_SAE");
        wpa_auth |= WPA3_AUTH_SAE_PSK;
        break;
      default:
        BRCMF_DBG(CONN, "Invalid key mgmt info, the auth mgmt suite is %u", data[offset]);
    }
    offset++;
  }

  /* Don't set SES_OW_ENABLED for now (since we don't support WPS yet) */
  wsec = (pval | gval);
  BRCMF_INFO("WSEC: 0x%x WPA AUTH: 0x%x", wsec, wpa_auth);

  /* set wsec */
  err = brcmf_fil_bsscfg_int_set(ifp, "wsec", wsec);
  if (err != ZX_OK) {
    BRCMF_ERR("wsec error %d", err);
    goto exit;
  }

  mfp = BRCMF_MFP_NONE;
  if (is_rsn_ie) {
    if (is_ap) {
      wme_bss_disable = 1;
      if (((int32_t)offset + RSN_CAP_LEN) <= len) {
        rsn_cap = data[offset] + (data[offset + 1] << 8);
        if (rsn_cap & RSN_CAP_PTK_REPLAY_CNTR_MASK) {
          wme_bss_disable = 0;
        }
        if (rsn_cap & RSN_CAP_MFPR_MASK) {
          BRCMF_DBG(TRACE, "MFP Required");
          mfp = BRCMF_MFP_REQUIRED;
          /* Firmware only supports mfp required in
           * combination with WPA2_AUTH_PSK_SHA256 or
           * WPA2_AUTH_1X_SHA256.
           */
          if (!(wpa_auth & (WPA2_AUTH_PSK_SHA256 | WPA2_AUTH_1X_SHA256))) {
            err = ZX_ERR_INVALID_ARGS;
            goto exit;
          }
          /* Firmware has requirement that WPA2_AUTH_PSK/
           * WPA2_AUTH_UNSPECIFIED be set, if SHA256 OUI
           * is to be included in the rsn ie.
           */
          if (wpa_auth & WPA2_AUTH_PSK_SHA256) {
            wpa_auth |= WPA2_AUTH_PSK;
          } else if (wpa_auth & WPA2_AUTH_1X_SHA256) {
            wpa_auth |= WPA2_AUTH_UNSPECIFIED;
          }
        } else if (rsn_cap & RSN_CAP_MFPC_MASK) {
          BRCMF_DBG(TRACE, "MFP Capable");
          mfp = BRCMF_MFP_CAPABLE;
        }
      }
      offset += RSN_CAP_LEN;
      /* set wme_bss_disable to sync RSN Capabilities */
      err = brcmf_fil_bsscfg_int_set(ifp, "wme_bss_disable", wme_bss_disable);
      if (err != ZX_OK) {
        BRCMF_ERR("wme_bss_disable error %d", err);
        goto exit;
      }

      /* Skip PMKID cnt as it is know to be 0 for AP. */
      offset += RSN_PMKID_COUNT_LEN;

      /* See if there is BIP wpa suite left for MFP */
      if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFP) &&
          ((int32_t)(offset + WPA_IE_MIN_OUI_LEN) <= len)) {
        err = brcmf_fil_bsscfg_data_set(ifp, "bip", &data[offset], WPA_IE_MIN_OUI_LEN);
        if (err != ZX_OK) {
          BRCMF_ERR("bip error %d", err);
          goto exit;
        }
      }
    } else {
      // Client path: honour the RSN-capabilities word SME placed in
      // security_ie (MFPR/MFPC) so the firmware "mfp" knob matches what the
      // host actually negotiated. Mirrors bcmdhd wl_set_key_mgmt().
      if ((static_cast<int32_t>(offset) + RSN_CAP_LEN) <= len) {
        rsn_cap = data[offset] + (data[offset + 1] << 8);
        if (rsn_cap & RSN_CAP_MFPR_MASK) {
          mfp = BRCMF_MFP_REQUIRED;
        } else if (rsn_cap & RSN_CAP_MFPC_MASK) {
          mfp = BRCMF_MFP_CAPABLE;
        }
      }
    }
  }

  /* Configure MFP, just a reminder, this needs to go after wsec otherwise the wsec command
   * will overwrite the values set by MFP
   */
  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFP)) {
    err = brcmf_fil_bsscfg_int_set(ifp, "mfp", mfp);
    if (err != ZX_OK) {
      BRCMF_ERR("mfp error %s", zx_status_get_string(err));
      goto exit;
    }
  }

  /* set upper-layer auth */
  err = brcmf_fil_bsscfg_int_set(ifp, "wpa_auth", wpa_auth);
  if (err != ZX_OK) {
    BRCMF_ERR("wpa_auth error %d", err);
    goto exit;
  }

exit:
  return err;
}

static zx_status_t brcmf_configure_opensecurity(struct brcmf_if* ifp) {
  zx_status_t err;
  int32_t wpa_val;

  /* set wsec */
  BRCMF_DBG(CONN, "Setting wsec to 0");
  err = brcmf_fil_bsscfg_int_set(ifp, "wsec", 0);
  if (err != ZX_OK) {
    BRCMF_ERR("wsec error %d", err);
    return err;
  }
  /* set upper-layer auth */
  wpa_val = WPA_AUTH_DISABLED;
  BRCMF_DBG(CONN, "Setting wpa_auth to %d", wpa_val);
  err = brcmf_fil_bsscfg_int_set(ifp, "wpa_auth", wpa_val);
  if (err != ZX_OK) {
    BRCMF_ERR("wpa_auth error %d", err);
    return err;
  }

  return ZX_OK;
}

// Retrieve information about the station with the specified MAC address. Note that
// association ID is only available when operating in AP mode (for our clients).
static zx_status_t brcmf_cfg80211_get_station(struct net_device* ndev, const uint8_t* mac,
                                              struct brcmf_sta_info_le* sta_info_le) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  zx_status_t err = ZX_OK;

  BRCMF_DBG(TRACE, "Enter, MAC " FMT_MAC, FMT_MAC_ARGS(mac));
  if (!check_vif_up(ifp->vif)) {
    return ZX_ERR_IO;
  }

  memset(sta_info_le, 0, sizeof(*sta_info_le));
  memcpy(sta_info_le, mac, ETH_ALEN);

  // First, see if we have a TDLS peer
  err = brcmf_fil_iovar_data_get(ifp, "tdls_sta_info", sta_info_le, sizeof(*sta_info_le), nullptr);
  if (err != ZX_OK) {
    bcme_status_t fw_err = BCME_OK;
    err = brcmf_fil_iovar_data_get(ifp, "sta_info", sta_info_le, sizeof(*sta_info_le), &fw_err);
    if (err != ZX_OK) {
      BRCMF_ERR("GET STA INFO failed: %s, fw err %s", zx_status_get_string(err),
                brcmf_fil_get_errstr(fw_err));
    }
  }
  BRCMF_DBG(TRACE, "Exit");
  return err;
}

static inline bool brcmf_tlv_ie_has_msft_type(const uint8_t* ie, uint8_t oui_type) {
  return (ie[TLV_LEN_OFF] >= TLV_OUI_LEN + TLV_OUI_TYPE_LEN &&
          !memcmp(&ie[TLV_BODY_OFF], MSFT_OUI, TLV_OUI_LEN) &&
          // The byte after OUI is OUI type
          ie[TLV_BODY_OFF + TLV_OUI_LEN] == oui_type);
}

void brcmf_return_assoc_result(struct net_device* ndev,
                               fuchsia_wlan_ieee80211_wire::StatusCode status_code) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping association callback");
    return;
  }

  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }
  auto conf = fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcConnectConfRequest::Builder(*arena);
  fidl::Array<uint8_t, ETH_ALEN> address;
  memcpy(address.data(), ifp->connect_req.selected_bss()->bssid().data(), ETH_ALEN);
  conf.peer_sta_address(address);
  conf.result_code(status_code);
  if (status_code == fuchsia_wlan_ieee80211_wire::StatusCode::kSuccess &&
      cfg->conn_info.resp_ie_len > 0) {
    BRCMF_DBG(TEMP, " * Hard-coding association_id to 42; this will likely break something!");
    uint16_t association_id = 42;  // TODO: Use brcmf_cfg80211_get_station() to get aid
    conf.association_id(association_id);
    conf.association_ies(::fidl::VectorView<uint8_t>::FromExternal(cfg->conn_info.resp_ie,
                                                                   cfg->conn_info.resp_ie_len));
  } else {
    conf.association_id(0);
  }

  BRCMF_IFDBG(WLANIF, ndev, "Sending connect result to SME. result: %u, aid: %u",
              static_cast<uint16_t>(status_code), conf.association_id());
  auto result = ndev->if_proto.buffer(*arena)->ConnectConf(conf.Build());
  if (!result.ok()) {
    BRCMF_ERR("Failed to send connect conf result.status: %s", result.status_string());
  }
}

void brcmf_return_roam_start(struct net_device* ndev) {
  const std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  struct brcmf_cfg80211_vif* vif = ndev_to_vif(ndev);
  struct brcmf_cfg80211_profile* prof = ndev_to_prof(ndev);

  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping roam start callback");
    return;
  }

  cfg->roam_start_sent = false;

  // These error conditions would probably indicate a bug in the driver.
  if (cfg->target_bss_info_buf == nullptr || !cfg->target_bssid.has_value()) {
    BRCMF_ERR(
        "Internal error: target BSSID and/or BSS description cannot be accessed; roam has failed at start time");
    brcmf_link_down(vif, fuchsia_wlan_ieee80211::ReasonCode::kUnspecifiedReason, BRCMF_E_ROAM_PREP,
                    prof->bssid);
    return;
  }

  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }

  auto roam_start_builder =
      fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcRoamStartIndRequest::Builder(*arena);
  fuchsia_wlan_ieee80211::wire::BssDescription selected_bss;

  // In the current implementation, roam attempts do not maintain association with the original BSS.
  // TODO(https://fxbug.dev/370513949): Support Fast BSS Transition.
  roam_start_builder.original_association_maintained(false);
  const auto target_bss_info = reinterpret_cast<brcmf_bss_info_le*>(cfg->target_bss_info_buf);

  selected_bss.beacon_period = target_bss_info->beacon_period;

  // TODO(https://fxbug.dev/80230): The probably shouldn't be hardcoded (here and elsewhere).
  selected_bss.bss_type = fuchsia_wlan_ieee80211::wire::BssType::kInfrastructure;

  auto selected_bssid = ::fidl::Array<uint8_t, ETH_ALEN>{};
  memcpy(selected_bssid.data(), cfg->target_bssid->data(), ETH_ALEN);
  roam_start_builder.selected_bssid(selected_bssid);

  const auto target_bss_info_bssid = cpp20::span{target_bss_info->BSSID};
  std::copy(target_bss_info_bssid.begin(), target_bss_info_bssid.end(), selected_bss.bssid.begin());

  selected_bss.capability_info = target_bss_info->capability;
  selected_bss.primary =
      chanspec_to_primary_channel_number(&cfg->d11inf, target_bss_info->chanspec);
  selected_bss.bandwidth = chanspec_to_channel_bandwidth(&cfg->d11inf, target_bss_info->chanspec);
  selected_bss.vht_secondary_80_channel =
      chanspec_to_secondary80(&cfg->d11inf, target_bss_info->chanspec);

  if (target_bss_info->ie_length > 0) {
    const auto& ie_ptr = cfg->target_bss_info_buf + target_bss_info->ie_offset;
    selected_bss.ies =
        ::fidl::VectorView<uint8_t>::FromExternal(ie_ptr, target_bss_info->ie_length);
  }

  selected_bss.rssi_dbm = std::min<int16_t>(0, std::max<int16_t>(-255, target_bss_info->RSSI));
  selected_bss.snr_db = static_cast<int8_t>(target_bss_info->SNR);

  roam_start_builder.selected_bss(selected_bss);

  BRCMF_IFDBG(WLANIF, ndev, "Sending roam start, BSSID: " FMT_MAC, FMT_MAC_ARGS(selected_bssid));

  auto result = ndev->if_proto.buffer(*arena)->RoamStartInd(roam_start_builder.Build());
  if (!result.ok()) {
    BRCMF_ERR("Failed to send roam start, result.status: %s", result.status_string());
    return;
  }
  cfg->roam_start_sent = true;
}

void brcmf_return_roam_result(struct net_device* ndev, const uint8_t* selected_bssid,
                              fuchsia_wlan_ieee80211_wire::StatusCode status_code) {
  const std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping roam result callback");
    return;
  }

  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }
  auto roam_result_builder =
      fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcRoamResultIndRequest::Builder(*arena);

  roam_result_builder.status_code(status_code);

  const auto selected_bssid_span = cpp20::span{selected_bssid, ETH_ALEN};
  fidl::Array<uint8_t, ETH_ALEN> selected_bssid_out;
  std::copy(selected_bssid_span.begin(), selected_bssid_span.end(), selected_bssid_out.begin());
  roam_result_builder.selected_bssid(selected_bssid_out);

  if (status_code == fuchsia_wlan_ieee80211_wire::StatusCode::kSuccess) {
    roam_result_builder.target_bss_authenticated(true);
  } else {
    roam_result_builder.target_bss_authenticated(cfg->target_bss_authenticated);
  }

  // In the current implementation, roam attempts do not maintain association with the original BSS.
  // This may change with Fast BSS Transition support.
  roam_result_builder.original_association_maintained(false);

  roam_result_builder.association_id(0);
  roam_result_builder.association_ies(::fidl::VectorView<uint8_t>::FromExternal(
      cfg->conn_info.resp_ie, cfg->conn_info.resp_ie_len));
  if (status_code == fuchsia_wlan_ieee80211_wire::StatusCode::kSuccess) {
    BRCMF_DBG(TEMP, " * Hard-coding association_id to 42; this will likely break something!");
    uint16_t association_id = 42;  // TODO: Use brcmf_cfg80211_get_station() to get aid
    roam_result_builder.association_id(association_id);
    if (cfg->conn_info.resp_ie_len <= 0) {
      BRCMF_WARN("Reassociation IEs were not found for roam result");
    }
  }

  BRCMF_IFDBG(WLANIF, ndev, "Sending roam result: 0x%x, BSSID: " FMT_MAC,
              static_cast<uint16_t>(status_code), FMT_MAC_ARGS(selected_bssid));
  auto result = ndev->if_proto.buffer(*arena)->RoamResultInd(roam_result_builder.Build());
  if (!result.ok()) {
    BRCMF_ERR("Failed to send roam result, result.status: %s", result.status_string());
  }
}

void brcmf_return_roam_conf(struct net_device* ndev, const uint8_t* selected_bssid,
                            fuchsia_wlan_ieee80211_wire::StatusCode status_code) {
  const std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping roam conf callback");
    return;
  }

  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }
  auto roam_conf_builder =
      fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcRoamConfRequest::Builder(*arena);

  roam_conf_builder.status_code(status_code);

  if (status_code == fuchsia_wlan_ieee80211_wire::StatusCode::kSuccess) {
    roam_conf_builder.target_bss_authenticated(true);
  } else {
    roam_conf_builder.target_bss_authenticated(cfg->target_bss_authenticated);
  }

  // In the current implementation, roam attempts do not maintain association with the original BSS.
  // This may change with Fast BSS Transition support.
  roam_conf_builder.original_association_maintained(false);

  const auto selected_bssid_span = cpp20::span{selected_bssid, ETH_ALEN};
  fidl::Array<uint8_t, ETH_ALEN> selected_bssid_out;
  std::copy(selected_bssid_span.begin(), selected_bssid_span.end(), selected_bssid_out.begin());
  roam_conf_builder.selected_bssid(selected_bssid_out);

  roam_conf_builder.association_id(0);
  if (status_code == fuchsia_wlan_ieee80211_wire::StatusCode::kSuccess) {
    BRCMF_DBG(TEMP, " * Hard-coding association_id to 42; this will likely break something!");
    uint16_t association_id = 42;  // TODO: Use brcmf_cfg80211_get_station() to get aid
    roam_conf_builder.association_id(association_id);
    if (cfg->conn_info.resp_ie_len <= 0) {
      BRCMF_WARN("Reassociation IEs were not found for roam conf");
    } else {
      roam_conf_builder.association_ies(::fidl::VectorView<uint8_t>::FromExternal(
          cfg->conn_info.resp_ie, cfg->conn_info.resp_ie_len));
    }
  }

  BRCMF_IFDBG(WLANIF, ndev, "Sending roam conf: 0x%x, BSSID: " FMT_MAC,
              static_cast<uint16_t>(status_code), FMT_MAC_ARGS(selected_bssid));
  auto result = ndev->if_proto.buffer(*arena)->RoamConf(roam_conf_builder.Build());
  if (!result.ok()) {
    BRCMF_ERR("Failed to send roam conf, status: %s", result.status_string());
  }
}

std::vector<uint8_t> brcmf_find_ssid_in_ies(const uint8_t* ie, size_t ie_len) {
  std::vector<uint8_t> ssid;
  size_t offset = 0;
  while (offset < ie_len) {
    uint8_t type = ie[offset];
    uint8_t length = ie[offset + TLV_LEN_OFF];
    if (type == WLAN_IE_TYPE_SSID) {
      size_t ssid_len = std::min<size_t>(length, ie_len - (offset + TLV_HDR_LEN));
      ssid_len = std::min<size_t>(ssid_len, fuchsia_wlan_ieee80211::kMaxSsidByteLen);
      auto start = ie + offset + TLV_HDR_LEN;
      ssid = std::vector<uint8_t>(start, start + ssid_len);
      break;
    }
    offset += length + TLV_HDR_LEN;
  }
  return ssid;
}

zx_status_t brcmf_cfg80211_connect(struct net_device* ndev,
                                   const fuchsia_wlan_fullmac::WlanFullmacImplConnectRequest* req) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  struct brcmf_join_params join_params;
  size_t join_params_size = 0;
  std::vector<uint8_t> ssid;
  zx_status_t err = ZX_OK;
  bcme_status_t fw_err = BCME_OK;
  bool is_rsn_ie = true;
  chanspec_t chspec = INVCHANSPEC;

  if (!req->selected_bss().has_value()) {
    BRCMF_ERR("Missing required field, selected_bss: %d", req->selected_bss().has_value());
    return ZX_ERR_INVALID_ARGS;
  }
  BRCMF_DBG(TRACE, "Enter");
  if (!check_vif_up(ifp->vif)) {
    return ZX_ERR_IO;
  }

  // TODO(https://fxbug.dev/42144507) - Remove this conversion, it is only for logging.
  const auto& selected_bss = ifp->connect_req.selected_bss().value();
  const auto old_chanspec = channel_to_chanspec(&cfg->d11inf, selected_bss.primary().number(),
                                                selected_bss.primary().band(),
                                                fuchsia_wlan_ieee80211::ChannelBandwidth::kCbw20);

  const auto chanspec_result =
      channel_to_chanspec(&cfg->d11inf, req->selected_bss()->primary().number(),
                          req->selected_bss()->primary().band(), req->selected_bss()->bandwidth());
  if (chanspec_result.is_ok()) {
    chspec = chanspec_result.value();
  } else {
    BRCMF_WARN("Connect chanspec conversion error: %s", chanspec_result.status_string());
    if (old_chanspec.is_error()) {
      BRCMF_ERR("Connect failed due to chanspec conversion errors");
      err = ZX_ERR_INVALID_ARGS;
      goto fail;
    }
    BRCMF_WARN("Connect using legacy chanspec 0x%x", old_chanspec.value());
    chspec = old_chanspec.value();
  }

  // Wait until disconnect completes before proceeding with the connect.
  if (sync_completion_wait(&ifp->disconnect_done, BRCMF_WAIT_FOR_DISCONNECT_MSEC) != ZX_OK) {
    BRCMF_ERR("Timed out waiting for client disconnect");
    goto fail;
  }
  // Firmware is already processing a connect request. Don't clear the CONNECTING bit because the
  // operation is still expected to complete.
  if (brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state)) {
    err = ZX_ERR_BAD_STATE;
    BRCMF_WARN("Connection not possible. Another connection attempt in progress.");
    brcmf_return_assoc_result(ndev,
                              fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified);
    goto done;
  }

  if (req->security_ie().has_value() && req->security_ie()->size() >= TLV_HDR_LEN &&
      req->security_ie()->data()[0] == WLAN_IE_TYPE_VENDOR_SPECIFIC &&
      brcmf_tlv_ie_has_msft_type(req->security_ie()->data(), WPA_OUI_TYPE)) {
    // Not RSNE but is WPA1 IE
    is_rsn_ie = false;
  }

  if (req->security_ie().has_value() && req->security_ie()->size() > 0) {
    // Set wpaie only if there's security ie
    err = brcmf_fil_iovar_data_set(ifp, "wpaie", req->security_ie()->data(),
                                   req->security_ie()->size(), &fw_err);
    if (err != ZX_OK) {
      BRCMF_ERR("wpaie failed: %s, fw err %s", zx_status_get_string(err),
                brcmf_fil_get_errstr(fw_err));
      goto fail;
    }
  }

  // TODO(https://fxbug.dev/42104096): We should be getting the IEs from SME. Passing a null entry
  // seems to work for now, presumably because the firmware uses its defaults.
  err = brcmf_vif_set_mgmt_ie(ifp->vif, BRCMF_VNDR_IE_ASSOCREQ_FLAG, nullptr, 0);
  if (err != ZX_OK) {
    BRCMF_ERR("Set Assoc REQ IE Failed");
  } else {
    BRCMF_DBG(TRACE, "Applied Vndr IEs for Assoc request");
  }

  if (req->security_ie().has_value() && req->security_ie()->size() > 0) {
    struct brcmf_vs_tlv* tmp_ie = (struct brcmf_vs_tlv*)req->security_ie()->data();
    err = brcmf_configure_wpaie(ifp, tmp_ie, is_rsn_ie, false);
    if (err != ZX_OK) {
      BRCMF_ERR("Failed to install RSNE: %s", zx_status_get_string(err));
      goto fail;
    }
  }

  brcmf_set_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state);

  cfg->channel = chspec;

  ssid = brcmf_find_ssid_in_ies(ifp->connect_req.selected_bss()->ies().data(),
                                ifp->connect_req.selected_bss()->ies().size());

  join_params_size = sizeof(join_params);
  memset(&join_params, 0, join_params_size);

  memcpy(&join_params.ssid_le.SSID, ssid.data(), ssid.size());
  join_params.ssid_le.SSID_len = ssid.size();

  memcpy(join_params.params_le.bssid, ifp->connect_req.selected_bss()->bssid().data(), ETH_ALEN);
  join_params.params_le.chanspec_num = 1;
  join_params.params_le.chanspec_list[0] = chspec;

  // Attempt to clear counters here and ignore the error. Synaptics indicates that
  // some counters might be active even when the client is not connected.
  brcmf_fil_iovar_data_get(ifp, "reset_cnts", nullptr, 0, &fw_err);
  brcmf_fil_iovar_data_set(ifp, "wme_clear_counters", nullptr, 0, &fw_err);
  BRCMF_DBG(CONN, "Sending C_SET_SSID to FW");
  err = brcmf_fil_cmd_data_set(ifp, BRCMF_C_SET_SSID, &join_params, join_params_size, &fw_err);
  if (err != ZX_OK) {
    BRCMF_ERR("join failed (%d)", err);
  } else {
    BRCMF_IFDBG(WLANIF, ndev, "Connect timer started.");
    cfg->connect_timer->Start(BRCMF_CONNECT_TIMER_DUR_MS);
  }

fail:
  if (err != ZX_OK) {
    if (chanspec_result.is_ok() && old_chanspec.is_ok() &&
        chanspec_result.value() != old_chanspec.value()) {
      BRCMF_WARN("Used new chanspec 0x%x instead of old chanspec 0x%x", chanspec_result.value(),
                 old_chanspec.value());
    }
    brcmf_clear_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state);
    BRCMF_DBG(CONN, "Failed during join: %s", zx_status_get_string(err));
    brcmf_return_assoc_result(ndev,
                              fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified);
  }

done:
  BRCMF_DBG(TRACE, "Exit");
  return err;
}

static zx_status_t brcmf_get_ctrl_channel(brcmf_if* ifp, uint16_t* chanspec_out,
                                          uint8_t* ctl_chan_out) {
  bcme_status_t fw_err;
  zx_status_t err;

  // Get chanspec of the given IF from firmware.
  err = brcmf_fil_iovar_data_get(ifp, "chanspec", chanspec_out, sizeof(uint16_t), &fw_err);
  if (err != ZX_OK) {
    BRCMF_ERR("Failed to retrieve chanspec: %s, fw err %s", zx_status_get_string(err),
              brcmf_fil_get_errstr(fw_err));
    return err;
  }

  // Get the control channel given chanspec
  err = chspec_ctlchan(*chanspec_out, ctl_chan_out);
  if (err != ZX_OK) {
    BRCMF_ERR("Failed to get control channel from chanspec: 0x%x status: %s", *chanspec_out,
              zx_status_get_string(err));
    return err;
  }
  return ZX_OK;
}

// Log driver and FW packet counters along with current channel and signal strength
static void brcmf_log_client_stats(struct brcmf_cfg80211_info* cfg) {
  struct net_device* ndev = cfg_to_ndev(cfg);
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_profile* prof = ndev_to_prof(ndev);
  bcme_status_t fw_err;
  uint32_t is_up = 0;
  float periodic_err_rate_tx = 0.0, periodic_err_rate_rx = 0.0;
  float lifetime_err_rate_tx = 0.0, lifetime_err_rate_rx = 0.0;

  // First check if the IF is up.
  zx_status_t err =
      brcmf_fil_cmd_data_get(ifp, BRCMF_C_GET_IS_IF_UP, &is_up, sizeof(is_up), &fw_err);
  if (err != ZX_OK) {
    BRCMF_INFO("Unable to get IF status: %s fw err %s", zx_status_get_string(err),
               brcmf_fil_get_errstr(fw_err));
  }
  // Get channel information from firmware.
  uint16_t chanspec;
  uint8_t ctl_chan = 0;
  err = brcmf_get_ctrl_channel(ifp, &chanspec, &ctl_chan);

  // Get the current rate
  uint32_t fw_rate = 0;
  float real_rate = 0.0;
  err = brcmf_fil_cmd_data_get(ifp, BRCMF_C_GET_RATE, &fw_rate, sizeof(fw_rate), &fw_err);
  if (err != ZX_OK) {
    BRCMF_INFO("Unable to get rate: %s fw err %s", zx_status_get_string(err),
               brcmf_fil_get_errstr(fw_err));
  } else {
    real_rate = BRCMF_CONVERT_TO_REAL_RATE(fw_rate);
  }

  // Get the current noise floor
  int32_t noise = 0;
  err = brcmf_fil_cmd_data_get(ifp, BRCMF_C_GET_PHY_NOISE, &noise, sizeof(noise), &fw_err);
  if (err != ZX_OK) {
    BRCMF_INFO("Unable to get noise: %s fw err %s", zx_status_get_string(err),
               brcmf_fil_get_errstr(fw_err));
  }
  BRCMF_INFO("Client IF up: %d channel: %d Rate: %.2f Mbps RSSI: %d dBm SNR: %d dB  noise: %d dBm",
             is_up, ctl_chan, real_rate, ndev->last_known_rssi_dbm, ndev->last_known_snr_db, noise);

  // Get the FW packet counts
  brcmf_pktcnt_le fw_pktcnt = {};
  err =
      brcmf_fil_cmd_data_get(ifp, BRCMF_C_GET_GET_PKTCNTS, &fw_pktcnt, sizeof(fw_pktcnt), &fw_err);
  if (err != ZX_OK) {
    BRCMF_INFO("Unable to get FW packet counts err: %s fw err %s", zx_status_get_string(err),
               brcmf_fil_get_errstr(fw_err));
  } else {
    int32_t total_rx_pkts = fw_pktcnt.rx_good_pkt + fw_pktcnt.rx_bad_pkt;
    int32_t total_tx_pkts = fw_pktcnt.tx_good_pkt + fw_pktcnt.tx_bad_pkt;

    lifetime_err_rate_rx = (float)(fw_pktcnt.rx_bad_pkt) / total_rx_pkts;
    lifetime_err_rate_tx = (float)(fw_pktcnt.tx_bad_pkt) / total_tx_pkts;
    if (total_rx_pkts > ndev->stats.total_rx_pkts_prev) {
      periodic_err_rate_rx = (float)(fw_pktcnt.rx_bad_pkt - ndev->stats.rx_bad_pkts_prev) /
                             (total_rx_pkts - ndev->stats.total_rx_pkts_prev);
      ndev->stats.total_rx_pkts_prev = total_rx_pkts;
      ndev->stats.rx_bad_pkts_prev = fw_pktcnt.rx_bad_pkt;
    }
    if (total_tx_pkts > ndev->stats.total_tx_pkts_prev) {
      periodic_err_rate_tx = (float)(fw_pktcnt.tx_bad_pkt - ndev->stats.tx_bad_pkts_prev) /
                             (total_tx_pkts - ndev->stats.total_tx_pkts_prev);
      ndev->stats.total_tx_pkts_prev = total_tx_pkts;
      ndev->stats.tx_bad_pkts_prev = fw_pktcnt.tx_bad_pkt;
    }

    const uint32_t period = BRCMF_CONNECT_LOG_DUR / ZX_SEC(1);
    BRCMF_INFO(
        "FW Stats: Rx - Good: %d Bad: %d Ocast: %d Err Rate %.2f%% (last %us %.2f%%); Tx - Good: %d Bad: %d Err Rate %.2f%% (last %us %.2f%%)",
        fw_pktcnt.rx_good_pkt, fw_pktcnt.rx_bad_pkt, fw_pktcnt.rx_ocast_good_pkt,
        lifetime_err_rate_rx * 100, period, periodic_err_rate_rx * 100, fw_pktcnt.tx_good_pkt,
        fw_pktcnt.tx_bad_pkt, lifetime_err_rate_tx * 100, period, periodic_err_rate_tx * 100);
  }

  if (ndev->stats.rx_packets != ndev->stats.rx_pkts_prev) {
    if (ndev->stats.rx_packets < ndev->stats.rx_pkts_prev) {
      BRCMF_INFO(
          "Current value for rx_packets is smaller than the last one, an overflow might happened.");
    }
    // Clear the freeze count once the device gets out of the bad state.
    ndev->stats.rx_freeze_count = 0;
  } else if (ndev->stats.tx_packets > ndev->stats.tx_pkts_prev) {
    // Increase the rx freeze count only when tx_packets is still increasing while rx_packets
    // is unchanged. This pattern is expected if a scan happens when the device is not connected to
    // an AP, but this function will not be called in this case, so no false positive will occur.
    ndev->stats.rx_freeze_count++;
  }

  // Update driver rx and tx count cached from last log.
  ndev->stats.rx_pkts_prev = ndev->stats.rx_packets;
  ndev->stats.tx_pkts_prev = ndev->stats.tx_packets;

  // Increase inspect counter when the rx freeze counter first reaches threshold.
  if (ndev->stats.rx_freeze_count == BRCMF_RX_FREEZE_THRESHOLD / BRCMF_CONNECT_LOG_DUR) {
    // Note the rx freeze in the inspect logs
    ifp->drvr->device->GetInspect()->LogRxFreeze();
  }

  // Setting attempt_deauth will cause the system to deauth the connection, if it is within the
  // allowed window of time.
  bool attempt_deauth = false;
  std::optional<fuchsia_wlan_ieee80211::ReasonCode> deauth_reason_code;

  // The reason for using larger or equal here is to make sure the deauth can be triggered again
  // after the limitation time passes.
  constexpr int kFreezeThreshold = BRCMF_RX_FREEZE_THRESHOLD / BRCMF_CONNECT_LOG_DUR;
  if (ndev->stats.rx_freeze_count >= kFreezeThreshold) {
    BRCMF_ERR("No rx frames received in %zu seconds, attempting deauth.",
              static_cast<size_t>(BRCMF_RX_FREEZE_THRESHOLD) / ZX_SEC(1));
    attempt_deauth = true;
    deauth_reason_code = fuchsia_wlan_ieee80211::ReasonCode::kFwRxStalled;
  }

  BRCMF_INFO("Driver Stats: Rx - Good: %d Bad: %d; Tx - Sent to FW: %d Conf: %d Drop: %d Bad: %d",
             ndev->stats.rx_packets, ndev->stats.rx_errors, ndev->stats.tx_packets,
             ndev->stats.tx_confirmed, ndev->stats.tx_dropped, ndev->stats.tx_errors);

  // Get the WME counters
  wl_wme_cnt_t wme_cnt;
  float wme_periodic_rx_err_rate = 0;
  err = brcmf_fil_iovar_data_get(ifp, "wme_counters", &wme_cnt, sizeof(wl_wme_cnt_t), &fw_err);
  if (err != ZX_OK) {
    BRCMF_INFO("Unable to get WME counters err: %s fw err %s", zx_status_get_string(err),
               brcmf_fil_get_errstr(fw_err));
  } else {
    int wme_rx_good_pkts = wme_cnt.rx[AC_VO].packets + wme_cnt.rx[AC_VI].packets +
                           wme_cnt.rx[AC_BE].packets + wme_cnt.rx[AC_BK].packets;
    int wme_rx_bad_pkts = wme_cnt.rx_failed[AC_VO].packets + wme_cnt.rx_failed[AC_VI].packets +
                          wme_cnt.rx_failed[AC_BE].packets + wme_cnt.rx_failed[AC_BK].packets;
    int wme_total_rx_pkts = wme_rx_good_pkts + wme_rx_bad_pkts;
    int wme_tx_good_pkts = wme_cnt.tx[AC_VO].packets + wme_cnt.tx[AC_VI].packets +
                           wme_cnt.tx[AC_BE].packets + wme_cnt.tx[AC_BK].packets;
    int wme_tx_bad_pkts = wme_cnt.tx_failed[AC_VO].packets + wme_cnt.tx_failed[AC_VI].packets +
                          wme_cnt.tx_failed[AC_BE].packets + wme_cnt.tx_failed[AC_BK].packets;

    if (wme_total_rx_pkts > ndev->stats.wme_total_rx_pkts_prev) {
      wme_periodic_rx_err_rate = (float)(wme_rx_bad_pkts - ndev->stats.wme_rx_bad_pkts_prev) /
                                 (wme_total_rx_pkts - ndev->stats.wme_total_rx_pkts_prev);
    }
    ndev->stats.wme_total_rx_pkts_prev = wme_total_rx_pkts;
    ndev->stats.wme_rx_bad_pkts_prev = wme_rx_bad_pkts;

    if (wme_periodic_rx_err_rate >= BRCMF_WME_BAD_PKT_THRESHOLD) {
      BRCMF_WARN("wme rx error rate %.2f%% greater than threshold of %.2f%%.",
                 wme_periodic_rx_err_rate * 100, BRCMF_WME_BAD_PKT_THRESHOLD * 100);
      ndev->stats.high_wme_rx_error_rate_count++;
    } else {
      ndev->stats.high_wme_rx_error_rate_count = 0;
    }

    if (ndev->stats.high_wme_rx_error_rate_count >=
        (BRCMF_HIGH_WME_RX_ERROR_RATE_PERIOD_THRESHOLD / BRCMF_CONNECT_LOG_DUR)) {
      // Log excessive wme rx error indicent to inspect
      ifp->drvr->device->GetInspect()->LogHighWmeRxErrorRate();
      BRCMF_ERR("wme rx error rate has been greater than %.2f%% for %ld, attempting deauth.",
                BRCMF_WME_BAD_PKT_THRESHOLD * 100,
                BRCMF_HIGH_WME_RX_ERROR_RATE_PERIOD_THRESHOLD / ZX_SEC(1));
      attempt_deauth = true;
      deauth_reason_code = fuchsia_wlan_ieee80211::ReasonCode::kFwHighWmeRxErrRate;
    }

    BRCMF_INFO("WME counters - Rx: %d; Rx Bad: %d; Tx: %d; Tx Bad: %d", wme_rx_good_pkts,
               wme_rx_bad_pkts, wme_tx_good_pkts, wme_tx_bad_pkts);

    BRCMF_INFO("VO AC - Rx: %d; Rx Bad: %d; Tx: %d; Tx Bad: %d", wme_cnt.rx[AC_VO].packets,
               wme_cnt.rx_failed[AC_VO].packets, wme_cnt.tx[AC_VO].packets,
               wme_cnt.tx_failed[AC_VO].packets);
    BRCMF_INFO("VI AC - Rx: %d; Rx Bad: %d; Tx: %d; Tx Bad: %d", wme_cnt.rx[AC_VI].packets,
               wme_cnt.rx_failed[AC_VI].packets, wme_cnt.tx[AC_VI].packets,
               wme_cnt.tx_failed[AC_VI].packets);
    BRCMF_INFO("BE AC - Rx: %d; Rx Bad: %d; Tx: %d; Tx Bad: %d", wme_cnt.rx[AC_BE].packets,
               wme_cnt.rx_failed[AC_BE].packets, wme_cnt.tx[AC_BE].packets,
               wme_cnt.tx_failed[AC_BE].packets);
    BRCMF_INFO("BK AC - Rx: %d; Rx Bad: %d; Tx: %d; Tx Bad: %d", wme_cnt.rx[AC_BK].packets,
               wme_cnt.rx_failed[AC_BK].packets, wme_cnt.tx[AC_BK].packets,
               wme_cnt.tx_failed[AC_BK].packets);
  }

  if (attempt_deauth && !deauth_reason_code.has_value()) {
    BRCMF_WARN("deauth not triggered, since reason code is not set.");
  } else if (attempt_deauth) {
    // Trigger a deauth, unless we have exceeded our maximum deauth rate
    // of BRCMF_MAX_DEAUTHS_PER_HOUR.
    bool deauth_allowed = false;
    uint32_t current_log_count = ndev->client_stats_log_count;

    std::list<uint32_t>* deauth_times = &ndev->deauth_trigger_times;

    if (deauth_times->size() < BRCMF_MAX_DEAUTHS_PER_HOUR) {
      // Our total number of deauth's is less than the per-hour limit
      deauth_allowed = true;
    } else {
      uint32_t oldest_deauth_time = deauth_times->front();
      if ((current_log_count - oldest_deauth_time) > ZX_HOUR(1) / BRCMF_CONNECT_LOG_DUR) {
        // It has been more than an hour since our oldest recorded deauth
        deauth_allowed = true;
        deauth_times->pop_front();
      }
    }

    if (deauth_allowed) {
      // Deauthenticate
      BRCMF_WARN("we are within allowed limit of %d deauths per hour, triggering deauth",
                 BRCMF_MAX_DEAUTHS_PER_HOUR);
      // Reset the rx freeze count when deauth is triggered, waiting for the next trigger.
      ndev->stats.rx_freeze_count = 0;
      brcmf_link_down(ifp->vif, deauth_reason_code.value(), BRCMF_E_DEAUTH, prof->bssid);
      deauth_times->push_back(current_log_count);
    }
  }

  brcmf_bus_log_stats(cfg->pub->bus_if);
  // If the client is connected to a 2.4 GHz channel, log some BT Coex related info
  if (ctl_chan <= CH_MAX_2G_CHANNEL) {
    brcmf_btcoex_log_active_bt_tasks(ifp);
  }

  // If the rate is 6 Mbps or less OR Rx error rate >= 15% OR Tx error rate is >= 15%
  // log some of the Tx and Rx error counts retrieved from FW.
  if ((real_rate != 0.0 && (real_rate <= BRCMF_LOW_DATA_RATE_THRESHOLD)) ||
      periodic_err_rate_rx >= BRCMF_HIGH_ERR_RATE_THRESHOLD ||
      periodic_err_rate_tx >= BRCMF_HIGH_ERR_RATE_THRESHOLD ||
      lifetime_err_rate_rx >= BRCMF_HIGH_ERR_RATE_THRESHOLD ||
      lifetime_err_rate_tx >= BRCMF_HIGH_ERR_RATE_THRESHOLD ||
      wme_periodic_rx_err_rate >= BRCMF_WME_BAD_PKT_LOG_THRESHOLD) {
    uint8_t cnt_buf[BRCMF_DCMD_MAXLEN] = {0};
    // If data rate is at or below threshold, increment the counter.
    if (real_rate != 0.0 && (real_rate <= BRCMF_LOW_DATA_RATE_THRESHOLD)) {
      ndev->stats.low_data_rate_count++;
    } else if (real_rate != 0.0) {
      ndev->stats.low_data_rate_count = 0;
    }
    // Increase inspect counter when the low data rate counter first reaches threshold.
    if (ndev->stats.low_data_rate_count ==
        BRCMF_LOW_DATA_RATE_DUR_THRESHOLD / BRCMF_CONNECT_LOG_DUR) {
      // Note the low data rate in the inspect logs
      ifp->drvr->device->GetInspect()->LogLowDataRate();
    }
    if (ndev->stats.low_data_rate_count >=
        BRCMF_LOW_DATA_RATE_DUR_RESET_THRESHOLD / BRCMF_CONNECT_LOG_DUR) {
      // Reset the low data rate counter if it has been stuck for this long (so it can be logged
      // into inspect again).
      ndev->stats.low_data_rate_count = 0;
    }
    // The version # in the counters struct returned by FW is set to 10 currently but its
    // corresponding struct definition is not available. It appears each new version is a superset
    // of the previous one. So tell FW the size of the struct is that of wl_cnt_ver_11_t which is >=
    // "wl_cnt_ver_10_t".
    err = brcmf_fil_iovar_data_get(ifp, "counters", cnt_buf, sizeof(wl_cnt_ver_11_t), &fw_err);
    if (err != ZX_OK) {
      BRCMF_WARN("Unable to get fw counters err: %s fw_err %d", zx_status_get_string(err), fw_err);
    } else {
      wl_cnt_ver_6_t* counters = reinterpret_cast<wl_cnt_ver_6_t*>(cnt_buf);

      BRCMF_INFO(
          "FW Err Counts: Tx: retrans: %u err %u serr %u nobuf %u runt %u uflo %u "
          "phyerr %u fail %u noassoc %u noack %u",
          counters->txretrans, counters->txerror, counters->txserr, counters->txnobuf,
          counters->txrunt, counters->txuflo, counters->txphyerr, counters->txfail,
          counters->txnoassoc, counters->txnoack);
      BRCMF_INFO(
          "FW Err Counts: Rx: err %u oflo %u nobuf %u runt %u fragerr %u badplcp %u "
          "crsglitch %u badfcs %u giant %u noscb %u badsrcmac %u",
          counters->rxerror, counters->rxoflo, counters->rxnobuf, counters->rxrunt,
          counters->rxfragerr, counters->rxbadplcp, counters->rxcrsglitch, counters->rxbadfcs,
          counters->rxgiant, counters->rxnoscb, counters->rxbadsrcmac);
      BRCMF_INFO(
          "FW Sec Counts: rxundec %u tkipmicfaill %u tkipcntrmsr %u tkipreplay %u "
          "ccmpfmterr %u ccmpreplay %u ccmpundec %u fourwayfail %u tkipicverr %u",
          counters->rxundec, counters->tkipmicfaill, counters->tkipcntrmsr, counters->tkipreplay,
          counters->ccmpfmterr, counters->ccmpreplay, counters->ccmpundec, counters->fourwayfail,
          counters->tkipicverr);
    }
  }
  ndev->client_stats_log_count++;
}

// Cleanup after a disconnect, or after a disconnect timeout.
// Any pending SME disconnect requests are considered serviced after this function.
static void brcmf_disconnect_done(struct brcmf_cfg80211_info* cfg) {
  struct net_device* ndev = cfg_to_ndev(cfg);
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_profile* profile = &ifp->vif->profile;

  BRCMF_DBG(TRACE, "Enter");

  if (brcmf_test_and_clear_bit(brcmf_vif_status_bit_t::DISCONNECTING, &ifp->vif->sme_state)) {
    cfg->disconnect_timer->Stop();
    if (brcmf_test_and_clear_bit(brcmf_disconnect_request_bit_t::DISASSOC,
                                 &cfg->disconnect_request_state)) {
      brcmf_notify_disassoc(ndev, ZX_OK);
    }
    if (brcmf_test_and_clear_bit(brcmf_disconnect_request_bit_t::DEAUTH_CURRENT_BSS,
                                 &cfg->disconnect_request_state)) {
      brcmf_notify_deauth(ndev, profile->bssid);
    }
    if (brcmf_test_and_clear_bit(brcmf_disconnect_request_bit_t::DEAUTH_TARGET_BSS,
                                 &cfg->disconnect_request_state) &&
        cfg->target_bssid.has_value()) {
      brcmf_notify_deauth(ndev, cfg->target_bssid->data());
    }
    cfg->target_bssid.reset();
    brcmf_clear_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state);
  } else {
    BRCMF_IFDBG(WLANIF, ndev, "Skipped notify deauth/disassoc because not DISCONNECTING");
  }

  if (!brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
    cfg->signal_report_timer->Stop();
    // Log the client stats one last time before clearing out the counters
    brcmf_log_client_stats(cfg);
    ndev->stats = {};
    bcme_status_t fw_err;
    zx_status_t status = brcmf_fil_iovar_data_get(ifp, "reset_cnts", nullptr, 0, &fw_err);
    if (status != ZX_OK) {
      BRCMF_WARN("Failed to clear counters: %s, fw err %s", zx_status_get_string(status),
                 brcmf_fil_get_errstr(fw_err));
    }
    status = brcmf_fil_iovar_data_set(ifp, "wme_clear_counters", nullptr, 0, &fw_err);
    if (status != ZX_OK) {
      BRCMF_WARN("Failed to clear WME counters: %s, fw err %s", zx_status_get_string(status),
                 brcmf_fil_get_errstr(fw_err));
    }
  }

  BRCMF_DBG(TRACE, "Exit");
}

static zx_status_t brcmf_get_rssi_snr(net_device* ndev, int8_t* rssi_dbm, int8_t* snr_db) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  bcme_status_t fw_err = BCME_OK;
  int32_t rssi, snr;

  *rssi_dbm = *snr_db = 0;
  zx_status_t status = brcmf_fil_cmd_data_get(ifp, BRCMF_C_GET_RSSI, &rssi, sizeof(rssi), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("could not get rssi: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    return status;
  }
  status = brcmf_fil_iovar_data_get(ifp, "snr", &snr, sizeof(snr), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("could not get snr: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    return status;
  }
  *rssi_dbm = rssi;
  *snr_db = snr;
  return status;
}

static void cfg80211_signal_ind(net_device* ndev) {
  if (ndev == nullptr) {
    BRCMF_WARN("cfg80211_signal_ind called with null ndev");
    return;
  }
  struct brcmf_if* ifp = ndev_to_if(ndev);
  brcmf_cfg80211_info* cfg = ifp->drvr->config;
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping signal report indication callback");
    // Stop the timer
    cfg->signal_report_timer->Stop();
    return;
  }

  // Send signal report indication only if client is in connected state
  if (brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state)) {
    fuchsia_wlan_fullmac_wire::WlanFullmacSignalReportIndication signal_ind = {};
    int8_t rssi, snr;
    if (brcmf_get_rssi_snr(ndev, &rssi, &snr) == ZX_OK) {
      signal_ind.rssi_dbm = rssi;
      signal_ind.snr_db = snr;

      // Get the negotiated rate
      uint32_t rate = 0;
      bcme_status_t fw_err = BCME_OK;
      int status = brcmf_fil_cmd_data_get(ifp, BRCMF_C_GET_RATE, &rate, sizeof(rate), &fw_err);
      if (status == ZX_OK) {
        signal_ind.tx_rate_500kbps = rate;
      } else {
        BRCMF_INFO("Failed to get rate: %s, fw err %s", zx_status_get_string(status),
                   brcmf_fil_get_errstr(fw_err));
        signal_ind.tx_rate_500kbps = 0;
      }

      // Store the value in ndev (dumped out when link goes down)
      ndev->last_known_rssi_dbm = rssi;
      ndev->last_known_snr_db = snr;
      auto arena = fdf::Arena::Create(0, 0);
      if (arena.is_error()) {
        BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
        return;
      }
      auto result = ndev->if_proto.buffer(*arena)->SignalReport(signal_ind);
      if (!result.ok()) {
        BRCMF_ERR("Failed to send signal report result.status: %s", result.status_string());
        return;
      }
    }
    cfg->connect_log_cnt++;
    if (cfg->connect_log_cnt >= BRCMF_CONNECT_LOG_COUNT) {
      // Log the stats
      brcmf_log_client_stats(cfg);
      cfg->connect_log_cnt = 0;
    }
  } else if (!brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
    // If client is not connected, stop the timer
    cfg->signal_report_timer->Stop();
  }
}

static void brcmf_connect_timeout(struct brcmf_cfg80211_info* cfg) {
  cfg->pub->irq_callback_lock.lock();
  BRCMF_DBG(TRACE, "Enter");
  EXEC_TIMEOUT_WORKER(connect_timeout_work);
  cfg->pub->irq_callback_lock.unlock();
}

static void brcmf_signal_report_worker(WorkItem* work) {
  struct brcmf_cfg80211_info* cfg =
      containerof(work, struct brcmf_cfg80211_info, signal_report_work);
  struct net_device* ndev = cfg_to_ndev(cfg);
  cfg80211_signal_ind(ndev);
}

static void brcmf_signal_report_timeout(struct brcmf_cfg80211_info* cfg) {
  cfg->pub->irq_callback_lock.lock();
  BRCMF_DBG(TRACE, "Enter");
  // If it's for SIM tests, won't enqueue.
  EXEC_TIMEOUT_WORKER(signal_report_work);
  cfg->pub->irq_callback_lock.unlock();
}

static void brcmf_disconnect_timeout_worker(WorkItem* work) {
  struct brcmf_cfg80211_info* cfg =
      containerof(work, struct brcmf_cfg80211_info, disconnect_timeout_work);
  brcmf_disconnect_done(cfg);
}

static void brcmf_disconnect_timeout(struct brcmf_cfg80211_info* cfg) {
  cfg->pub->irq_callback_lock.lock();
  BRCMF_DBG(TRACE, "Enter");

  // If it's for SIM tests, won't enqueue.
  EXEC_TIMEOUT_WORKER(disconnect_timeout_work);

  cfg->pub->irq_callback_lock.unlock();
}

static void brcmf_roam_timeout(struct brcmf_cfg80211_info* cfg) {
  cfg->pub->irq_callback_lock.lock();
  BRCMF_DBG(TRACE, "Enter");
  EXEC_TIMEOUT_WORKER(roam_timeout_work);
  cfg->pub->irq_callback_lock.unlock();
}

// Send a disconnect command to firmware to service an SME disconnect request.
// If the disconnect command fails, or the peer_sta_address is not the current/
// target BSS:
//   - this function will return an error status
//   - and then the caller of this function is responsible for sending any
//     notification to SME
static zx_status_t brcmf_cfg80211_disconnect(struct net_device* ndev,
                                             const uint8_t peer_sta_address[ETH_ALEN],
                                             uint16_t reason_code, bool deauthenticate) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  struct brcmf_scb_val_le scbval;
  zx_status_t status = ZX_OK;
  bcme_status_t fw_err = BCME_OK;

  BRCMF_DBG(TRACE, "Enter. Reason code = %d", reason_code);

  if (!check_vif_up(ifp->vif)) {
    status = ZX_ERR_IO;
    goto done;
  }

  if (!brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state) &&
      !brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state) &&
      !brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    status = ZX_ERR_BAD_STATE;
    BRCMF_INFO("Driver is not CONNECTED/CONNECTING/ROAMING, skipping disconnect");
    goto done;
  }

  if (!(is_current_bss(cfg, peer_sta_address) || is_target_bss(cfg, peer_sta_address))) {
    BRCMF_ERR("peer_sta_address does not match expected BSSID.");
#if !defined(NDEBUG)
    struct brcmf_cfg80211_profile* profile = &ifp->vif->profile;
    BRCMF_DBG(CONN, "  peer_sta_address:" FMT_MAC ", bssid in profile:" FMT_MAC "",
              FMT_MAC_ARGS(peer_sta_address), FMT_MAC_ARGS(profile->bssid));
#endif /* !defined(NDEBUG) */
    status = ZX_ERR_INVALID_ARGS;
    goto done;
  }
  // In case the connection is still in progress, stop the timer
  cfg->connect_timer->Stop();

  // For now, we treat any SME disassoc request, or SME deauth request for current or target BSS, as
  // a full client disconnect. There may be more nuance around this in the future.

  // If roaming is in progress, and disconnect is for target BSS, roam has failed.
  // Note: we don't clear the ROAMING bit here because it will get cleared after SME is notified of
  // the roam failure.
  if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    BRCMF_INFO("Roam has failed, SME requested disconnect while client was roaming");
  }
  brcmf_set_bit(brcmf_vif_status_bit_t::DISCONNECTING, &ifp->vif->sme_state);
  brcmf_clear_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state);
  brcmf_clear_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state);

  // Keep track of when there are disconnect requests waiting for a firmware disconnect command to
  // complete.
  if (deauthenticate) {
    if (is_target_bss(cfg, peer_sta_address)) {
      brcmf_set_bit(brcmf_disconnect_request_bit_t::DEAUTH_TARGET_BSS,
                    &cfg->disconnect_request_state);
    } else {
      brcmf_set_bit(brcmf_disconnect_request_bit_t::DEAUTH_CURRENT_BSS,
                    &cfg->disconnect_request_state);
    }
  } else {
    brcmf_set_bit(brcmf_disconnect_request_bit_t::DISASSOC, &cfg->disconnect_request_state);
  }

  BRCMF_DBG(CONN, "Disconnecting");

  // Set the timer before notifying firmware as this thread might get preempted to
  // handle the response event back from firmware. Timer can be stopped if the command
  // fails.
  cfg->disconnect_timer->Start(BRCMF_DISCONNECT_TIMER_DUR_MS);

  memcpy(&scbval.ea, peer_sta_address, ETH_ALEN);
  scbval.val = reason_code;

  status = brcmf_fil_cmd_data_set(ifp, BRCMF_C_SCB_DEAUTHENTICATE_FOR_REASON, &scbval,
                                  sizeof(scbval), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("Failed to deauthenticate: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    brcmf_clear_bit(brcmf_vif_status_bit_t::DISCONNECTING, &ifp->vif->sme_state);
    brcmf_clear_bit(brcmf_disconnect_request_bit_t::DEAUTH_CURRENT_BSS,
                    &cfg->disconnect_request_state);
    brcmf_clear_bit(brcmf_disconnect_request_bit_t::DEAUTH_TARGET_BSS,
                    &cfg->disconnect_request_state);
    brcmf_clear_bit(brcmf_disconnect_request_bit_t::DISASSOC, &cfg->disconnect_request_state);

    cfg->disconnect_timer->Stop();
  }

done:
  BRCMF_DBG(TRACE, "Exit");
  return status;
}

static zx_status_t brcmf_cfg80211_del_key(struct net_device* ndev, uint8_t key_idx) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_wsec_key* key;
  zx_status_t err;

  BRCMF_DBG(TRACE, "Enter");
  BRCMF_DBG(CONN, "key index (%d)", key_idx);

  if (!check_vif_up(ifp->vif)) {
    return ZX_ERR_IO;
  }

  if (key_idx >= BRCMF_MAX_DEFAULT_KEYS) {
    /* we ignore this key index in this case */
    return ZX_ERR_INVALID_ARGS;
  }

  key = &ifp->vif->profile.key[key_idx];

  if (key->algo == CRYPTO_ALGO_OFF) {
    BRCMF_DBG(CONN, "Ignore clearing of (never configured) key");
    return ZX_ERR_BAD_STATE;
  }

  memset(key, 0, sizeof(*key));
  key->index = static_cast<uint32_t>(key_idx);
  key->flags = BRCMF_PRIMARY_KEY;

  /* Clear the key/index */
  err = send_key_to_dongle(ifp, key);

  BRCMF_DBG(TRACE, "Exit");
  return err;
}

static zx_status_t brcmf_cfg80211_add_key(
    struct net_device* ndev, const fuchsia_wlan_ieee80211::wire::SetKeyDescriptor* req) {
  if (!(req->has_key() && req->has_key_id() && req->has_peer_addr() && req->has_cipher_type())) {
    BRCMF_ERR(
        "Key config missing required fields: has_key %u has_key_id %u has_peer_addr %u has_cipher_type %u",
        req->has_key(), req->has_key_id(), req->has_peer_addr(), req->has_cipher_type());
    return ZX_ERR_INVALID_ARGS;
  }

  if (req->has_cipher_oui()) {
    // If a cipher OUI is specified, check that it's a known valid OUI. Other OUIs indicate a custom
    // cipher type which the broadcom driver doesn't support.

    // IEEE 802.11-2016 3.2 (c.f. "vendor organizationally unique identifier")
    constexpr uint8_t kIeeeOui[] = {0x00, 0x0F, 0xAC};
    constexpr uint8_t kMsftOui[] = {0x00, 0x50, 0xF2};
    auto cipher_oui = req->cipher_oui();
    if (!std::equal(cipher_oui.begin(), cipher_oui.begin() + cipher_oui.size(), kIeeeOui,
                    kIeeeOui + std::size(kIeeeOui)) &&
        !std::equal(cipher_oui.begin(), cipher_oui.begin() + cipher_oui.size(), kMsftOui,
                    kMsftOui + std::size(kMsftOui))) {
      BRCMF_ERR("Cipher OUI does not match either IEEE or MSFT OUI, not supported.");
      return ZX_ERR_NOT_SUPPORTED;
    }
  }
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_wsec_key* key;
  int32_t val;
  int32_t wsec;
  zx_status_t err;
  bool ext_key;
  uint8_t key_idx = req->key_id();
  const uint8_t* mac_addr = req->peer_addr().data();

  BRCMF_DBG(TRACE, "Enter");
  BRCMF_DBG(CONN, "key index (%d)", key_idx);
  if (!check_vif_up(ifp->vif)) {
    return ZX_ERR_IO;
  }

  if (key_idx >= BRCMF_MAX_DEFAULT_KEYS) {
    /* we ignore this key index in this case */
    BRCMF_ERR("invalid key index (%d)", key_idx);
    return ZX_ERR_INVALID_ARGS;
  }

  if (req->key().size() == 0) {
    return brcmf_cfg80211_del_key(ndev, key_idx);
  }

  if (req->key().size() > sizeof(key->data)) {
    BRCMF_ERR("Too long key length (%zu)", req->key().size());
    return ZX_ERR_INVALID_ARGS;
  }

  ext_key = false;
  if (mac_addr && !address_is_multicast(mac_addr) &&
      (req->cipher_type() != fuchsia_wlan_ieee80211_wire::CipherSuiteType::kWep40) &&
      (req->cipher_type() != fuchsia_wlan_ieee80211_wire::CipherSuiteType::kWep104)) {
    BRCMF_DBG(TRACE, "Ext key, mac " FMT_MAC, FMT_MAC_ARGS(mac_addr));
    ext_key = true;
  }

  key = &ifp->vif->profile.key[key_idx];
  memset(key, 0, sizeof(*key));
  if ((ext_key) && (!address_is_multicast(mac_addr))) {
    memcpy((char*)&key->ea, (void*)mac_addr, ETH_ALEN);
  }
  key->len = req->key().size();
  key->index = key_idx;
  memcpy(key->data, req->key().data(), key->len);
  if (!ext_key) {
    key->flags = BRCMF_PRIMARY_KEY;
  }

  switch (req->cipher_type()) {
    case fuchsia_wlan_ieee80211_wire::CipherSuiteType::kWep40:
      key->algo = CRYPTO_ALGO_WEP1;
      val = WEP_ENABLED;
      BRCMF_DBG(CONN, "WPA_CIPHER_WEP_40");
      break;
    case fuchsia_wlan_ieee80211_wire::CipherSuiteType::kWep104:
      key->algo = CRYPTO_ALGO_WEP128;
      val = WEP_ENABLED;
      BRCMF_DBG(CONN, "WPA_CIPHER_WEP_104");
      break;
    case fuchsia_wlan_ieee80211_wire::CipherSuiteType::kTkip:
      /* Note: Linux swaps the Tx and Rx MICs in client mode, but this doesn't work for us (see
         https://fxbug.dev/42103305). It's unclear why this would be necessary. */
      key->algo = CRYPTO_ALGO_TKIP;
      val = TKIP_ENABLED;
      BRCMF_DBG(CONN, "WPA_CIPHER_TKIP");
      break;
    case fuchsia_wlan_ieee80211_wire::CipherSuiteType::kBipCmac128:
      key->algo = CRYPTO_ALGO_AES_CCM;
      val = AES_ENABLED;
      BRCMF_DBG(CONN, "WPA_CIPHER_CMAC_128");
      break;
    case fuchsia_wlan_ieee80211_wire::CipherSuiteType::kCcmp128:
      key->algo = CRYPTO_ALGO_AES_CCM;
      val = AES_ENABLED;
      BRCMF_DBG(CONN, "WPA_CIPHER_CCMP_128");
      break;
    default:
      BRCMF_ERR("Unsupported cipher (0x%x)", fidl::ToUnderlying(req->cipher_type()));
      err = ZX_ERR_INVALID_ARGS;
      goto done;
  }

  BRCMF_DBG(CONN, "key length (%d) key index (%d) algo (%d) flags (%d)", key->len, key->index,
            key->algo, key->flags);
  err = send_key_to_dongle(ifp, key);
  if (err != ZX_OK) {
    goto done;
  }

  if (ext_key) {
    goto done;
  }
  err = brcmf_fil_bsscfg_int_get(ifp, "wsec", (uint32_t*)&wsec);  // TODO(cphoenix): This cast?!?
  if (err != ZX_OK) {
    BRCMF_ERR("get wsec error (%d)", err);
    goto done;
  }
  wsec |= val;
  BRCMF_DBG(CONN, "setting wsec to 0x%x", wsec);
  err = brcmf_fil_bsscfg_int_set(ifp, "wsec", wsec);
  if (err != ZX_OK) {
    BRCMF_ERR("set wsec error (%d)", err);
    goto done;
  }

done:
  BRCMF_DBG(TRACE, "Exit");
  return err;
}

// EAPOL frames are queued up along with event notifications to ensure processing order.
void brcmf_cfg80211_handle_eapol_frame(struct brcmf_if* ifp, const void* data, size_t size) {
  struct net_device* ndev = ifp->ndev;
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping eapol frame callback");
    return;
  }

  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }

  uint8_t* data_bytes = const_cast<uint8_t*>(reinterpret_cast<const uint8_t*>(data));

  fidl::Array<uint8_t, ETH_ALEN> dst_addr;
  memcpy(dst_addr.data(), data_bytes, ETH_ALEN);

  fidl::Array<uint8_t, ETH_ALEN> src_addr;
  memcpy(src_addr.data(), data_bytes + 6, ETH_ALEN);

  const auto eapol_ind =
      fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcEapolIndRequest::Builder(*arena)
          .dst_addr(dst_addr)
          .src_addr(src_addr)
          // The EAPOL PDU is stored at data_bytes + 14.
          // See IEEE Std. 802.1X-2010, 11.3, Figure 11-1 for the format of the EAPOL PDU.
          .data(::fidl::VectorView<uint8_t>::FromExternal(data_bytes + 14, size - 14))
          .Build();

  BRCMF_IFDBG(WLANIF, ndev, "Sending EAPOL frame to SME. data_len: %zu", size - 14);

  auto result = ndev->if_proto.buffer(*arena)->EapolInd(eapol_ind);
  if (!result.ok()) {
    BRCMF_ERR("Failed to send eapol ind result.status: %s", result.status_string());
  }
}

#define EAPOL_ETHERNET_TYPE_UINT16 0x8e88
static bool brcmf_is_eapol_frame(const wlan::drivers::components::Frame& frame) {
  if (frame.Size() >= sizeof(ethhdr)) {
    const uint16_t eth_type = reinterpret_cast<const uint16_t*>(frame.Data())[6];
    return eth_type == EAPOL_ETHERNET_TYPE_UINT16;
  }
  return false;
}

void brcmf_cfg80211_rx(struct brcmf_if* ifp, wlan::drivers::components::Frame&& frame) {
  // First check if this is an EAPOL frame, if it is it should not go back to the network device.
  if (brcmf_is_eapol_frame(frame)) {
    // Queue up the eapol frame along with events to ensure processing order.
    brcmf_fweh_queue_eapol_frame(ifp, frame.Data(), frame.Size());
    return;
  }

  ifp->drvr->device->NetDev().CompleteRx(std::move(frame));
}

void brcmf_cfg80211_rx(struct brcmf_pub* drvr, wlan::drivers::components::FrameContainer&& frames) {
  // Make sure we process and exclude any EAPOL frames first
  for (auto& frame : frames) {
    if (brcmf_is_eapol_frame(frame)) {
      // queue up the eapol frame along with events to ensure processing order
      brcmf_fweh_queue_eapol_frame(brcmf_get_ifp(drvr, frame.PortId()), frame.Data(), frame.Size());
      // The data plane shouldn't do anything with the EAPOL data so set the size to zero here to
      // ensure that it's not processed after this point. The frame will still be passed around, the
      // data just won't be accessed because if it's zero in size there is technically  no data.
      frame.SetSize(0);
    }
  }

  drvr->device->NetDev().CompleteRx(std::move(frames));
}

static void brcmf_iedump(uint8_t* ies, size_t total_len) {
  if (BRCMF_IS_ON(CONN) && BRCMF_IS_ON(BYTES)) {
    size_t offset = 0;
    while (offset + TLV_HDR_LEN <= total_len) {
      uint8_t elem_type = ies[offset];
      uint8_t elem_len = ies[offset + TLV_LEN_OFF];
      offset += TLV_HDR_LEN;
      if (offset + elem_len > total_len) {
        break;
      }
      BRCMF_DBG_HEX_DUMP(true, ies + offset, elem_len, "IE %d, len %d:", elem_type, elem_len);
      offset += elem_len;
    }
    if (offset != total_len) {
      BRCMF_DBG(ALL, " * * Offset %ld didn't match length %ld", offset, total_len);
    }
  }
}

static void brcmf_return_scan_result(struct net_device* ndev, uint16_t chanspec,
                                     const uint8_t* bssid, uint16_t capability, uint16_t interval,
                                     uint8_t* ie, size_t ie_len, int16_t rssi_dbm,
                                     uint16_t snr_db) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  struct brcmf_cfg80211_info* cfg = ndev_to_if(ndev)->drvr->config;
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping scan result callback");
    return;
  }
  if (!brcmf_test_bit(brcmf_scan_status_bit_t::BUSY, &cfg->scan_status)) {
    return;
  }
  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR(
        "Failed to create Arena in WlanFullmacIfc::OnScanResult(). "
        "status=%s",
        arena.status_string());
    return;
  }
  auto scan_result_builder =
      fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcOnScanResultRequest::Builder(*arena);
  fuchsia_wlan_ieee80211::BssType bss_type = fuchsia_wlan_ieee80211::BssType::kInfrastructure;
  fuchsia_wlan_ieee80211::wire::BssDescription bss;

  if ((capability & IEEE80211_BCN_CAPS_ESS) && !(capability & IEEE80211_BCN_CAPS_IBSS)) {
    bss_type = fuchsia_wlan_ieee80211::BssType::kInfrastructure;
  } else if (!(capability & IEEE80211_BCN_CAPS_ESS) && (capability & IEEE80211_BCN_CAPS_IBSS)) {
    bss_type = fuchsia_wlan_ieee80211::BssType::kIndependent;
  } else if (!(capability & IEEE80211_BCN_CAPS_ESS) && !(capability & IEEE80211_BCN_CAPS_IBSS)) {
    bss_type = fuchsia_wlan_ieee80211::BssType::kMesh;
  }

  scan_result_builder.txn_id(ndev->scan_txn_id);
  scan_result_builder.timestamp_nanos(zx::clock::get_monotonic().get());
  memcpy(bss.bssid.data(), bssid, ETH_ALEN);
  bss.bss_type = bss_type;
  bss.beacon_period = 0;
  bss.capability_info = capability;

  // The rest of the stack expects the control channel to be provided in the BssDescription.
  bss.primary = chanspec_to_primary_channel_number(&cfg->d11inf, chanspec);
  bss.bandwidth = chanspec_to_channel_bandwidth(&cfg->d11inf, chanspec);
  bss.vht_secondary_80_channel = chanspec_to_secondary80(&cfg->d11inf, chanspec);
  bss.rssi_dbm = std::min<int16_t>(0, std::max<int16_t>(-255, rssi_dbm));
  bss.snr_db = static_cast<int8_t>(snr_db);
  bss.ies = ::fidl::VectorView<uint8_t>::FromExternal(ie, ie_len);
  scan_result_builder.bss(bss);

  BRCMF_DBG(SCAN, "Returning scan result id: %lu, channel: %d, dbm: %d", ndev->scan_txn_id,
            bss.primary.number, bss.rssi_dbm);
#if !defined(NDEBUG)
  auto ssid = brcmf_find_ssid_in_ies(bss.ies.data(), bss.ies.size());
  BRCMF_DBG(SCAN, "  ssid: " FMT_SSID, FMT_SSID_VECT(ssid));
#endif /* !defined(NDEBUG) */
  ndev->scan_num_results++;
  auto status = ndev->if_proto.buffer(*arena)->OnScanResult(scan_result_builder.Build());
  if (!status.ok()) {
    BRCMF_ERR(
        "Failed to WlanScanResult up in WlanFullmacIfc::OnScanResult(). "
        "result.status: %s, txn_id=%zu",
        status.status_string(), ndev->scan_txn_id);
    return;
  }
}

static zx_status_t brcmf_inform_single_bss(struct net_device* ndev, struct brcmf_cfg80211_info* cfg,
                                           struct brcmf_bss_info_le* bi) {
  uint16_t channel;
  uint16_t notify_capability;
  uint16_t notify_interval;
  uint8_t* notify_ie;
  size_t notify_ielen;
  int16_t notify_rssi_dbm;
  uint16_t notify_snr_db;

  if (bi->length > WL_BSS_INFO_MAX) {
    BRCMF_ERR("Bss info is larger than buffer. Discarding");
    BRCMF_DBG(TEMP, "Early return, due to length.");
    return ZX_OK;
  }

  if (!bi->ctl_ch) {
    bi->ctl_ch = chanspec_to_primary_channel_number(&cfg->d11inf, bi->chanspec).number;
  } else {
    uint16_t derived_channel =
        chanspec_to_primary_channel_number(&cfg->d11inf, bi->chanspec).number;
    if (bi->ctl_ch != derived_channel) {
      BRCMF_WARN("Received channel (%3d) does not match derived channel (%3d).", bi->ctl_ch,
                 derived_channel);
    }
  }
  channel = bi->ctl_ch;

  notify_capability = bi->capability;
  notify_interval = bi->beacon_period;
  notify_ie = (uint8_t*)bi + bi->ie_offset;
  notify_ielen = bi->ie_length;
  notify_rssi_dbm = (int16_t)bi->RSSI;
  notify_snr_db = bi->SNR;

  BRCMF_DBG(CONN,
            "Scan result received  BSS: " FMT_MAC
            "  Channel: %3d  chanspec: 0x%x Capability: %#6x  Beacon interval: %5d  Signal: %4d"
            " SNR: %d",
            FMT_MAC_ARGS(bi->BSSID), channel, bi->chanspec, notify_capability, notify_interval,
            notify_rssi_dbm, notify_snr_db);
  if (BRCMF_IS_ON(CONN) && BRCMF_IS_ON(BYTES)) {
    brcmf_iedump(notify_ie, notify_ielen);
  }

  brcmf_return_scan_result(ndev, bi->chanspec, (const uint8_t*)bi->BSSID, notify_capability,
                           notify_interval, notify_ie, notify_ielen, notify_rssi_dbm,
                           notify_snr_db);

  return ZX_OK;
}

static zx_status_t brcmf_abort_scanning(struct brcmf_cfg80211_info* cfg) {
  struct escan_info* escan = &cfg->escan_info;
  zx_status_t err = ZX_OK;

  if (brcmf_test_and_set_bit(brcmf_scan_status_bit_t::ABORT, &cfg->scan_status)) {
    BRCMF_INFO("Abort scan already in progress.");
    return ZX_OK;
  }

  if (cfg->scan_in_progress) {
    escan->escan_state = WL_ESCAN_STATE_IDLE;
    if ((err = brcmf_abort_escan(escan->ifp)) != ZX_OK) {
      BRCMF_ERR("Abort scan failed -- error: %s", zx_status_get_string(err));
    }
  }
  brcmf_clear_bit(brcmf_scan_status_bit_t::ABORT, &cfg->scan_status);
  return err;
}

// Abort scanning immediately and inform SME right away
static void brcmf_abort_scanning_immediately(struct brcmf_cfg80211_info* cfg) {
  brcmf_abort_scanning(cfg);
  if (cfg->scan_in_progress) {
    brcmf_notify_escan_complete(cfg, cfg->escan_info.ifp, BRCMF_E_STATUS_ABORT);
  }
}

static void brcmf_cfg80211_escan_timeout_worker(WorkItem* work) {
  struct brcmf_cfg80211_info* cfg =
      containerof(work, struct brcmf_cfg80211_info, escan_timeout_work);

  BRCMF_WARN("Scan timed out, sending notification of aborted scan");
  brcmf_abort_scanning_immediately(cfg);
}

static void brcmf_escan_timeout(struct brcmf_cfg80211_info* cfg) {
  cfg->pub->irq_callback_lock.lock();

  if (cfg->scan_in_progress) {
    BRCMF_ERR("scan timer expired");
    // If it's for SIM tests, won't enqueue.
    EXEC_TIMEOUT_WORKER(escan_timeout_work);
  }
  cfg->pub->irq_callback_lock.unlock();
}

static zx_status_t brcmf_cfg80211_is_valid_sync_id(net_device* ndev,
                                                   const brcmf_escan_result_le* result,
                                                   uint32_t size) {
  std::lock_guard sync_id_lock(ndev->scan_sync_id_mutex);
  if (size < sizeof(result->sync_id) + offsetof(brcmf_escan_result_le, sync_id)) {
    BRCMF_ERR("Invalid escan result, not enough data in result, %u available", size);
    return false;
  }
  if (result->sync_id != ndev->scan_sync_id) {
    BRCMF_ERR("Invalid escan result with sync_id %u, current scan_sync_id %u", result->sync_id,
              ndev->scan_sync_id);
    return false;
  }
  return true;
}

static zx_status_t brcmf_cfg80211_escan_handler(struct brcmf_if* ifp,
                                                const struct brcmf_event_msg* e, void* data) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  struct net_device* ndev = cfg_to_ndev(cfg);
  brcmf_fweh_event_status_t status = e->status;
  uint32_t escan_buflen;
  struct brcmf_bss_info_le* bss_info_le;
  auto escan_result_le = static_cast<struct brcmf_escan_result_le*>(data);

  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });

  if (!escan_result_le) {
    BRCMF_ERR("Invalid escan result (nullptr)");
    goto chk_scan_end;
  }

  if (!brcmf_cfg80211_is_valid_sync_id(ndev, escan_result_le, e->datalen)) {
    return ZX_ERR_UNAVAILABLE;
  }

  if (status == BRCMF_E_STATUS_ABORT) {
    BRCMF_INFO("Firmware aborted escan: %d", e->reason);
    goto chk_scan_end;
  }

  if (!brcmf_test_bit(brcmf_scan_status_bit_t::BUSY, &cfg->scan_status)) {
    BRCMF_ERR("scan not ready, bsscfgidx=%d", ifp->bsscfgidx);
    return ZX_ERR_UNAVAILABLE;
  }

  bss_info_le = &escan_result_le->bss_info_le;

  if (e->datalen < sizeof(*escan_result_le)) {
    // Print the error only if the scan result is partial (as end of scan may not
    // contain a scan result)
    if (status == BRCMF_E_STATUS_PARTIAL) {
      BRCMF_ERR("Insufficient escan result data exp: %lu got: %d", sizeof(*escan_result_le),
                e->datalen);
    }
    goto chk_scan_end;
  }

  escan_buflen = escan_result_le->buflen;
  if (escan_buflen > BRCMF_ESCAN_BUF_SIZE || escan_buflen > e->datalen ||
      escan_buflen < sizeof(*escan_result_le)) {
    BRCMF_ERR("Invalid escan buffer length: %d", escan_buflen);
    goto chk_scan_end;
  }

  if (escan_result_le->bss_count != 1) {
    BRCMF_ERR("Invalid bss_count %d: ignoring", escan_result_le->bss_count);
    goto chk_scan_end;
  }

  if (!cfg->scan_in_progress) {
    BRCMF_DBG(SCAN, "result without cfg80211 request");
    goto chk_scan_end;
  }

  if (bss_info_le->length != escan_buflen - WL_ESCAN_RESULTS_FIXED_SIZE) {
    BRCMF_ERR("Ignoring invalid bss_info length: %d", bss_info_le->length);
    goto chk_scan_end;
  }

  brcmf_inform_single_bss(ndev, cfg, bss_info_le);

  if (status == BRCMF_E_STATUS_PARTIAL) {
    BRCMF_DBG(SCAN, "ESCAN Partial result");
    goto done;
  }

chk_scan_end:
  // If this is not a partial notification, indicate scan complete to wlanstack
  if (status != BRCMF_E_STATUS_PARTIAL) {
    cfg->escan_info.escan_state = WL_ESCAN_STATE_IDLE;
    if (cfg->scan_in_progress) {
      brcmf_notify_escan_complete(cfg, ifp, status);
    } else {
      BRCMF_DBG(SCAN, "Ignored scan complete result 0x%x", status);
    }
  }

done:
  return ZX_OK;
}

static void brcmf_init_escan(struct brcmf_cfg80211_info* cfg) {
  brcmf_fweh_register(cfg->pub, BRCMF_E_ESCAN_RESULT, brcmf_cfg80211_escan_handler);
  cfg->escan_info.escan_state = WL_ESCAN_STATE_IDLE;
  /* Init scan_timeout timer */
  cfg->escan_timer = new Timer(
      cfg->pub->device->GetTimerDispatcher(), [cfg] { brcmf_escan_timeout(cfg); },
      Timer::Type::OneShot);
  cfg->escan_timeout_work = WorkItem(brcmf_cfg80211_escan_timeout_worker);
}

static zx_status_t brcmf_parse_vndr_ies(const uint8_t* vndr_ie_buf, uint32_t vndr_ie_len,
                                        struct parsed_vndr_ies* vndr_ies) {
  struct brcmf_vs_tlv* vndrie;
  struct brcmf_tlv* ie;
  struct parsed_vndr_ie_info* parsed_info;
  int32_t remaining_len;

  remaining_len = (int32_t)vndr_ie_len;
  memset(vndr_ies, 0, sizeof(*vndr_ies));

  ie = (struct brcmf_tlv*)vndr_ie_buf;
  while (ie) {
    if (ie->id != WLAN_IE_TYPE_VENDOR_SPECIFIC) {
      goto next;
    }
    vndrie = (struct brcmf_vs_tlv*)ie;
    /* len should be bigger than OUI length + one */
    if (vndrie->len < (VS_IE_FIXED_HDR_LEN - TLV_HDR_LEN + 1)) {
      BRCMF_ERR("invalid vndr ie. length is too small %d", vndrie->len);
      goto next;
    }
    /* if wpa or wme ie, do not add ie */
    if (!memcmp(vndrie->oui, (uint8_t*)MSFT_OUI, TLV_OUI_LEN) &&
        ((vndrie->oui_type == WPA_OUI_TYPE) || (vndrie->oui_type == WME_OUI_TYPE))) {
      BRCMF_DBG(TRACE, "Found WPA/WME oui. Do not add it");
      goto next;
    }

    parsed_info = &vndr_ies->ie_info[vndr_ies->count];

    /* save vndr ie information */
    parsed_info->ie_ptr = (uint8_t*)vndrie;
    parsed_info->ie_len = vndrie->len + TLV_HDR_LEN;
    memcpy(&parsed_info->vndrie, vndrie, sizeof(*vndrie));

    vndr_ies->count++;

    BRCMF_DBG(TRACE, "** OUI %02x %02x %02x, type 0x%02x", parsed_info->vndrie.oui[0],
              parsed_info->vndrie.oui[1], parsed_info->vndrie.oui[2], parsed_info->vndrie.oui_type);

    if (vndr_ies->count >= VNDR_IE_PARSE_LIMIT) {
      break;
    }
  next:
    remaining_len -= (ie->len + TLV_HDR_LEN);
    if (remaining_len <= TLV_HDR_LEN) {
      ie = nullptr;
    } else {
      ie = (struct brcmf_tlv*)(((uint8_t*)ie) + ie->len + TLV_HDR_LEN);
    }
  }
  return ZX_OK;
}

static uint32_t brcmf_vndr_ie(uint8_t* iebuf, int32_t pktflag, uint8_t* ie_ptr, uint32_t ie_len,
                              int8_t* add_del_cmd) {
  strncpy((char*)iebuf, (char*)add_del_cmd, VNDR_IE_CMD_LEN - 1);
  iebuf[VNDR_IE_CMD_LEN - 1] = '\0';

  *(uint32_t*)&iebuf[VNDR_IE_COUNT_OFFSET] = 1;

  *(uint32_t*)&iebuf[VNDR_IE_PKTFLAG_OFFSET] = pktflag;

  memcpy(&iebuf[VNDR_IE_VSIE_OFFSET], ie_ptr, ie_len);

  return ie_len + VNDR_IE_HDR_SIZE;
}

zx_status_t brcmf_vif_set_mgmt_ie(struct brcmf_cfg80211_vif* vif, int32_t pktflag,
                                  const uint8_t* vndr_ie_buf, uint32_t vndr_ie_len) {
  struct brcmf_if* ifp;
  struct vif_saved_ie* saved_ie;
  zx_status_t err = ZX_OK;
  uint8_t* iovar_ie_buf;
  uint8_t* curr_ie_buf;
  uint8_t* mgmt_ie_buf = nullptr;
  int mgmt_ie_buf_len;
  uint32_t* mgmt_ie_len;
  uint32_t del_add_ie_buf_len = 0;
  uint32_t total_ie_buf_len = 0;
  uint32_t parsed_ie_buf_len = 0;
  struct parsed_vndr_ies old_vndr_ies;
  struct parsed_vndr_ies new_vndr_ies;
  struct parsed_vndr_ie_info* vndrie_info;
  int32_t i;
  uint8_t* ptr;
  int remained_buf_len;

  if (!vif) {
    return ZX_ERR_IO_NOT_PRESENT;
  }
  ifp = vif->ifp;
  saved_ie = &vif->saved_ie;

  BRCMF_DBG(TRACE, "bsscfgidx %d, pktflag : 0x%02X", ifp->bsscfgidx, pktflag);
  iovar_ie_buf = static_cast<decltype(iovar_ie_buf)>(calloc(1, WL_EXTRA_BUF_MAX));
  if (!iovar_ie_buf) {
    return ZX_ERR_NO_MEMORY;
  }
  curr_ie_buf = iovar_ie_buf;
  switch (pktflag) {
    case BRCMF_VNDR_IE_PRBREQ_FLAG:
      mgmt_ie_buf = saved_ie->probe_req_ie;
      mgmt_ie_len = &saved_ie->probe_req_ie_len;
      mgmt_ie_buf_len = sizeof(saved_ie->probe_req_ie);
      break;
    case BRCMF_VNDR_IE_PRBRSP_FLAG:
      mgmt_ie_buf = saved_ie->probe_res_ie;
      mgmt_ie_len = &saved_ie->probe_res_ie_len;
      mgmt_ie_buf_len = sizeof(saved_ie->probe_res_ie);
      break;
    case BRCMF_VNDR_IE_BEACON_FLAG:
      mgmt_ie_buf = saved_ie->beacon_ie;
      mgmt_ie_len = &saved_ie->beacon_ie_len;
      mgmt_ie_buf_len = sizeof(saved_ie->beacon_ie);
      break;
    case BRCMF_VNDR_IE_ASSOCREQ_FLAG:
      mgmt_ie_buf = saved_ie->assoc_req_ie;
      mgmt_ie_len = &saved_ie->assoc_req_ie_len;
      mgmt_ie_buf_len = sizeof(saved_ie->assoc_req_ie);
      break;
    default:
      err = ZX_ERR_WRONG_TYPE;
      BRCMF_ERR("not suitable type");
      goto exit;
  }

  if ((int)vndr_ie_len > mgmt_ie_buf_len) {
    err = ZX_ERR_NO_MEMORY;
    BRCMF_ERR("extra IE size too big");
    goto exit;
  }

  /* parse and save new vndr_ie in curr_ie_buff before comparing it */
  if (vndr_ie_buf && vndr_ie_len && curr_ie_buf) {
    ptr = curr_ie_buf;
    brcmf_parse_vndr_ies(vndr_ie_buf, vndr_ie_len, &new_vndr_ies);
    for (i = 0; i < (int32_t)new_vndr_ies.count; i++) {
      vndrie_info = &new_vndr_ies.ie_info[i];
      memcpy(ptr + parsed_ie_buf_len, vndrie_info->ie_ptr, vndrie_info->ie_len);
      parsed_ie_buf_len += vndrie_info->ie_len;
    }
  }

  if (mgmt_ie_buf && *mgmt_ie_len) {
    if (parsed_ie_buf_len && (parsed_ie_buf_len == *mgmt_ie_len) &&
        (memcmp(mgmt_ie_buf, curr_ie_buf, parsed_ie_buf_len) == 0)) {
      BRCMF_DBG(TRACE, "Previous mgmt IE equals to current IE");
      goto exit;
    }

    /* parse old vndr_ie */
    brcmf_parse_vndr_ies(mgmt_ie_buf, *mgmt_ie_len, &old_vndr_ies);

    /* make a command to delete old ie */
    for (i = 0; i < (int32_t)old_vndr_ies.count; i++) {
      vndrie_info = &old_vndr_ies.ie_info[i];

      BRCMF_DBG(TRACE, "DEL ID : %d, Len: %d , OUI:%02x:%02x:%02x", vndrie_info->vndrie.id,
                vndrie_info->vndrie.len, vndrie_info->vndrie.oui[0], vndrie_info->vndrie.oui[1],
                vndrie_info->vndrie.oui[2]);

      del_add_ie_buf_len = brcmf_vndr_ie(curr_ie_buf, pktflag, vndrie_info->ie_ptr,
                                         vndrie_info->ie_len, (int8_t*)"del");
      curr_ie_buf += del_add_ie_buf_len;
      total_ie_buf_len += del_add_ie_buf_len;
    }
  }

  *mgmt_ie_len = 0;
  /* Add if there is any extra IE */
  if (mgmt_ie_buf && parsed_ie_buf_len) {
    ptr = mgmt_ie_buf;

    remained_buf_len = mgmt_ie_buf_len;

    /* make a command to add new ie */
    for (i = 0; i < (int32_t)new_vndr_ies.count; i++) {
      vndrie_info = &new_vndr_ies.ie_info[i];

      /* verify remained buf size before copy data */
      if (remained_buf_len < (vndrie_info->vndrie.len + VNDR_IE_VSIE_OFFSET)) {
        BRCMF_ERR("no space in mgmt_ie_buf: len left %d", remained_buf_len);
        break;
      }
      remained_buf_len -= (vndrie_info->ie_len + VNDR_IE_VSIE_OFFSET);

      BRCMF_DBG(TRACE, "ADDED ID : %d, Len: %d, OUI:%02x:%02x:%02x", vndrie_info->vndrie.id,
                vndrie_info->vndrie.len, vndrie_info->vndrie.oui[0], vndrie_info->vndrie.oui[1],
                vndrie_info->vndrie.oui[2]);

      del_add_ie_buf_len = brcmf_vndr_ie(curr_ie_buf, pktflag, vndrie_info->ie_ptr,
                                         vndrie_info->ie_len, (int8_t*)"add");

      /* save the parsed IE in wl struct */
      memcpy(ptr + (*mgmt_ie_len), vndrie_info->ie_ptr, vndrie_info->ie_len);
      *mgmt_ie_len += vndrie_info->ie_len;

      curr_ie_buf += del_add_ie_buf_len;
      total_ie_buf_len += del_add_ie_buf_len;
    }
  }
  if (total_ie_buf_len) {
    err = brcmf_fil_bsscfg_data_set(ifp, "vndr_ie", iovar_ie_buf, total_ie_buf_len);
    if (err != ZX_OK) {
      BRCMF_ERR("vndr ie set error : %d", err);
    }
  }

exit:
  free(iovar_ie_buf);
  return err;
}

zx_status_t brcmf_vif_clear_mgmt_ies(struct brcmf_cfg80211_vif* vif) {
  int32_t pktflags[] = {BRCMF_VNDR_IE_PRBREQ_FLAG, BRCMF_VNDR_IE_PRBRSP_FLAG,
                        BRCMF_VNDR_IE_BEACON_FLAG};
  int i;

  for (i = 0; i < (int)std::size(pktflags); i++) {
    brcmf_vif_set_mgmt_ie(vif, pktflags[i], nullptr, 0);
  }

  memset(&vif->saved_ie, 0, sizeof(vif->saved_ie));
  return ZX_OK;
}

bool brcmf_is_ap_start_pending(brcmf_cfg80211_info* cfg) {
  struct net_device* softap_ndev = cfg_to_softap_ndev(cfg);

  // No softAP interface
  if (softap_ndev == nullptr) {
    return false;
  }

  struct brcmf_cfg80211_vif* vif = ndev_to_vif(softap_ndev);
  return brcmf_test_bit(brcmf_vif_status_bit_t::AP_START_PENDING, &vif->sme_state);
}

// Deauthenticate with specified STA.
static fuchsia_wlan_fullmac_wire::StopResult brcmf_cfg80211_stop_ap(struct net_device* ndev) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  zx_status_t status;
  bcme_status_t fw_err = BCME_OK;
  auto result = fuchsia_wlan_fullmac_wire::StopResult::kSuccess;
  struct brcmf_join_params join_params;
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  if (!brcmf_test_bit(brcmf_vif_status_bit_t::AP_CREATED, &ifp->vif->sme_state) &&
      !brcmf_test_bit(brcmf_vif_status_bit_t::AP_START_PENDING, &ifp->vif->sme_state)) {
    BRCMF_INFO("attempt to stop already stopped AP");
    return fuchsia_wlan_fullmac_wire::StopResult::kBssAlreadyStopped;
  }

  // If we are in the process of resetting, then ap interface no longer exists
  // in firmware (since fw has been reloaded). We can skip sending commands
  // related to destorying the interface.
  if (ifp->drvr->drvr_resetting.load()) {
    goto skip_fw_cmds;
  }

  memset(&join_params, 0, sizeof(join_params));
  status =
      brcmf_fil_cmd_data_set(ifp, BRCMF_C_SET_SSID, &join_params, sizeof(join_params), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("SET SSID error: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    result = fuchsia_wlan_fullmac_wire::StopResult::kInternalError;
  }

  // Issue "bss" iovar to bring down the SoftAP IF.
  brcmf_bss_ctrl bss_down;
  bss_down.bsscfgidx = ifp->bsscfgidx;
  bss_down.value = 0;
  status = brcmf_fil_bsscfg_data_set(ifp, "bss", &bss_down, sizeof(bss_down));
  if (status != ZX_OK) {
    // If "bss" fails, issue C_DOWN/UP to cleanly shutdown the SoftAP
    status = brcmf_bss_reset(ifp);
    if (status != ZX_OK) {
      BRCMF_ERR("BRCMF_C_DOWN/UP error: %s, fw err %s", zx_status_get_string(status),
                brcmf_fil_get_errstr(fw_err));
    }
  }

  // Disable AP mode in MFG build since the IF is shared.
  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
    status = brcmf_fil_cmd_int_set(ifp, BRCMF_C_SET_AP, 0, &fw_err);
    if (status != ZX_OK) {
      BRCMF_ERR("Unset AP mode failed %s, fw err %s", zx_status_get_string(status),
                brcmf_fil_get_errstr(fw_err));
    }
  }
  brcmf_vif_clear_mgmt_ies(ifp->vif);

  // ap_started must be unset for brcmf_enable_mpc() to take effect.
  cfg->ap_started = false;
  brcmf_enable_mpc(ifp, 1);

skip_fw_cmds:
  cfg->ap_started = false;
  brcmf_clear_bit(brcmf_vif_status_bit_t::AP_START_PENDING, &ifp->vif->sme_state);
  brcmf_clear_bit(brcmf_vif_status_bit_t::AP_CREATED, &ifp->vif->sme_state);
  brcmf_net_setcarrier(ifp, false);
  ifp->saved_softap_ssid = {};

  return result;
}

// Returns an MLME result code (WLAN_START_RESULT_*) if an error is encountered.
// If all iovars succeed, MLME is notified when E_LINK event is received.
static fuchsia_wlan_fullmac_wire::StartResult brcmf_cfg80211_start_ap(
    struct net_device* ndev, const fuchsia_wlan_fullmac_wire::WlanFullmacImplStartBssRequest* req) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  if (brcmf_test_bit(brcmf_vif_status_bit_t::AP_CREATED, &ifp->vif->sme_state)) {
    BRCMF_ERR("AP already started");
    return fuchsia_wlan_fullmac_wire::StartResult::kBssAlreadyStartedOrJoined;
  }

  if (brcmf_test_bit(brcmf_vif_status_bit_t::AP_START_PENDING, &ifp->vif->sme_state)) {
    BRCMF_ERR("AP start request received, start pending");
    return fuchsia_wlan_fullmac_wire::StartResult::kBssAlreadyStartedOrJoined;
  }

  if (req->bss_type() != fuchsia_wlan_ieee80211::BssType::kInfrastructure) {
    BRCMF_ERR("Attempt to start AP in unsupported mode (%d)", fidl::ToUnderlying(req->bss_type()));
    return fuchsia_wlan_fullmac_wire::StartResult::kNotSupported;
  }

  if (ifp->vif->mbss) {
    BRCMF_ERR("Mesh role not yet supported");
    return fuchsia_wlan_fullmac_wire::StartResult::kNotSupported;
  }

  // Enter AP_START_PENDING mode before we abort any on-going scans. As soon as
  // we abort a scan we're open for other scans coming in and we want to make
  // sure those scans are blocked by setting this bit.
  brcmf_set_bit(brcmf_vif_status_bit_t::AP_START_PENDING, &ifp->vif->sme_state);

  if (brcmf_test_bit(brcmf_scan_status_bit_t::BUSY, &cfg->scan_status)) {
    std::string scan_status_report;
    brcmf_check_scan_status(cfg->scan_status.load(), &scan_status_report);
    BRCMF_INFO("AP start request incoming during scan_status %s", scan_status_report.c_str());
    BRCMF_INFO("Aborting scan to continue AP start request.");
    brcmf_abort_scanning(cfg);
  }

  BRCMF_DBG(TRACE,
            "ssid: " FMT_SSID "  beacon period: %d  dtim_period: %d  channel: %d  rsne_len: %zd",
            FMT_SSID_BYTES(req->ssid().data(), req->ssid().size()), req->beacon_period(),
            req->dtim_period(), req->primary().number, req->has_rsne() ? req->rsne().size() : 0);

  zx_status_t status;
  bcme_status_t fw_err = BCME_OK;
  zx::result<chanspec_t> chanspec =
      channel_to_chanspec(&cfg->d11inf, req->primary().number, req->primary().band,
                          fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20);
  if (chanspec.is_error()) {
    BRCMF_ERR("Failed to get chanspec: %s", chanspec.status_string());
    goto fail;
  }

  struct brcmf_ssid_le ssid_le;
  memset(&ssid_le, 0, sizeof(ssid_le));
  memcpy(ssid_le.SSID, req->ssid().data(), req->ssid().size());
  ssid_le.SSID_len = req->ssid().size();

  brcmf_enable_mpc(ifp, 0);

  // Start timer before starting to issue commands.
  cfg->ap_start_timer->Start(BRCMF_AP_START_TIMER_DUR_MS);
  // set to open authentication for external supplicant
  status = brcmf_fil_bsscfg_int_set(ifp, "auth", BRCMF_AUTH_MODE_OPEN);
  if (status != ZX_OK) {
    BRCMF_ERR("auth error %s", zx_status_get_string(status));
    goto fail;
  }

  // Configure RSN IE
  if (req->has_rsne() && req->rsne().size() != 0) {
    struct brcmf_vs_tlv* tmp_ie = (struct brcmf_vs_tlv*)req->rsne().data();
    status = brcmf_configure_wpaie(ifp, tmp_ie, true, true);
    if (status != ZX_OK) {
      BRCMF_ERR("Failed to install RSNE: %s", zx_status_get_string(status));
      goto fail;
    }
  } else {
    status = brcmf_configure_opensecurity(ifp);
    if (status != ZX_OK) {
      BRCMF_ERR("Failed to configure AP for open security: %s", zx_status_get_string(status));
      goto fail;
    }
  }

  status = brcmf_fil_cmd_int_set(ifp, BRCMF_C_SET_BCNPRD, req->beacon_period(), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("Beacon Interval Set Error: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    goto fail;
  }
  ifp->vif->profile.beacon_period = req->beacon_period();

  status = brcmf_fil_cmd_int_set(ifp, BRCMF_C_SET_DTIMPRD, req->dtim_period(), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("DTIM Interval Set Error: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    goto fail;
  }

  // If we are operating with manufacturing FW, we have access to just one IF
  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
    status = brcmf_fil_cmd_int_set(ifp, BRCMF_C_DOWN, 1, &fw_err);
    if (status != ZX_OK) {
      BRCMF_ERR("BRCMF_C_DOWN error %s, fw err %s", zx_status_get_string(status),
                brcmf_fil_get_errstr(fw_err));
      goto fail;
    }
    // Disable simultaneous STA/AP operation
    status = brcmf_fil_iovar_int_set(ifp, "apsta", 0, &fw_err);
    if (status != ZX_OK) {
      BRCMF_ERR("Set apsta error %s, fw err %s", zx_status_get_string(status),
                brcmf_fil_get_errstr(fw_err));
      goto fail;
    }
  }

  status = brcmf_fil_cmd_int_set(ifp, BRCMF_C_SET_INFRA, 1, &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("SET INFRA error %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    goto fail;
  }

  status = brcmf_fil_cmd_int_set(ifp, BRCMF_C_SET_AP, 1, &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("Set AP mode failed %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    goto fail;
  }

  status = brcmf_fil_iovar_int_set(ifp, "chanspec", chanspec.value(), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("Set Channel failed: chspec=%d, status=%s, fw_err=%s", chanspec.value(),
              zx_status_get_string(status), brcmf_fil_get_errstr(fw_err));
    goto fail;
  }

  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
    status = brcmf_fil_cmd_int_set(ifp, BRCMF_C_UP, 1, &fw_err);
    if (status != ZX_OK) {
      BRCMF_ERR("BRCMF_C_UP error: %s, fw err %s", zx_status_get_string(status),
                brcmf_fil_get_errstr(fw_err));
      goto fail;
    }
  }
  struct brcmf_join_params join_params;
  memset(&join_params, 0, sizeof(join_params));
  // join parameters starts with ssid
  memcpy(&join_params.ssid_le, &ssid_le, sizeof(ssid_le));
  // create softap
  status =
      brcmf_fil_cmd_data_set(ifp, BRCMF_C_SET_SSID, &join_params, sizeof(join_params), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("SET SSID error: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    if (fw_err == BCME_ERROR) {
      ifp->drvr->device->GetInspect()->LogApSetSsidErr();
    }
    goto fail;
  }

  BRCMF_DBG(TRACE, "AP mode configuration complete");

  brcmf_net_setcarrier(ifp, true);

  cfg->ap_started = true;
  // Save the SSID for checking when SoftAP is stopped.
  ifp->saved_softap_ssid.resize(req->ssid().size());
  memcpy(ifp->saved_softap_ssid.data(), req->ssid().data(), req->ssid().size());
  return fuchsia_wlan_fullmac_wire::StartResult::kSuccess;

fail:
  // Stop the timer when the function fails to issue any of the commands.
  cfg->ap_start_timer->Stop();
  // Unconditionally stop the AP as some of the iovars might have succeeded and
  // thus the SoftAP might have been partially started.
  brcmf_cfg80211_stop_ap(ndev);

  return fuchsia_wlan_fullmac_wire::StartResult::kNotSupported;
}

static zx_status_t brcmf_cfg80211_del_station(struct net_device* ndev, const uint8_t* mac,
                                              fuchsia_wlan_ieee80211::ReasonCode reason) {
  BRCMF_DBG(TRACE, "Enter: reason: %d", fidl::ToUnderlying(reason));

  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_scb_val_le scbval;
  memset(&scbval, 0, sizeof(scbval));
  memcpy(&scbval.ea, mac, ETH_ALEN);
  scbval.val = static_cast<uint16_t>(reason);
  bcme_status_t fw_err = BCME_OK;
  zx_status_t status = brcmf_fil_cmd_data_set(ifp, BRCMF_C_SCB_DEAUTHENTICATE_FOR_REASON, &scbval,
                                              sizeof(scbval), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("SCB_DEAUTHENTICATE_FOR_REASON failed: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
  }

  BRCMF_DBG(TRACE, "Exit");
  return status;
}

static zx_status_t brcmf_notify_tdls_peer_event(struct brcmf_if* ifp,
                                                const struct brcmf_event_msg* e, void* data) {
  switch (e->reason) {
    case BRCMF_E_REASON_TDLS_PEER_DISCOVERED:
      BRCMF_DBG(TRACE, "TDLS Peer Discovered");
      break;
    case BRCMF_E_REASON_TDLS_PEER_CONNECTED:
      BRCMF_DBG(TRACE, "TDLS Peer Connected");
      brcmf_proto_add_tdls_peer(ifp->drvr, ifp->ifidx, (uint8_t*)e->addr);
      break;
    case BRCMF_E_REASON_TDLS_PEER_DISCONNECTED:
      BRCMF_DBG(TRACE, "TDLS Peer Disconnected");
      brcmf_proto_delete_peer(ifp->drvr, ifp->ifidx, (uint8_t*)e->addr);
      break;
  }
  return ZX_OK;
}

// Country is initialized to US by default. This should be retrieved from location services
// when available.
zx_status_t brcmf_if_start(net_device* ndev, zx_handle_t* out_sme_channel) {
  if (!ndev->sme_channel.is_valid()) {
    return ZX_ERR_ALREADY_BOUND;
  }

  BRCMF_IFDBG(WLANIF, ndev, "Starting wlan_fullmac interface");
  brcmf_netdev_open(ndev);
  ndev->is_up = true;

  ZX_DEBUG_ASSERT(out_sme_channel != nullptr);
  *out_sme_channel = ndev->sme_channel.release();
  return ZX_OK;
}

void brcmf_if_stop(net_device* ndev) {
  BRCMF_IFDBG(WLANIF, ndev, "Stopping wlan_fullmac interface");

  std::lock_guard<std::shared_mutex> guard(ndev->if_proto_lock);
  ndev->is_up = false;
  BRCMF_IFDBG(WLANIF, ndev, "wlan_fullmac interface stopped");
}

void brcmf_if_start_scan(net_device* ndev,
                         const fuchsia_wlan_fullmac_wire::WlanFullmacImplStartScanRequest* req) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping scan request.");
    return;
  }
  zx_status_t result;

  BRCMF_IFDBG(WLANIF, ndev, "Scan request from SME. txn_id: %" PRIu64 ", type: %s", req->txn_id(),
              req->scan_type() == fuchsia_wlan_fullmac_wire::WlanScanType::kPassive  ? "passive"
              : req->scan_type() == fuchsia_wlan_fullmac_wire::WlanScanType::kActive ? "active"
                                                                                     : "invalid");

  ndev->scan_num_results = 0;

  uint16_t sync_id = 0;
  BRCMF_DBG(SCAN, "About to scan! Txn ID %lu", req->txn_id());
  // Lock here so that we ensure that the sync_id is stored in ndev before we start processing
  // results. Otherwise this thread might get suspended and results would come in and be discareded
  // because the sync_id would not match.
  std::lock_guard sync_id_lock(ndev->scan_sync_id_mutex);
  result = brcmf_cfg80211_scan(ndev, req, &sync_id);
  switch (result) {
    case ZX_OK:
      ndev->scan_txn_id = req->txn_id();
      ndev->scan_sync_id = sync_id;
      break;
    case ZX_ERR_SHOULD_WAIT:
      BRCMF_INFO("Scan failed. Firmware busy: %d %s", result, zx_status_get_string(result));
      brcmf_signal_scan_end(ndev, req->txn_id(),
                            fuchsia_wlan_fullmac_wire::WlanScanResult::kShouldWait);
      break;
    case ZX_ERR_INVALID_ARGS:
      BRCMF_ERR("Scan failed. Invalid arguments: %d %s", result, zx_status_get_string(result));
      brcmf_signal_scan_end(ndev, req->txn_id(),
                            fuchsia_wlan_fullmac_wire::WlanScanResult::kInvalidArgs);
      break;
    default:
      BRCMF_INFO("Scan failed. Internal error: %d %s", result, zx_status_get_string(result));
      brcmf_signal_scan_end(ndev, req->txn_id(),
                            fuchsia_wlan_fullmac_wire::WlanScanResult::kInternalError);
  }
}

// Reset internal fields that tracked an in-progress roam attempt. This function does not send out
// a disconnect or a roam result; caller is responsible for sending these, if they are needed.
static void clear_roam_attempt(struct brcmf_if* ifp) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  brcmf_clear_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state);

  ifp->roam_req.reset();
  cfg->target_bss_authenticated = false;

  // If roam failed due to a SME-issued deauth for the target BSS, we have to keep the
  // target BSSID until the deauth handler cleans it up. Otherwise, clear it now.
  if (!brcmf_test_bit(brcmf_disconnect_request_bit_t::DEAUTH_TARGET_BSS,
                      &cfg->disconnect_request_state)) {
    cfg->target_bssid.reset();
  }
}

void brcmf_if_connect_req(net_device* ndev,
                          const fuchsia_wlan_fullmac_wire::WlanFullmacImplConnectRequest* req) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_profile* profile = &ifp->vif->profile;

  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping connect request");
    return;
  }

  if (!(req->has_selected_bss() && req->has_auth_type())) {
    BRCMF_ERR("Missing required field, selected_bss: %d, auth_type: %d", req->has_selected_bss(),
              req->has_auth_type());
    return;
  }

  // Saving the request as FIDL natural type.
  // Note that below this point, `req` and `ifp->connect_req` refer to the same connect request and
  // are equivalent.
  ifp->connect_req = fidl::ToNatural(*req);
  fuchsia_wlan_ieee80211_wire::StatusCode status_code;
  zx_status_t status;

  auto ssid =
      brcmf_find_ssid_in_ies(req->selected_bss().ies.data(), req->selected_bss().ies.size());

  // Saving the request as FIDL natural type.
  // Note that below this point, `req` and `ifp->connect_req` refer to the same connect request and
  // are equivalent.
  ifp->connect_req = fidl::ToNatural(*req);

  if (ssid.empty()) {
    BRCMF_DBG(WLANIF, "Connect request from SME exited: no SSID in request");
    status_code = fuchsia_wlan_ieee80211_wire::StatusCode::kJoinFailure;
    goto fail;
  }

  if (req->has_wep_key()) {
    if (!req->wep_key().has_key()) {
      BRCMF_DBG(WLANIF,
                "Connect request from SME exited: WEP key configuration does not have key data");
      status_code = fuchsia_wlan_ieee80211_wire::StatusCode::kJoinFailure;
      goto fail;
    }
    if (req->wep_key().key().size() > 0 &&
        !(req->auth_type() == fuchsia_wlan_fullmac_wire::WlanAuthType::kSharedKey ||
          req->auth_type() == fuchsia_wlan_fullmac_wire::WlanAuthType::kOpenSystem)) {
      BRCMF_DBG(WLANIF, "Connect request from SME exited: unexpected WEP key in request");
      status_code = fuchsia_wlan_ieee80211_wire::StatusCode::kJoinFailure;
      goto fail;
    }

    if (req->wep_key().key().size() > MAX_SUPPORTED_WEP_KEY_LEN) {
      BRCMF_DBG(WLANIF, "Connect request from SME exited: WEP key len %zu larger than %d",
                req->wep_key().key().size(), MAX_SUPPORTED_WEP_KEY_LEN);
      status_code = fuchsia_wlan_ieee80211_wire::StatusCode::kJoinFailure;
      goto fail;
    }
  }

  BRCMF_IFDBG(WLANIF, ndev, "Connect request from SME.");
#if !defined(NDEBUG)
  BRCMF_IFDBG(WLANIF, ndev, "  ssid: " FMT_SSID ", bssid: " FMT_MAC ", channel: %u",
              FMT_SSID_VECT(ssid), FMT_MAC_ARGS(req->selected_bss().bssid),
              req->selected_bss().primary.number);
#endif /* !defined(NDEBUG) */

  // Once a connection attempt is in progress, no roam is possible. Erase anything about any
  // previous roam, just in case.
  clear_roam_attempt(ifp);

  memcpy(profile->bssid, req->selected_bss().bssid.data(), ETH_ALEN);

  status = brcmf_configure_opensecurity(ifp);
  if (status != ZX_OK) {
    BRCMF_DBG(WLANIF, "Connect request from SME exited: unable to reset security iovars");
    status_code = fuchsia_wlan_ieee80211_wire::StatusCode::kJoinFailure;
    goto fail;
  }

  if (req->has_wep_key_desc() && req->wep_key_desc().has_key() &&
      req->wep_key_desc().key().size() > 0) {
    auto add_key_result = brcmf_cfg80211_add_key(ndev, &req->wep_key_desc());
    if (add_key_result != ZX_OK) {
      BRCMF_DBG(WLANIF, "Connect request from SME exited: unable to set WEP key");
      status_code = fuchsia_wlan_ieee80211_wire::StatusCode::kJoinFailure;
      goto fail;
    }
  }

  if (brcmf_set_auth_type(ndev, ifp->connect_req.auth_type().value()) != ZX_OK) {
    BRCMF_IFDBG(WLANIF, ndev, "Connect request from SME exited: bad auth_type parameters");
    status_code = fuchsia_wlan_ieee80211_wire::StatusCode::kUnsupportedAuthAlgorithm;
    goto fail;
  }

  if (ifp->connect_req.security_ie()->size() > 0) {
    BRCMF_DBG(TEMP, " * * Security IE non-zero! %ld", ifp->connect_req.security_ie()->size());
    BRCMF_DBG_HEX_DUMP(BRCMF_IS_ON(BYTES), ifp->connect_req.security_ie()->data(),
                       ifp->connect_req.security_ie()->size(), "Security IE:");
  }

  brcmf_cfg80211_connect(ndev, &ifp->connect_req);
  BRCMF_IFDBG(WLANIF, ndev, "Initiated connect request from SME");
  return;

fail:
  brcmf_return_assoc_result(ndev, status_code);
}

void brcmf_if_reconnect_req(net_device* ndev,
                            const fuchsia_wlan_fullmac_wire::WlanFullmacImplReconnectRequest* req) {
  struct brcmf_if* ifp = ndev_to_if(ndev);

  if (!req->has_peer_sta_address()) {
    BRCMF_ERR("Reconnect req does not contain peer addr");
    return;
  }

  if (memcmp(req->peer_sta_address().data(), ifp->connect_req.selected_bss()->bssid().data(),
             ETH_ALEN)) {
    BRCMF_ERR("Requested MAC != Connected MAC");
#if !defined(NDEBUG)
    const uint8_t* old_mac = ifp->connect_req.selected_bss()->bssid().data();
    const uint8_t* new_mac = req->peer_sta_address().data();
    BRCMF_IFDBG(WLANIF, ndev, " requested mac: " FMT_MAC ", connected mac: " FMT_MAC,
                FMT_MAC_ARGS(new_mac), FMT_MAC_ARGS(old_mac));
#endif /* !defined(NDEBUG) */

    brcmf_return_assoc_result(ndev,
                              fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified);
    return;
  }

  brcmf_cfg80211_connect(ndev, &ifp->connect_req);
}

// In AP mode, receive a response from wlan_fullmac confirming that a client was successfully
// authenticated.
void brcmf_if_auth_resp(net_device* ndev,
                        const fuchsia_wlan_fullmac_wire::WlanFullmacImplAuthRespRequest* ind) {
  struct brcmf_if* ifp = ndev_to_if(ndev);

  if (!brcmf_is_apmode(ifp->vif)) {
    BRCMF_ERR("Received AUTHENTICATE.response but not in AP mode - ignoring");
    return;
  }

  if (!ind->has_result_code() || !ind->has_peer_sta_address()) {
    BRCMF_ERR("Missing fields in auth resp has_result_code: %d has_peer_sta_addr: %d",
              ind->has_result_code(), ind->has_peer_sta_address());
    return;
  }

  BRCMF_IFDBG(
      WLANIF, ndev, "Auth response from SME. result: %s",
      ind->result_code() == fuchsia_wlan_fullmac_wire::WlanAuthResult::kSuccess   ? "success"
      : ind->result_code() == fuchsia_wlan_fullmac_wire::WlanAuthResult::kRefused ? "refused"
      : ind->result_code() == fuchsia_wlan_fullmac_wire::WlanAuthResult::kAntiCloggingTokenRequired
          ? "anti-clogging token required"
      : ind->result_code() ==
              fuchsia_wlan_fullmac_wire::WlanAuthResult::kFiniteCyclicGroupNotSupported
          ? "finite cyclic group not supported"
      : ind->result_code() == fuchsia_wlan_fullmac_wire::WlanAuthResult::kRejected ? "rejected"
      : ind->result_code() == fuchsia_wlan_fullmac_wire::WlanAuthResult::kFailureTimeout
          ? "timeout"
          : "invalid");
#if !defined(NDEBUG)
  BRCMF_IFDBG(WLANIF, ndev, "  , address: " FMT_MAC, FMT_MAC_ARGS(ind->peer_sta_address().data()));
#endif /* !defined(NDEBUG) */

  if (ind->result_code() == fuchsia_wlan_fullmac_wire::WlanAuthResult::kSuccess) {
    const uint8_t* mac = ind->peer_sta_address().data();
    BRCMF_DBG(CONN, "Successfully authenticated client " FMT_MAC "\n", FMT_MAC_ARGS(mac));
    return;
  }

  fuchsia_wlan_ieee80211::ReasonCode reason = {};
  switch (ind->result_code()) {
    case fuchsia_wlan_fullmac_wire::WlanAuthResult::kRefused:
    case fuchsia_wlan_fullmac_wire::WlanAuthResult::kRejected:
      reason = fuchsia_wlan_ieee80211::ReasonCode::kNotAuthenticated;
      break;
    case fuchsia_wlan_fullmac_wire::WlanAuthResult::kFailureTimeout:
      reason = fuchsia_wlan_ieee80211::ReasonCode::kTimeout;
      break;
    case fuchsia_wlan_fullmac_wire::WlanAuthResult::kAntiCloggingTokenRequired:
    case fuchsia_wlan_fullmac_wire::WlanAuthResult::kFiniteCyclicGroupNotSupported:
    default:
      reason = fuchsia_wlan_ieee80211::ReasonCode::kUnspecifiedReason;
      break;
  }
  brcmf_cfg80211_del_station(ndev, ind->peer_sta_address().data(), reason);
}

// Respond to a MLME-DEAUTHENTICATE.request message. Note that we are required to respond with a
// MLME-DEAUTHENTICATE.confirm on completion (or failure), even though there is no status
// reported.
void brcmf_if_deauth_req(net_device* ndev,
                         const fuchsia_wlan_fullmac_wire::WlanFullmacImplDeauthRequest* req) {
  struct brcmf_if* ifp = ndev_to_if(ndev);

  if (!req->has_peer_sta_address() || !req->has_reason_code()) {
    BRCMF_ERR("Deauth Req does not contain all the required fields peer addr: %d reason code: %d",
              req->has_peer_sta_address(), req->has_reason_code());
    return;
  }
  const auto& peer_sta_address = req->peer_sta_address().data();
  BRCMF_IFDBG(WLANIF, ndev, "Deauth request from SME. reason: %" PRIu16 "",
              fidl::ToUnderlying(req->reason_code()));

  if (brcmf_is_apmode(ifp->vif)) {
    struct brcmf_scb_val_le scbval;
    bcme_status_t fw_err = BCME_OK;

    memcpy(&scbval.ea, peer_sta_address, ETH_ALEN);
    // The FIDL reason code is defined in uint16_t, so no information will be lost.
    scbval.val = fidl::ToUnderlying(req->reason_code());
    zx_status_t status = brcmf_fil_cmd_data_set(ifp, BRCMF_C_SCB_DEAUTHENTICATE_FOR_REASON, &scbval,
                                                sizeof(scbval), &fw_err);
    if (status != ZX_OK) {
      BRCMF_ERR("Failed to disassociate: %s, fw err %s", zx_status_get_string(status),
                brcmf_fil_get_errstr(fw_err));
    }
    // Deauth confirm will get sent when the driver receives the DEAUTH_EVENT
    return;
  }

  // Client IF processing
  if (brcmf_cfg80211_disconnect(ndev, peer_sta_address, fidl::ToUnderlying(req->reason_code()),
                                true) != ZX_OK) {
    // Request to disconnect failed, so respond immediately
    brcmf_notify_deauth(ndev, peer_sta_address);
  }  // else wait for disconnect to complete before sending response

  // Workaround for https://fxbug.dev/42103512: allow time for disconnect to complete
  zx_nanosleep(zx_deadline_after(ZX_MSEC(50)));
}

void brcmf_if_assoc_resp(net_device* ndev,
                         const fuchsia_wlan_fullmac_wire::WlanFullmacImplAssocRespRequest* req) {
  struct brcmf_if* ifp = ndev_to_if(ndev);

  if (!req->has_result_code() || !req->has_association_id() || !req->has_peer_sta_address()) {
    BRCMF_ERR("Assoc resp does not contain all fields reason: %d assoc id: %d id: peer addr: %d",
              req->has_result_code(), req->has_association_id(), req->has_peer_sta_address());
    return;
  }
  BRCMF_IFDBG(WLANIF, ndev, "Assoc response from SME. result: %" PRIu8 ", aid: %" PRIu16,
              fidl::ToUnderlying(req->result_code()), req->association_id());
#if !defined(NDEBUG)
  BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC, FMT_MAC_ARGS(req->peer_sta_address().data()));
#endif /* !defined(NDEBUG) */

  if (!brcmf_is_apmode(ifp->vif)) {
    BRCMF_ERR("Received ASSOCIATE.response but not in AP mode - ignoring");
    return;
  }

  if (req->result_code() == fuchsia_wlan_fullmac_wire::WlanAssocResult::kSuccess) {
    const uint8_t* mac = req->peer_sta_address().data();
    BRCMF_DBG(CONN, "Successfully associated client " FMT_MAC, FMT_MAC_ARGS(mac));
    return;
  }

  // TODO(https://fxbug.dev/42140476): The translation here is poor because the set of result codes
  // available for an association response is too small.
  fuchsia_wlan_ieee80211::ReasonCode reason = {};
  switch (req->result_code()) {
    case fuchsia_wlan_fullmac_wire::WlanAssocResult::kRefusedNotAuthenticated:
      reason = fuchsia_wlan_ieee80211::ReasonCode::kNotAuthenticated;
      break;
    case fuchsia_wlan_fullmac_wire::WlanAssocResult::kRefusedCapabilitiesMismatch:
      reason = fuchsia_wlan_ieee80211::ReasonCode::kInvalidRsneCapabilities;
      break;
    case fuchsia_wlan_fullmac_wire::WlanAssocResult::kRefusedReasonUnspecified:
    case fuchsia_wlan_fullmac_wire::WlanAssocResult::kRefusedExternalReason:
    case fuchsia_wlan_fullmac_wire::WlanAssocResult::kRefusedApOutOfMemory:
    case fuchsia_wlan_fullmac_wire::WlanAssocResult::kRefusedBasicRatesMismatch:
    case fuchsia_wlan_fullmac_wire::WlanAssocResult::kRejectedEmergencyServicesNotSupported:
    case fuchsia_wlan_fullmac_wire::WlanAssocResult::kRefusedTemporarily:
    default:
      reason = fuchsia_wlan_ieee80211::ReasonCode::kUnspecifiedReason;
      break;
  }
  // The copy removed, why we want to copy before passing it into the next function?
  brcmf_cfg80211_del_station(ndev, req->peer_sta_address().data(), reason);
}

void brcmf_if_disassoc_req(net_device* ndev,
                           const fuchsia_wlan_fullmac_wire::WlanFullmacImplDisassocRequest* req) {
  if (!req->has_reason_code() || !req->has_peer_sta_address()) {
    BRCMF_ERR("Disassoc req does not contain all fields reason: %d sta address: %d",
              req->has_reason_code(), req->has_peer_sta_address());
    return;
  }
  BRCMF_IFDBG(WLANIF, ndev, "Disassoc request from SME. reason: %" PRIu16,
              fidl::ToUnderlying(req->reason_code()));
#if !defined(NDEBUG)
  BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC, FMT_MAC_ARGS(req->peer_sta_address().data()));
#endif /* !defined(NDEBUG) */
  zx_status_t status = brcmf_cfg80211_disconnect(ndev, req->peer_sta_address().data(),
                                                 fidl::ToUnderlying(req->reason_code()), false);
  if (status != ZX_OK) {
    brcmf_notify_disassoc(ndev, status);
  }  // else notification will happen asynchronously
}

static void brcmf_if_start_conf(net_device* ndev, fuchsia_wlan_fullmac_wire::StartResult result) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping AP start callback");
    return;
  }

  BRCMF_IFDBG(WLANIF, ndev, "Sending AP start confirm to SME. result_code: %s",
              result == fuchsia_wlan_fullmac_wire::StartResult::kSuccess ? "success"
              : result == fuchsia_wlan_fullmac_wire::StartResult::kBssAlreadyStartedOrJoined
                  ? "already started"
              : result == fuchsia_wlan_fullmac_wire::StartResult::kResetRequiredBeforeStart
                  ? "reset required"
              : result == fuchsia_wlan_fullmac_wire::StartResult::kNotSupported ? "not supported"
                                                                                : "unknown");

  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }

  auto start_conf = fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcStartConfRequest::Builder(*arena)
                        .result_code(result)
                        .Build();

  auto status = ndev->if_proto.buffer(*arena)->StartConf(start_conf);
  if (!status.ok()) {
    BRCMF_ERR("Failed to send start conf result.status: %s", status.status_string());
  }
}

// AP start timeout worker
static void brcmf_ap_start_timeout_worker(WorkItem* work) {
  struct brcmf_cfg80211_info* cfg =
      containerof(work, struct brcmf_cfg80211_info, ap_start_timeout_work);
  struct net_device* ndev = cfg_to_softap_ndev(cfg);
  struct brcmf_if* ifp = ndev_to_if(ndev);

  // Indicate status only if AP start pending is set
  if (brcmf_test_and_clear_bit(brcmf_vif_status_bit_t::AP_START_PENDING, &ifp->vif->sme_state)) {
    // Indicate AP start failed
    brcmf_if_start_conf(ndev, fuchsia_wlan_fullmac_wire::StartResult::kNotSupported);
  }
}

// AP start timeout handler
static void brcmf_ap_start_timeout(struct brcmf_cfg80211_info* cfg) {
  cfg->pub->irq_callback_lock.lock();
  BRCMF_DBG(TRACE, "Enter");
  EXEC_TIMEOUT_WORKER(ap_start_timeout_work);
  cfg->pub->irq_callback_lock.unlock();
}

/* Start AP mode */
void brcmf_if_start_req(net_device* ndev,
                        const fuchsia_wlan_fullmac_wire::WlanFullmacImplStartBssRequest* req) {
  if (!req->has_ssid() || !req->has_dtim_period() || !req->has_primary() || !req->has_bss_type() ||
      !req->has_beacon_period()) {
    BRCMF_ERR(
        "Start BSS req does not have all required fields ssid: %d "
        "dtim: %d channel: %d bss type: %d beacon period: %d",
        req->has_ssid(), req->has_dtim_period(), req->has_primary(), req->has_bss_type(),
        req->has_beacon_period());
    brcmf_if_start_conf(ndev, fuchsia_wlan_fullmac_wire::StartResult::kNotSupported);
    return;
  }
  BRCMF_IFDBG(WLANIF, ndev, "Start AP request from SME. rsne_len: %zu, channel: %u",
              req->has_rsne() ? req->rsne().size() : 0, req->primary().number);
#if !defined(NDEBUG)
  BRCMF_DBG(WLANIF, "  ssid: " FMT_SSID, FMT_SSID_BYTES(req->ssid().data(), req->ssid().size()));
#endif /* !defined(NDEBUG) */

  fuchsia_wlan_fullmac_wire::StartResult result_code = brcmf_cfg80211_start_ap(ndev, req);
  if (result_code != fuchsia_wlan_fullmac_wire::StartResult::kSuccess) {
    brcmf_if_start_conf(ndev, result_code);
  }
}

/* Stop AP mode */
void brcmf_if_stop_req(net_device* ndev,
                       const fuchsia_wlan_fullmac_wire::WlanFullmacImplStopBssRequest* req) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  struct brcmf_if* ifp = ndev_to_if(ndev);
  fuchsia_wlan_fullmac_wire::StopResult result_code;

  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping AP stop callback");
    return;
  }

  BRCMF_IFDBG(WLANIF, ndev, "Stop AP request from SME.");
  if (!req->has_ssid()) {
    BRCMF_ERR("Stop req does not contain ssid");
    result_code = fuchsia_wlan_fullmac_wire::StopResult::kInternalError;
    goto done;
  }
#if !defined(NDEBUG)
  BRCMF_DBG(WLANIF, "  ssid: " FMT_SSID, FMT_SSID_BYTES(req->ssid().data(), req->ssid().size()));
#endif /* !defined(NDEBUG) */
  if ((req->ssid().size() != ifp->saved_softap_ssid.size()) ||
      (memcmp(req->ssid().data(), ifp->saved_softap_ssid.data(), req->ssid().size()) != 0)) {
    BRCMF_ERR("SSID does not match running SoftAP, req SSID: " FMT_SSID " current SSID: " FMT_SSID,
              FMT_SSID_BYTES(req->ssid().data(), req->ssid().size()),
              FMT_SSID_BYTES(ifp->saved_softap_ssid.data(), ifp->saved_softap_ssid.size()));
    result_code = fuchsia_wlan_fullmac_wire::StopResult::kInternalError;
    goto done;
  }

  result_code = brcmf_cfg80211_stop_ap(ndev);
done:
  BRCMF_IFDBG(
      WLANIF, ndev, "Sending AP stop confirm to SME. result_code: %s",
      result_code == fuchsia_wlan_fullmac_wire::StopResult::kSuccess             ? "success"
      : result_code == fuchsia_wlan_fullmac_wire::StopResult::kBssAlreadyStopped ? "already stopped"
      : result_code == fuchsia_wlan_fullmac_wire::StopResult::kInternalError     ? "internal error"
                                                                                 : "unknown");

  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }

  auto result = fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcStopConfRequest::Builder(*arena)
                    .result_code(result_code)
                    .Build();

  auto proto_status = ndev->if_proto.buffer(*arena)->StopConf(result);
  if (!proto_status.ok()) {
    BRCMF_ERR("Failed to send stop conf result.status: %s", proto_status.status_string());
  }
}

std::vector<zx_status_t> brcmf_if_set_keys_req(
    net_device* ndev, const fuchsia_wlan_fullmac_wire::WlanFullmacImplSetKeysRequest* req) {
  BRCMF_IFDBG(WLANIF, ndev, "Set keys request from SME. num_keys: %zu",
              req->key_descriptors().size());
  zx_status_t result;

  std::vector<zx_status_t> statuslist;
  statuslist.reserve(req->key_descriptors().size());
  for (size_t i = 0; i < req->key_descriptors().size(); i++) {
    result = brcmf_cfg80211_add_key(ndev, &req->key_descriptors().data()[i]);
    if (result != ZX_OK) {
      BRCMF_WARN("Error setting key %zu: %s.", i, zx_status_get_string(result));
    }
    statuslist.emplace_back(result);
  }
  return statuslist;
}

static void brcmf_send_eapol_confirm(
    net_device* ndev, const fuchsia_wlan_fullmac_wire::WlanFullmacImplEapolTxRequest* req,
    zx_status_t result) {
  const auto eapol_result = result == ZX_OK
                                ? fuchsia_wlan_fullmac_wire::EapolTxResult::kSuccess
                                : fuchsia_wlan_fullmac_wire::EapolTxResult::kTransmissionFailure;

  BRCMF_IFDBG(WLANIF, ndev, "Sending EAPOL xmit confirm to SME. result: %s, code: %u",
              eapol_result == fuchsia_wlan_fullmac_wire::EapolTxResult::kSuccess ? "success"
              : eapol_result == fuchsia_wlan_fullmac_wire::EapolTxResult::kTransmissionFailure
                  ? "failure"
                  : "unknown",
              static_cast<uint8_t>(eapol_result));
  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }

  fidl::Array<uint8_t, ETH_ALEN> dst_addr;
  memcpy(dst_addr.data(), req->dst_addr().data(), ETH_ALEN);

  const auto confirm =
      fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcEapolConfRequest::Builder(*arena)
          .result_code(eapol_result)
          .dst_addr(dst_addr)
          .Build();

  auto proto_status = ndev->if_proto.buffer(*arena)->EapolConf(confirm);
  if (!proto_status.ok()) {
    BRCMF_ERR("Failed to send eapol confirm result.status: %s", proto_status.status_string());
  }
}

static void brcmf_populate_eapol_eth_header(
    uint8_t* dest, const fuchsia_wlan_fullmac_wire::WlanFullmacImplEapolTxRequest* req) {
  // IEEE Std. 802.3-2015, 3.1.1
  memcpy(dest, req->dst_addr().data(), ETH_ALEN);
  memcpy(dest + ETH_ALEN, req->src_addr().data(), ETH_ALEN);
  *reinterpret_cast<uint16_t*>(dest + 2 * ETH_ALEN) = EAPOL_ETHERNET_TYPE_UINT16;
  memcpy(dest + 2 * ETH_ALEN + sizeof(uint16_t), req->data().data(), req->data().size());
}

static void brcmf_if_eapol_req_netdev(
    net_device* ndev, const fuchsia_wlan_fullmac_wire::WlanFullmacImplEapolTxRequest* req,
    int length) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_pub* drvr = ifp->drvr;
  wlan::drivers::components::FrameContainer frames = brcmf_bus_acquire_tx_space(drvr->bus_if, 1);
  if (frames.empty()) {
    BRCMF_ERR("Failed to allocate space for EAPOL transmittion");
    return;
  }

  wlan::drivers::components::Frame& frame = *frames.begin();
  frame.ShrinkHead(drvr->hdrlen);
  frame.SetPortId(ifp->ifidx);
  frame.SetPriority(0);
  frame.SetSize(length);

  brcmf_populate_eapol_eth_header(frame.Data(), req);

  cpp20::span<wlan::drivers::components::Frame> frame_span(frames);
  zx_status_t result = brcmf_start_xmit(drvr, frame_span);

  brcmf_send_eapol_confirm(ndev, req, result);
}

void brcmf_if_eapol_req(net_device* ndev,
                        const fuchsia_wlan_fullmac_wire::WlanFullmacImplEapolTxRequest* req) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping EAPOL xmit callback");
    return;
  }

  BRCMF_IFDBG(WLANIF, ndev, "EAPOL xmit request from SME. data_len: %zu", req->data().size());

  int packet_length;

  // Ethernet header length + EAPOL PDU length
  packet_length = 2 * ETH_ALEN + sizeof(uint16_t) + req->data().size();

  brcmf_if_eapol_req_netdev(ndev, req, packet_length);
}

static void brcmf_get_bwcap(struct brcmf_if* ifp, uint32_t bw_cap[]) {
  // 2.4 GHz
  uint32_t val = WLC_BAND_2G;
  zx_status_t status = brcmf_fil_iovar_int_get(ifp, "bw_cap", &val, nullptr);
  if (status == ZX_OK) {
    bw_cap[static_cast<uint8_t>(fuchsia_wlan_ieee80211_wire::WlanBand::kTwoGhz)] = val;

    // 5 GHz
    val = WLC_BAND_5G;
    status = brcmf_fil_iovar_int_get(ifp, "bw_cap", &val, nullptr);
    if (status == ZX_OK) {
      bw_cap[static_cast<uint8_t>(fuchsia_wlan_ieee80211_wire::WlanBand::kFiveGhz)] = val;
      return;
    }
    BRCMF_WARN(
        "Failed to retrieve 5GHz bandwidth info, but sucessfully retrieved bandwidth "
        "info for 2.4GHz bands.");
    return;
  }

  // bw_cap not supported in this version of fw
  uint32_t mimo_bwcap = 0;
  status = brcmf_fil_iovar_int_get(ifp, "mimo_bw_cap", &mimo_bwcap, nullptr);
  if (status != ZX_OK) {
    /* assume 20MHz if firmware does not give a clue */
    BRCMF_WARN("Failed to retrieve bandwidth capability info. Assuming 20MHz for all.");
    mimo_bwcap = WLC_N_BW_20ALL;
  }

  switch (mimo_bwcap) {
    case WLC_N_BW_40ALL:
      bw_cap[static_cast<uint8_t>(fuchsia_wlan_ieee80211_wire::WlanBand::kTwoGhz)] |=
          WLC_BW_40MHZ_BIT;
      __FALLTHROUGH;
    case WLC_N_BW_20IN2G_40IN5G:
      bw_cap[static_cast<uint8_t>(fuchsia_wlan_ieee80211_wire::WlanBand::kFiveGhz)] |=
          WLC_BW_40MHZ_BIT;
      __FALLTHROUGH;
    case WLC_N_BW_20ALL:
      bw_cap[static_cast<uint8_t>(fuchsia_wlan_ieee80211_wire::WlanBand::kTwoGhz)] |=
          WLC_BW_20MHZ_BIT;
      bw_cap[static_cast<uint8_t>(fuchsia_wlan_ieee80211_wire::WlanBand::kFiveGhz)] |=
          WLC_BW_20MHZ_BIT;
      break;
    default:
      BRCMF_ERR("invalid mimo_bw_cap value");
  }
}

static uint16_t brcmf_get_mcs_map(uint32_t nchain, uint16_t supp) {
  uint16_t mcs_map = 0xffff;
  for (uint32_t i = 0; i < nchain; i++) {
    mcs_map = (mcs_map << 2) | supp;
  }

  return mcs_map;
}

// Updates |fidl_ht_caps| in place with capabilities determined by information from firmware.
static void brcmf_update_ht_cap(struct brcmf_if* ifp,
                                fuchsia_wlan_ieee80211::HtCapabilities& fidl_ht_caps,
                                fuchsia_wlan_ieee80211::WlanBand band, uint32_t bw_cap[2],
                                uint32_t ldpc_cap, uint32_t nchain, uint32_t max_ampdu_len_exp) {
  wlan::HtCapabilities* ht_caps =
      wlan::HtCapabilities::ViewFromRawBytes(fidl_ht_caps.bytes().data());

  // LDPC Support
  if (ldpc_cap) {
    ht_caps->ht_cap_info.set_ldpc_coding_cap(true);
  }

  // Bandwidth-related flags
  if (bw_cap[static_cast<uint8_t>(band)] & WLC_BW_40MHZ_BIT) {
    ht_caps->ht_cap_info.set_chan_width_set(true);
    ht_caps->ht_cap_info.set_short_gi_40(true);
  }
  ht_caps->ht_cap_info.set_short_gi_20(true);
  ht_caps->ht_cap_info.set_dsss_in_40(true);

  // SM Power Save
  // At present SMPS appears to never be enabled in firmware (see https://fxbug.dev/42104422)
  ht_caps->ht_cap_info.set_sm_power_save(IEEE80211_HT_CAPS_SMPS_DISABLED);

  // Rx STBC
  uint32_t rx_stbc = 0;
  (void)brcmf_fil_iovar_int_get(ifp, "stbc_rx", &rx_stbc, nullptr);
  ht_caps->ht_cap_info.set_rx_stbc(rx_stbc & 0x3);

  // Tx STBC
  // According to Broadcom, Tx STBC capability should be induced from the value of the
  // "stbc_rx" iovar and not "stbc_tx".
  if (rx_stbc != 0) {
    ht_caps->ht_cap_info.set_tx_stbc(true);
  }

  // AMPDU Parameters
  uint32_t ampdu_rx_density = 0;
  zx_status_t status = brcmf_fil_iovar_int_get(ifp, "ampdu_rx_density", &ampdu_rx_density, nullptr);
  if (status != ZX_OK) {
    BRCMF_ERR("Failed to retrieve value for AMPDU Rx density from firmware, using 16 us");
    ampdu_rx_density = 7;
  }
  ht_caps->ampdu_params.set_min_start_spacing(ampdu_rx_density & 0x7);
  if (max_ampdu_len_exp > 3) {
    // Cap A-MPDU length at 64K
    max_ampdu_len_exp = 3;
  }
  ht_caps->ampdu_params.set_exponent(max_ampdu_len_exp);

  // Supported MCS Set
  size_t mcs_set_size = sizeof(ht_caps->mcs_set);
  if (nchain > mcs_set_size) {
    BRCMF_ERR("Supported MCS set too small for nchain (%u), truncating", nchain);
    nchain = mcs_set_size;
  }
  memset(&ht_caps->mcs_set, 0xff, nchain);
}

// Updates |fidl_vht_caps| in place with capabilities determined by information from firmware.
static void brcmf_update_vht_cap(struct brcmf_if* ifp,
                                 fuchsia_wlan_ieee80211::VhtCapabilities& fidl_vht_caps,
                                 fuchsia_wlan_ieee80211::WlanBand band, uint32_t bw_cap[2],
                                 uint32_t nchain, uint32_t ldpc_cap, uint32_t max_ampdu_len_exp) {
  uint16_t mcs_map;
  wlan::VhtCapabilities* vht_caps =
      wlan::VhtCapabilities::ViewFromRawBytes(fidl_vht_caps.bytes().data());

  // Set Max MPDU length to 11454
  // TODO(https://fxbug.dev/42103822): Value hardcoded from firmware behavior of the BCM4356 and
  // BCM4359 chips.
  vht_caps->vht_cap_info.set_max_mpdu_len(2);

  /* 80MHz is mandatory */
  vht_caps->vht_cap_info.set_sgi_cbw80(true);
  if (bw_cap[static_cast<uint8_t>(band)] & WLC_BW_160MHZ_BIT) {
    vht_caps->vht_cap_info.set_supported_cbw_set(1);
    vht_caps->vht_cap_info.set_sgi_cbw160(true);
  }

  if (ldpc_cap) {
    vht_caps->vht_cap_info.set_rx_ldpc(true);
  }

  // Tx STBC
  // TODO(https://fxbug.dev/42103822): Value is hardcoded for now
  if (brcmf_feat_is_quirk_enabled(ifp, BRCMF_FEAT_QUIRK_IS_4359)) {
    vht_caps->vht_cap_info.set_tx_stbc(true);
  }

  /* all support 256-QAM */
  mcs_map = brcmf_get_mcs_map(nchain, IEEE80211_VHT_MCS_0_9);
  /* Rx MCS map (B0:15) + Tx MCS map (B0:15) */
  vht_caps->vht_mcs_nss.set_as_uint64((uint64_t)mcs_map | ((uint64_t)mcs_map << 32));

  /* Beamforming support information */
  uint32_t txbf_bfe_cap = 0;
  uint32_t txbf_bfr_cap = 0;

  // Use the *_cap_hw value when possible, since the reflects the capabilities of the device
  // regardless of current operating mode.
  zx_status_t status;
  status = brcmf_fil_iovar_int_get(ifp, "txbf_bfe_cap_hw", &txbf_bfe_cap, nullptr);
  if (status != ZX_OK) {
    (void)brcmf_fil_iovar_int_get(ifp, "txbf_bfe_cap", &txbf_bfe_cap, nullptr);
  }
  status = brcmf_fil_iovar_int_get(ifp, "txbf_bfr_cap_hw", &txbf_bfr_cap, nullptr);
  if (status != ZX_OK) {
    BRCMF_DBG(FIL, "Failed to get iovar txbf_bfr_cap_hw. Falling back to txbf_bfr_cap.");
    (void)brcmf_fil_iovar_int_get(ifp, "txbf_bfr_cap", &txbf_bfr_cap, nullptr);
  }

  if (txbf_bfe_cap & BRCMF_TXBF_SU_BFE_CAP) {
    vht_caps->vht_cap_info.set_su_bfee(true);
  }
  if (txbf_bfe_cap & BRCMF_TXBF_MU_BFE_CAP) {
    vht_caps->vht_cap_info.set_mu_bfee(true);
  }
  if (txbf_bfr_cap & BRCMF_TXBF_SU_BFR_CAP) {
    vht_caps->vht_cap_info.set_su_bfer(true);
  }
  if (txbf_bfr_cap & BRCMF_TXBF_MU_BFR_CAP) {
    vht_caps->vht_cap_info.set_mu_bfer(true);
  }

  uint32_t txstreams = 0;
  // txstreams_cap is not supported in all firmware versions, but when it is supported it
  // provides capability info regardless of current operating state.
  status = brcmf_fil_iovar_int_get(ifp, "txstreams_cap", &txstreams, nullptr);
  if (status != ZX_OK) {
    (void)brcmf_fil_iovar_int_get(ifp, "txstreams", &txstreams, nullptr);
  }

  if ((txbf_bfe_cap || txbf_bfr_cap) && (txstreams > 1)) {
    vht_caps->vht_cap_info.set_bfee_sts(2);
    vht_caps->vht_cap_info.set_num_sounding(txstreams - 1);
    // Link adapt = Both
    vht_caps->vht_cap_info.set_link_adapt(3);
  }

  // Maximum A-MPDU Length Exponent
  vht_caps->vht_cap_info.set_max_ampdu_exp(max_ampdu_len_exp);
}

static void brcmf_dump_80211_ht_caps(fuchsia_wlan_ieee80211::HtCapabilities* caps) {
  // wlan::HtCapabilities
  wlan::HtCapabilities* ht_caps = wlan::HtCapabilities::ViewFromRawBytes(caps->bytes().data());
  BRCMF_DBG_UNFILTERED("     ht_cap_info: %#x", ht_caps->ht_cap_info.as_uint16());
  BRCMF_DBG_UNFILTERED("     ampdu_params: %#x", ht_caps->ampdu_params.val());

  char mcs_set_str[std::size(ht_caps->mcs_set.val()) * 5 + 1];
  char* str = mcs_set_str;
  for (unsigned i = 0; i < std::size(ht_caps->mcs_set.val()); i++) {
    str += sprintf(str, "%s0x%02hhx", i > 0 ? " " : "", ht_caps->mcs_set.val()[i]);
  }

  BRCMF_DBG_UNFILTERED("     mcs_set: %s", mcs_set_str);
  BRCMF_DBG_UNFILTERED("     ht_ext_cap: %#x", ht_caps->ht_ext_cap.as_uint16());
  BRCMF_DBG_UNFILTERED("     asel_cap: %#x", ht_caps->asel_cap.val());
}

static void brcmf_dump_80211_vht_caps(fuchsia_wlan_ieee80211::VhtCapabilities* caps) {
  wlan::VhtCapabilities* vht_caps = wlan::VhtCapabilities::ViewFromRawBytes(caps->bytes().data());
  BRCMF_DBG_UNFILTERED("     vht_cap_info: %#lx", vht_caps->vht_cap_info.as_uint32());
  BRCMF_DBG_UNFILTERED("     vht_mcs_nss: %#" PRIx64 "", vht_caps->vht_mcs_nss.as_uint64());
}

static void brcmf_dump_if_band_cap(fuchsia_wlan_fullmac::BandCapability* band_cap) {
  char band_str[32];
  switch (band_cap->band().value()) {
    case fuchsia_wlan_ieee80211::WlanBand::kTwoGhz:
      sprintf(band_str, "2GHz");
      break;
    case fuchsia_wlan_ieee80211::WlanBand::kFiveGhz:
      sprintf(band_str, "5GHz");
      break;
    default:
      sprintf(band_str, "unknown (%hhu)", static_cast<uint8_t>(band_cap->band().value()));
      break;
  }
  BRCMF_DBG_UNFILTERED("   band: %s", band_str);

  char rates_str[fuchsia::wlan::ieee80211::MAX_SUPPORTED_BASIC_RATES * 6 + 1];
  char* str = rates_str;
  for (unsigned i = 0; i < band_cap->basic_rates()->size(); i++) {
    str += sprintf(str, "%s%d", i > 0 ? " " : "", band_cap->basic_rates()->at(i));
  }
  BRCMF_DBG_UNFILTERED("     basic_rates: %s", rates_str);

  size_t num_primary_channels = band_cap->primary_channels()->size();
  if (num_primary_channels > fuchsia::wlan::ieee80211::MAX_UNIQUE_CHANNEL_NUMBERS) {
    BRCMF_DBG_UNFILTERED("Number of channels reported (%zu) exceeds limit (%u), truncating",
                         band_cap->primary_channels()->size(),
                         fuchsia::wlan::ieee80211::MAX_UNIQUE_CHANNEL_NUMBERS);
    num_primary_channels = fuchsia::wlan::ieee80211::MAX_UNIQUE_CHANNEL_NUMBERS;
  }
  char channels_str[fuchsia::wlan::ieee80211::MAX_UNIQUE_CHANNEL_NUMBERS * 4 + 1];
  str = channels_str;
  for (unsigned i = 0; i < num_primary_channels; i++) {
    str += sprintf(str, "%s%d", i > 0 ? " " : "",
                   static_cast<int>(band_cap->primary_channels()->at(i).number()));
  }
  BRCMF_DBG_UNFILTERED("     channels: %s", channels_str);

  BRCMF_DBG_UNFILTERED("     ht_supported: %s", band_cap->ht_caps().has_value() ? "true" : "false");
  if (band_cap->ht_caps().has_value()) {
    brcmf_dump_80211_ht_caps(&band_cap->ht_caps().value());
  }

  BRCMF_DBG_UNFILTERED("     vht_supported: %s",
                       band_cap->vht_caps().has_value() ? "true" : "false");
  if (band_cap->vht_caps().has_value()) {
    brcmf_dump_80211_vht_caps(&band_cap->vht_caps().value());
  }
}

static void brcmf_dump_if_query_info(fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse* info) {
  BRCMF_DBG_UNFILTERED(" Device capabilities as reported to wlanif:");
  if (info->sta_addr().has_value()) {
    BRCMF_DBG_UNFILTERED("   sta_addr: " FMT_MAC, FMT_MAC_ARGS(info->sta_addr()->data()));
  } else {
    BRCMF_DBG_UNFILTERED("   missing sta_addr");
  }

  if (info->role().has_value()) {
    BRCMF_DBG_UNFILTERED("   role(s): %s%s%s",
                         info->role() == fuchsia_wlan_common::WlanMacRole::kClient ? "client " : "",
                         info->role() == fuchsia_wlan_common::WlanMacRole::kAp ? "ap " : "",
                         info->role() == fuchsia_wlan_common::WlanMacRole::kMesh ? "mesh " : "");
  } else {
    BRCMF_DBG_UNFILTERED("    missing role");
  }

  if (info->band_caps().has_value() && info->band_caps()->size() > 0) {
    for (unsigned i = 0; i < info->band_caps()->size(); i++) {
      brcmf_dump_if_band_cap(&info->band_caps()->at(i));
    }
  } else {
    BRCMF_DBG_UNFILTERED("    missing band caps");
  }
}

void brcmf_if_query(net_device* ndev, fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse& resp) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct wireless_dev* wdev = ndev_to_wdev(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  struct brcmf_chanspec_list* list = nullptr;
  uint32_t nmode = 0;
  uint32_t vhtmode = 0;
  uint32_t rxchain = 0, nchain = 0;
  uint32_t bw_cap[2] = {WLC_BW_20MHZ_BIT, WLC_BW_20MHZ_BIT};
  uint32_t ldpc_cap = 0;
  uint32_t max_ampdu_len_exp = 0;
  zx_status_t status;
  bcme_status_t fw_err = BCME_OK;

  BRCMF_IFDBG(WLANIF, ndev, "Query request received from SME.");

  // mac_addr
  resp.sta_addr().emplace();
  memcpy(resp.sta_addr()->data(), ifp->mac_addr, ETH_ALEN);

  // factory_addr, same as mac_addr
  resp.factory_addr().emplace();
  memcpy(resp.factory_addr()->data(), ifp->mac_addr, ETH_ALEN);

  // role
  if (wdev->iftype.IsUnknown()) {
    BRCMF_ERR("Invalid wdev->iftype obtained : %u", fidl::ToUnderlying(wdev->iftype));
  } else {
    resp.role(wdev->iftype);
  }

  // bands
  uint32_t bandlist[3];
  status = brcmf_fil_cmd_data_get(ifp, BRCMF_C_GET_BANDLIST, &bandlist, sizeof(bandlist), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("could not obtain band info: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    return;
  }

  fuchsia_wlan_fullmac::BandCapability* band_cap_2ghz = nullptr;
  fuchsia_wlan_fullmac::BandCapability* band_cap_5ghz = nullptr;

  // Firmware puts the number of bands in the first entry in |bandlist|.
  resp.band_caps().emplace();
  resp.band_caps()->resize(bandlist[0]);
  for (unsigned i = 1; i <= bandlist[0] && i < std::size(bandlist); i++) {
    fuchsia_wlan_fullmac::BandCapability* band_cap = &resp.band_caps()->at(i - 1);
    band_cap->basic_rates().emplace();

    // MLME requires primary channels for each band capability.
    band_cap->primary_channels().emplace();

    if (bandlist[i] == WLC_BAND_2G) {
      band_cap->band(fuchsia_wlan_ieee80211::WlanBand::kTwoGhz);

      constexpr uint8_t kNumSupported2GRates =
          std::min<size_t>(fuchsia::wlan::ieee80211::MAX_SUPPORTED_BASIC_RATES, wl_g_rates_size);
      band_cap->basic_rates()->resize(kNumSupported2GRates);

      // Ensure that element sizes are identical because we will memcpy them.
      static_assert(sizeof(band_cap->basic_rates()->at(0)) == sizeof(wl_g_rates[0]));

      memcpy(band_cap->basic_rates()->data(), wl_g_rates,
             kNumSupported2GRates * sizeof(band_cap->basic_rates()->at(0)));
      band_cap_2ghz = band_cap;
    } else if (bandlist[i] == WLC_BAND_5G) {
      band_cap->band(fuchsia_wlan_ieee80211::WlanBand::kFiveGhz);

      constexpr uint8_t kNumSupported5GRates =
          std::min<size_t>(fuchsia::wlan::ieee80211::MAX_SUPPORTED_BASIC_RATES, wl_a_rates_size);
      band_cap->basic_rates()->resize(kNumSupported5GRates);

      // Ensure that element sizes are identical because we will memcpy them.
      static_assert(sizeof(band_cap->basic_rates()->at(0)) == sizeof(wl_a_rates[0]));

      memcpy(band_cap->basic_rates()->data(), wl_a_rates,
             kNumSupported5GRates * sizeof(band_cap->basic_rates()->at(0)));
      band_cap_5ghz = band_cap;
    }
  }

  // channels
  uint8_t* pbuf = static_cast<decltype(pbuf)>(calloc(BRCMF_DCMD_MEDLEN, 1));
  if (pbuf == nullptr) {
    BRCMF_ERR("unable to allocate memory for channel information");
    return;
  }
  auto free_pbuf = fit::defer([pbuf]() { free(pbuf); });

  status = brcmf_fil_iovar_data_get(ifp, "chanspecs", pbuf, BRCMF_DCMD_MEDLEN, &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("get chanspecs error: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    return;
  }
  list = (struct brcmf_chanspec_list*)pbuf;
  for (uint32_t i = 0; i < list->count; i++) {
    struct brcmu_chan ch;
    ch.chspec = list->element[i];
    cfg->d11inf.decchspec(&ch);

    // Find the appropriate band
    fuchsia_wlan_fullmac::BandCapability* band_cap = nullptr;
    if (ch.band == BRCMU_CHAN_BAND_2G) {
      band_cap = band_cap_2ghz;
    } else if (ch.band == BRCMU_CHAN_BAND_5G) {
      band_cap = band_cap_5ghz;
    } else {
      BRCMF_ERR("unrecognized band for channel %d", ch.control_ch_num);
      continue;
    }
    if (band_cap == nullptr) {
      continue;
    }

    // Fuchsia's wlan channels are simply the control channel (for now), whereas
    // brcm specifies each channel + bw + sb configuration individually. Until we
    // offer that level of resolution, just filter out duplicates.
    fuchsia_wlan_ieee80211::ChannelNumber new_chan;
    new_chan.band(ch.band == BRCMU_CHAN_BAND_2G ? fuchsia_wlan_ieee80211::WlanBand::kTwoGhz
                                                : fuchsia_wlan_ieee80211::WlanBand::kFiveGhz);
    new_chan.number(ch.control_ch_num);
    auto& op_chans = band_cap->primary_channels().value();
    if (std::find_if(op_chans.begin(), op_chans.end(), [&](const auto& item) {
          return item.number() == new_chan.number() && item.band() == new_chan.band();
        }) != op_chans.end()) {
      continue;
    }
    band_cap->primary_channels()->push_back(new_chan);
  }

  // Parse HT/VHT information
  nmode = 0;
  vhtmode = 0;
  rxchain = 0;
  nchain = 0;
  (void)brcmf_fil_iovar_int_get(ifp, "vhtmode", &vhtmode, nullptr);
  status = brcmf_fil_iovar_int_get(ifp, "nmode", &nmode, &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("nmode error: %s, fw err %s. Assuming both HT mode and VHT mode are not available.",
              zx_status_get_string(status), brcmf_fil_get_errstr(fw_err));
    // VHT requires HT support
    vhtmode = 0;
  } else {
    brcmf_get_bwcap(ifp, bw_cap);
  }
  BRCMF_DBG(QUERY, "nmode=%d, vhtmode=%d, bw_cap=(%d, %d)", nmode, vhtmode,
            bw_cap[static_cast<uint8_t>(fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz)],
            bw_cap[static_cast<uint8_t>(fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz)]);

  // LDPC support, applies to both HT and VHT
  ldpc_cap = 0;
  (void)brcmf_fil_iovar_int_get(ifp, "ldpc_cap", &ldpc_cap, nullptr);

  // Max AMPDU length
  max_ampdu_len_exp = 0;
  status = brcmf_fil_iovar_int_get(ifp, "ampdu_rx_factor", &max_ampdu_len_exp, nullptr);
  if (status != ZX_OK) {
    BRCMF_ERR("Failed to retrieve value for AMPDU maximum Rx length. Using 8191 bytes");
  }

  // Rx chains (and streams)
  // The "rxstreams_cap" iovar, when present, indicates the maximum number of Rx streams
  // possible, encoded as one bit per stream (i.e., a value of 0x3 indicates 2 streams/chains).
  if (brcmf_feat_is_quirk_enabled(ifp, BRCMF_FEAT_QUIRK_IS_4359)) {
    // TODO(https://fxbug.dev/42103822): The BCM4359 firmware supports rxstreams_cap, but it returns
    // 0x2 instead of 0x3, which is incorrect.
    rxchain = 0x3;
  } else {
    // According to Broadcom, rxstreams_cap, when available, is an accurate representation of
    // the number of rx chains.
    status = brcmf_fil_iovar_int_get(ifp, "rxstreams_cap", &rxchain, nullptr);
    if (status != ZX_OK) {
      // TODO(https://fxbug.dev/42103822): The rxstreams_cap iovar isn't yet supported in the
      // BCM4356 firmware. For now we use a hard-coded value (another option would be to parse the
      // nvram contents ourselves (looking for the value associated with the key "rxchain").
      BRCMF_INFO("Failed to retrieve value for Rx chains. Assuming chip supports 2 Rx chains.");
      rxchain = 0x3;
    }
  }

  for (nchain = 0; rxchain; nchain++) {
    rxchain = rxchain & (rxchain - 1);
  }
  BRCMF_DBG(QUERY, "nchain=%d", nchain);

  if (nmode) {
    if (band_cap_2ghz) {
      band_cap_2ghz->ht_caps().emplace();
      // band_cap_2ghz->band() is set earlier in this function. It is safe to access here.
      brcmf_update_ht_cap(ifp, band_cap_2ghz->ht_caps().value(), band_cap_2ghz->band().value(),
                          bw_cap, ldpc_cap, nchain, max_ampdu_len_exp);
    }
    if (band_cap_5ghz) {
      band_cap_5ghz->ht_caps().emplace();
      // band_cap_5ghz->band() is set earlier in this function. It is safe to access here.
      brcmf_update_ht_cap(ifp, band_cap_5ghz->ht_caps().value(), band_cap_5ghz->band().value(),
                          bw_cap, ldpc_cap, nchain, max_ampdu_len_exp);
    }
  }
  if (vhtmode && band_cap_5ghz) {
    band_cap_5ghz->vht_caps().emplace();
    // band_cap_5ghz->band() is set earlier in this function. It is safe to access here.
    brcmf_update_vht_cap(ifp, band_cap_5ghz->vht_caps().value(), band_cap_5ghz->band().value(),
                         bw_cap, nchain, ldpc_cap, max_ampdu_len_exp);
  }

  if (BRCMF_IS_ON(QUERY)) {
    brcmf_dump_if_query_info(&resp);
  }
}

void brcmf_if_query_security_support(net_device* ndev, fuchsia_wlan_common::SecuritySupport* resp) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  BRCMF_IFDBG(WLANIF, ndev, "Query security feature support request received from SME.");

  *resp = fuchsia_wlan_common::SecuritySupport{{
      .sae = fuchsia_wlan_common::SaeFeature{{
          .driver_handler_supported = false,
          .sme_handler_supported = brcmf_feat_is_enabled(ifp, BRCMF_FEAT_EXTSAE),
      }},
      .mfp = fuchsia_wlan_common::MfpFeature{{
          .supported = brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFP),
      }},
  }};
}

void brcmf_if_query_spectrum_management_support(
    net_device* ndev, fuchsia_wlan_common::SpectrumManagementSupport* resp) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  BRCMF_IFDBG(WLANIF, ndev, "Query spectrum management support request received from SME.");

  *resp = fuchsia_wlan_common::SpectrumManagementSupport{{
      .dfs = fuchsia_wlan_common::DfsFeature{{
          .supported = brcmf_feat_is_enabled(ifp, BRCMF_FEAT_DFS),
      }},
  }};
}

void brcmf_if_query_telemetry_support(net_device* ndev,
                                      fuchsia_wlan_stats::wire::TelemetrySupport* resp,
                                      fidl::AnyArena& arena) {
  BRCMF_IFDBG(WLANIF, ndev, "Telemetry support request received from SME.");

  std::vector<fuchsia_wlan_stats::wire::InspectCounterConfig> inspect_counter_configs;
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_OCAST.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::DRIVER_RX_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::DRIVER_RX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::DRIVER_TX_TOTAL.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::DRIVER_TX_CONF.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::DRIVER_TX_DROP.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::DRIVER_TX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_VO_RX_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_VO_RX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_VO_TX_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_VO_TX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_VI_RX_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_VI_RX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_VI_TX_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_VI_TX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_BE_RX_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_BE_RX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_BE_TX_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_BE_TX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_BK_RX_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_BK_RX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_BK_TX_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::WME_BK_TX_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_RETRANSMITS.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_DATA_ERRORS.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_STATUS_ERRORS.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_NO_BUFFER.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_RUNT_FRAMES.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_UNDERFLOW.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_PHY_ERRORS.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_DOT11_FAILURES.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_NO_ASSOC.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_TX_NO_ACK.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_DATA_ERRORS.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_OVERFLOW.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_NO_BUFFER.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_RUNT_FRAMES.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_FRAGMENTATION_ERRORS.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_BAD_PLCP.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_CRS_GLITCH.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_BAD_FCS.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_GIANT_FRAMES.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_NO_SCB.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_BAD_SRC_MAC.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::FW_RX_DECRYPT_FAILURES.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::SDIO_FLOW_CONTROL_EVENTS.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::SDIO_TX_CTRL_FRAME_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::SDIO_TX_CTRL_FRAME_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::SDIO_RX_CTRL_FRAME_GOOD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::SDIO_RX_CTRL_FRAME_BAD.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::SDIO_RX_OUT_OF_BUFS.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::SDIO_INTERRUPTS.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::SDIO_RX_HEADERS_READ.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::SDIO_RX_PACKETS_READ.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::SDIO_TX_PACKETS_WRITE.toFidl(arena));
  inspect_counter_configs.push_back(CounterConfigs::BT_COEX_WLAN_PREEMPT_COUNT.toFidl(arena));

  std::vector<fuchsia_wlan_stats::wire::InspectGaugeConfig> inspect_gauge_configs;
  inspect_gauge_configs.push_back(GaugeConfigs::SDIO_TX_SEQ.toFidl(arena));
  inspect_gauge_configs.push_back(GaugeConfigs::SDIO_TX_MAX.toFidl(arena));
  inspect_gauge_configs.push_back(GaugeConfigs::SDIO_TX_QUEUE_LEN.toFidl(arena));
  inspect_gauge_configs.push_back(GaugeConfigs::SDIO_TX_QUEUE_0_LEN.toFidl(arena));
  inspect_gauge_configs.push_back(GaugeConfigs::SDIO_TX_QUEUE_1_LEN.toFidl(arena));
  inspect_gauge_configs.push_back(GaugeConfigs::SDIO_TX_QUEUE_2_LEN.toFidl(arena));
  inspect_gauge_configs.push_back(GaugeConfigs::SDIO_TX_QUEUE_3_LEN.toFidl(arena));

  *resp = fuchsia_wlan_stats::wire::TelemetrySupport::Builder(arena)
              .inspect_counter_configs(fidl::VectorView(arena, inspect_counter_configs))
              .inspect_gauge_configs(fidl::VectorView(arena, inspect_gauge_configs))
              .Build();
}

namespace {

zx_status_t brcmf_convert_antenna_id(const histograms_report_t& histograms_report,
                                     fuchsia_wlan_stats::wire::AntennaId* out_antenna_id) {
  switch (histograms_report.antennaid.freq) {
    case ANTENNA_2G:
      out_antenna_id->freq = fuchsia_wlan_stats::wire::AntennaFreq::kAntenna2G;
      break;
    case ANTENNA_5G:
      out_antenna_id->freq = fuchsia_wlan_stats::wire::AntennaFreq::kAntenna5G;
      break;
    default:
      return ZX_ERR_OUT_OF_RANGE;
  }
  out_antenna_id->index = histograms_report.antennaid.idx;
  return ZX_OK;
}

void brcmf_get_noise_floor_samples(
    const histograms_report_t& histograms_report,
    std::vector<fuchsia_wlan_stats::wire::HistBucket>* out_noise_floor_samples,
    uint64_t* out_invalid_samples) {
  for (size_t i = 0; i < fuchsia_wlan_stats::wire::kMaxNoiseFloorSamples; ++i) {
    fuchsia_wlan_stats::wire::HistBucket bucket;
    bucket.bucket_index = i;
    bucket.num_samples = histograms_report.rxnoiseflr[i];
    // Do not add empty buckets.
    if (bucket.num_samples > 0) {
      out_noise_floor_samples->push_back(bucket);
    }
  }
  // rxnoiseflr has an extra bucket. If there is anything in it, it is invalid.
  *out_invalid_samples = histograms_report.rxsnr[255];
}

void brcmf_get_rssi_samples(const histograms_report_t& histograms_report,
                            std::vector<fuchsia_wlan_stats::wire::HistBucket>* out_rssi_samples,
                            uint64_t* out_invalid_samples) {
  for (size_t i = 0; i < fuchsia_wlan_stats::wire::kMaxRssiSamples; ++i) {
    fuchsia_wlan_stats::wire::HistBucket bucket;
    bucket.bucket_index = i;
    bucket.num_samples = histograms_report.rxrssi[i];
    // Do not add empty buckets.
    if (bucket.num_samples > 0) {
      out_rssi_samples->push_back(bucket);
    }
  }
  // rxrssi has an extra bucket. If there is anything in it, it is invalid.
  *out_invalid_samples = histograms_report.rxrssi[255];
}

void brcmf_get_snr_samples(const histograms_report_t& histograms_report,
                           std::vector<fuchsia_wlan_stats::wire::HistBucket>* out_snr_samples,
                           uint64_t* out_invalid_samples) {
  for (size_t i = 0; i < fuchsia_wlan_stats::wire::kMaxSnrSamples; ++i) {
    fuchsia_wlan_stats::wire::HistBucket bucket;
    bucket.bucket_index = i;
    bucket.num_samples = histograms_report.rxsnr[i];
    // Do not add empty buckets.
    if (bucket.num_samples > 0) {
      out_snr_samples->push_back(bucket);
    }
  }
  // rxsnr does not have any indices that should be considered invalid buckets.
  *out_invalid_samples = 0;
}

void brcmf_get_rx_rate_index_samples(
    const histograms_report_t& histograms_report,
    std::vector<fuchsia_wlan_stats::wire::HistBucket>* out_rx_rate_index_samples,
    uint64_t* out_invalid_samples) {
  uint32_t rxrate[fuchsia_wlan_stats::wire::kMaxRxRateIndexSamples];
  brcmu_set_rx_rate_index_hist_rx11ac(histograms_report.rx11ac, rxrate);
  brcmu_set_rx_rate_index_hist_rx11b(histograms_report.rx11b, rxrate);
  brcmu_set_rx_rate_index_hist_rx11g(histograms_report.rx11g, rxrate);
  brcmu_set_rx_rate_index_hist_rx11n(histograms_report.rx11n, rxrate);
  for (uint8_t i = 0; i < fuchsia_wlan_stats::wire::kMaxRxRateIndexSamples; ++i) {
    fuchsia_wlan_stats::wire::HistBucket bucket;
    bucket.bucket_index = i;
    bucket.num_samples = rxrate[i];
    // Do not add empty buckets.
    if (bucket.num_samples > 0) {
      out_rx_rate_index_samples->push_back(bucket);
    }
  }
  // rxrate does not have any indices that should be considered invalid buckets.
  *out_invalid_samples = 0;
}

void brcmf_convert_histograms_report_noise_floor(
    const histograms_report_t& histograms_report,
    const fuchsia_wlan_stats::wire::AntennaId& antenna_id,
    fuchsia_wlan_stats::wire::NoiseFloorHistogram* out_hist, fidl::AnyArena& arena) {
  out_hist->antenna_id = fidl::ObjectView<fuchsia_wlan_stats::wire::AntennaId>(arena, antenna_id);
  out_hist->hist_scope = fuchsia_wlan_stats::wire::HistScope::kPerAntenna;
  std::vector<fuchsia_wlan_stats::wire::HistBucket> samples;
  brcmf_get_noise_floor_samples(histograms_report, &samples, &out_hist->invalid_samples);
  out_hist->noise_floor_samples =
      fidl::VectorView<fuchsia_wlan_stats::wire::HistBucket>(arena, samples);
}

void brcmf_convert_histograms_report_rx_rate_index(
    const histograms_report_t& histograms_report,
    const fuchsia_wlan_stats::wire::AntennaId& antenna_id,
    fuchsia_wlan_stats::wire::RxRateIndexHistogram* out_hist, fidl::AnyArena& arena) {
  out_hist->antenna_id = fidl::ObjectView<fuchsia_wlan_stats::wire::AntennaId>(arena, antenna_id);
  out_hist->hist_scope = fuchsia_wlan_stats::wire::HistScope::kPerAntenna;
  std::vector<fuchsia_wlan_stats::wire::HistBucket> samples;
  brcmf_get_rx_rate_index_samples(histograms_report, &samples, &out_hist->invalid_samples);
  out_hist->rx_rate_index_samples =
      fidl::VectorView<fuchsia_wlan_stats::wire::HistBucket>(arena, samples);
}

void brcmf_convert_histograms_report_rssi(const histograms_report_t& histograms_report,
                                          const fuchsia_wlan_stats::wire::AntennaId& antenna_id,
                                          fuchsia_wlan_stats::wire::RssiHistogram* out_hist,
                                          fidl::AnyArena& arena) {
  out_hist->antenna_id = fidl::ObjectView<fuchsia_wlan_stats::wire::AntennaId>(arena, antenna_id);
  out_hist->hist_scope = fuchsia_wlan_stats::wire::HistScope::kPerAntenna;
  std::vector<fuchsia_wlan_stats::wire::HistBucket> samples;
  brcmf_get_rssi_samples(histograms_report, &samples, &out_hist->invalid_samples);
  out_hist->rssi_samples = fidl::VectorView<fuchsia_wlan_stats::wire::HistBucket>(arena, samples);
}

void brcmf_convert_histograms_report_snr(const histograms_report_t& histograms_report,
                                         const fuchsia_wlan_stats::wire::AntennaId& antenna_id,
                                         fuchsia_wlan_stats::wire::SnrHistogram* out_hist,
                                         fidl::AnyArena& arena) {
  out_hist->antenna_id = fidl::ObjectView<fuchsia_wlan_stats::wire::AntennaId>(arena, antenna_id);
  out_hist->hist_scope = fuchsia_wlan_stats::wire::HistScope::kPerAntenna;
  std::vector<fuchsia_wlan_stats::wire::HistBucket> samples;
  brcmf_get_snr_samples(histograms_report, &samples, &out_hist->invalid_samples);
  out_hist->snr_samples = fidl::VectorView<fuchsia_wlan_stats::wire::HistBucket>(arena, samples);
}

zx_status_t brcmf_get_histograms_report(brcmf_if* ifp, histograms_report_t* out_report) {
  if (ifp == nullptr) {
    BRCMF_ERR("Invalid interface");
    return ZX_ERR_INTERNAL;
  }
  if (out_report == nullptr) {
    BRCMF_ERR("Invalid histograms_report_t pointer");
    return ZX_ERR_INTERNAL;
  }

  bcme_status_t fw_err = BCME_OK;
  wl_wstats_cnt_t wl_stats_cnt;
  std::memset(&wl_stats_cnt, 0, sizeof(wl_wstats_cnt_t));
  const auto wstats_counters_status = brcmf_fil_iovar_data_get(
      ifp, "wstats_counters", &wl_stats_cnt, sizeof(wl_wstats_cnt_t), &fw_err);
  if (wstats_counters_status != ZX_OK) {
    BRCMF_ERR("Failed to get wstats_counters: %s, fw err %s",
              zx_status_get_string(wstats_counters_status), brcmf_fil_get_errstr(fw_err));
    return wstats_counters_status;
  }

  uint32_t chanspec = 0;
  const auto chanspec_status = brcmf_fil_iovar_int_get(ifp, "chanspec", &chanspec, &fw_err);
  if (chanspec_status != ZX_OK) {
    BRCMF_ERR("Failed to retrieve chanspec: %s, fw err %s", zx_status_get_string(chanspec_status),
              brcmf_fil_get_errstr(fw_err));
    return chanspec_status;
  }

  uint32_t version;
  const auto version_status = brcmf_fil_cmd_int_get(ifp, BRCMF_C_GET_VERSION, &version, &fw_err);
  if (version_status != ZX_OK) {
    BRCMF_ERR("Failed to retrieve version: %s, fw err %s", zx_status_get_string(version_status),
              brcmf_fil_get_errstr(fw_err));
    return version_status;
  }

  uint32_t rxchain = 0;
  const auto rxchain_status = brcmf_fil_iovar_int_get(ifp, "rxchain", &rxchain, &fw_err);
  if (rxchain_status != ZX_OK) {
    BRCMF_ERR("Failed to retrieve rxchain: %s, fw err %s", zx_status_get_string(rxchain_status),
              brcmf_fil_get_errstr(fw_err));
    return rxchain_status;
  }

  const bool get_histograms_success =
      get_histograms(wl_stats_cnt, static_cast<chanspec_t>(chanspec), version, rxchain, out_report);
  if (get_histograms_success) {
    return ZX_OK;
  }
  BRCMF_ERR("Failed to get per-antenna metrics");
  return ZX_ERR_INTERNAL;
}

}  // namespace

zx_status_t brcmf_if_get_iface_stats(net_device* ndev,
                                     fuchsia_wlan_stats::wire::IfaceStats* out_stats,
                                     fidl::AnyArena& arena) {
  if (ndev == nullptr) {
    return ZX_ERR_INTERNAL;
  }

  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid() || !ndev->is_up) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping get iface stats");
    return ZX_ERR_INTERNAL;
  }

  struct brcmf_if* ifp = ndev_to_if(ndev);
  if (ifp == nullptr || ifp->vif == nullptr || ifp->drvr == nullptr ||
      ifp->drvr->config == nullptr) {
    BRCMF_IFDBG(WLANIF, ndev, "interface not ready -- skipping get iface stats");
    return ZX_ERR_INTERNAL;
  }
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
    // MFG builds do not support many of the stats iovars.
    return ZX_ERR_NOT_SUPPORTED;
  }

  auto stats_builder = fuchsia_wlan_stats::wire::IfaceStats::Builder(arena);
  std::vector<fuchsia_wlan_stats::wire::UnnamedCounter> iface_counters =
      brcmf_bus_get_counters(cfg->pub->bus_if);
  uint32_t wlan_preempt_count = brcmf_btcoex_get_wlan_preempt_count(ifp);
  iface_counters.push_back(CounterConfigs::BT_COEX_WLAN_PREEMPT_COUNT.unnamed(wlan_preempt_count));
  stats_builder.driver_specific_counters(iface_counters);

  std::vector<fuchsia_wlan_stats::wire::UnnamedGauge> sdio_gauges =
      brcmf_bus_get_gauges(cfg->pub->bus_if);
  stats_builder.driver_specific_gauges(sdio_gauges);

  if (ifp->vif == nullptr ||
      !brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state)) {
    *out_stats = stats_builder.Build();
    return ZX_OK;
  }

  brcmf_pktcnt_le pktcnt;
  bcme_status_t fw_err;
  zx_status_t status =
      brcmf_fil_cmd_data_get(ifp, BRCMF_C_GET_GET_PKTCNTS, &pktcnt, sizeof(pktcnt), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("could not get pkt cnts: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    return status;
  }

  BRCMF_DBG(DATA, "Cntrs: rxgood:%d rxbad:%d txgood:%d txbad:%d rxocast:%d", pktcnt.rx_good_pkt,
            pktcnt.rx_bad_pkt, pktcnt.tx_good_pkt, pktcnt.tx_bad_pkt, pktcnt.rx_ocast_good_pkt);

  auto connection_stats_builder =
      fuchsia_wlan_stats::wire::ConnectionStats::Builder(arena)
          .connection_id(ifp->connection_id)
          .rx_unicast_total(pktcnt.rx_good_pkt + pktcnt.rx_bad_pkt + ndev->stats.rx_errors)
          .rx_unicast_drop(pktcnt.rx_bad_pkt + ndev->stats.rx_errors)
          .rx_multicast(pktcnt.rx_ocast_good_pkt)
          .tx_total(pktcnt.tx_good_pkt + pktcnt.tx_bad_pkt + ndev->stats.tx_dropped)
          .tx_drop(pktcnt.tx_bad_pkt + ndev->stats.tx_dropped);

  std::vector<fuchsia_wlan_stats::wire::UnnamedCounter> connection_counters;
  connection_counters.push_back(CounterConfigs::FW_RX_GOOD.unnamed(pktcnt.rx_good_pkt));
  connection_counters.push_back(CounterConfigs::FW_RX_BAD.unnamed(pktcnt.rx_bad_pkt));
  connection_counters.push_back(CounterConfigs::FW_RX_OCAST.unnamed(pktcnt.rx_ocast_good_pkt));
  connection_counters.push_back(CounterConfigs::FW_TX_GOOD.unnamed(pktcnt.tx_good_pkt));
  connection_counters.push_back(CounterConfigs::FW_TX_BAD.unnamed(pktcnt.rx_bad_pkt));
  connection_counters.push_back(CounterConfigs::DRIVER_RX_GOOD.unnamed(ndev->stats.rx_packets));
  connection_counters.push_back(CounterConfigs::DRIVER_RX_BAD.unnamed(ndev->stats.rx_errors));
  connection_counters.push_back(CounterConfigs::DRIVER_TX_TOTAL.unnamed(ndev->stats.tx_packets));
  connection_counters.push_back(CounterConfigs::DRIVER_TX_CONF.unnamed(ndev->stats.tx_confirmed));
  connection_counters.push_back(CounterConfigs::DRIVER_TX_DROP.unnamed(ndev->stats.tx_dropped));
  connection_counters.push_back(CounterConfigs::DRIVER_TX_BAD.unnamed(ndev->stats.tx_errors));

  // Get the WME counters
  wl_wme_cnt_t wme_cnt;
  status = brcmf_fil_iovar_data_get(ifp, "wme_counters", &wme_cnt, sizeof(wl_wme_cnt_t), &fw_err);
  if (status != ZX_OK) {
    BRCMF_INFO("Unable to get WME counters err: %s fw err %s", zx_status_get_string(status),
               brcmf_fil_get_errstr(fw_err));
  } else {
    connection_counters.push_back(
        CounterConfigs::WME_VO_RX_GOOD.unnamed(wme_cnt.rx[AC_VO].packets));
    connection_counters.push_back(
        CounterConfigs::WME_VO_RX_BAD.unnamed(wme_cnt.rx_failed[AC_VO].packets));
    connection_counters.push_back(
        CounterConfigs::WME_VO_TX_GOOD.unnamed(wme_cnt.tx[AC_VO].packets));
    connection_counters.push_back(
        CounterConfigs::WME_VO_TX_BAD.unnamed(wme_cnt.tx_failed[AC_VO].packets));
    connection_counters.push_back(
        CounterConfigs::WME_VI_RX_GOOD.unnamed(wme_cnt.rx[AC_VI].packets));
    connection_counters.push_back(
        CounterConfigs::WME_VI_RX_BAD.unnamed(wme_cnt.rx_failed[AC_VI].packets));
    connection_counters.push_back(
        CounterConfigs::WME_VI_TX_GOOD.unnamed(wme_cnt.tx[AC_VI].packets));
    connection_counters.push_back(
        CounterConfigs::WME_VI_TX_BAD.unnamed(wme_cnt.tx_failed[AC_VI].packets));
    connection_counters.push_back(
        CounterConfigs::WME_BE_RX_GOOD.unnamed(wme_cnt.rx[AC_BE].packets));
    connection_counters.push_back(
        CounterConfigs::WME_BE_RX_BAD.unnamed(wme_cnt.rx_failed[AC_BE].packets));
    connection_counters.push_back(
        CounterConfigs::WME_BE_TX_GOOD.unnamed(wme_cnt.tx[AC_BE].packets));
    connection_counters.push_back(
        CounterConfigs::WME_BE_TX_BAD.unnamed(wme_cnt.tx_failed[AC_BE].packets));
    connection_counters.push_back(
        CounterConfigs::WME_BK_RX_GOOD.unnamed(wme_cnt.rx[AC_BK].packets));
    connection_counters.push_back(
        CounterConfigs::WME_BK_RX_BAD.unnamed(wme_cnt.rx_failed[AC_BK].packets));
    connection_counters.push_back(
        CounterConfigs::WME_BK_TX_GOOD.unnamed(wme_cnt.tx[AC_BK].packets));
    connection_counters.push_back(
        CounterConfigs::WME_BK_TX_BAD.unnamed(wme_cnt.tx_failed[AC_BK].packets));

    auto to_wire_pkt = [&](uint32_t ac) {
      auto builder = fuchsia_wlan_stats::wire::PacketStats::Builder(arena);
      builder.rx(wme_cnt.rx[ac].packets);
      builder.tx(wme_cnt.tx[ac].packets);
      builder.tx_drop(wme_cnt.tx_failed[ac].packets);
      // Per-AC tx retries are not supported in WME V1 (wl_wme_cnt_t), so leave retries unset
      // (none).
      return builder.Build();
    };
    auto wme_builder = fuchsia_wlan_stats::wire::WmePacketStats::Builder(arena);
    wme_builder.vo(to_wire_pkt(AC_VO));
    wme_builder.vi(to_wire_pkt(AC_VI));
    wme_builder.be(to_wire_pkt(AC_BE));
    wme_builder.bk(to_wire_pkt(AC_BK));
    connection_stats_builder.wme(wme_builder.Build());
  }

  uint8_t cnt_buf[BRCMF_DCMD_MAXLEN] = {0};
  // The version # in the counters struct returned by FW is set to 10 currently but its
  // corresponding struct definition is not available. It appears each new version is a superset
  // of the previous one. So tell FW the size of the struct is that of wl_cnt_ver_11_t which is >=
  // "wl_cnt_ver_10_t".
  status = brcmf_fil_iovar_data_get(ifp, "counters", cnt_buf, sizeof(wl_cnt_ver_11_t), &fw_err);
  if (status != ZX_OK) {
    BRCMF_WARN("Unable to get fw counters err: %s fw_err %d", zx_status_get_string(status), fw_err);
  } else {
    wl_cnt_ver_6_t* counters = reinterpret_cast<wl_cnt_ver_6_t*>(cnt_buf);
    connection_counters.push_back(CounterConfigs::FW_TX_RETRANSMITS.unnamed(counters->txretrans));
    connection_counters.push_back(CounterConfigs::FW_TX_DATA_ERRORS.unnamed(counters->txerror));
    connection_counters.push_back(CounterConfigs::FW_TX_STATUS_ERRORS.unnamed(counters->txserr));
    connection_counters.push_back(CounterConfigs::FW_TX_NO_BUFFER.unnamed(counters->txnobuf));
    connection_counters.push_back(CounterConfigs::FW_TX_RUNT_FRAMES.unnamed(counters->txrunt));
    connection_counters.push_back(CounterConfigs::FW_TX_UNDERFLOW.unnamed(counters->txuflo));
    connection_counters.push_back(CounterConfigs::FW_TX_PHY_ERRORS.unnamed(counters->txphyerr));
    connection_counters.push_back(CounterConfigs::FW_TX_DOT11_FAILURES.unnamed(counters->txfail));
    connection_counters.push_back(CounterConfigs::FW_TX_NO_ASSOC.unnamed(counters->txnoassoc));
    connection_counters.push_back(CounterConfigs::FW_TX_NO_ACK.unnamed(counters->txnoack));
    connection_counters.push_back(CounterConfigs::FW_RX_DATA_ERRORS.unnamed(counters->rxerror));
    connection_counters.push_back(CounterConfigs::FW_RX_OVERFLOW.unnamed(counters->rxoflo));
    connection_counters.push_back(CounterConfigs::FW_RX_NO_BUFFER.unnamed(counters->rxnobuf));
    connection_counters.push_back(CounterConfigs::FW_RX_RUNT_FRAMES.unnamed(counters->rxrunt));
    connection_counters.push_back(
        CounterConfigs::FW_RX_FRAGMENTATION_ERRORS.unnamed(counters->rxfragerr));
    connection_counters.push_back(CounterConfigs::FW_RX_BAD_PLCP.unnamed(counters->rxbadplcp));
    connection_counters.push_back(CounterConfigs::FW_RX_CRS_GLITCH.unnamed(counters->rxcrsglitch));
    connection_counters.push_back(CounterConfigs::FW_RX_BAD_FCS.unnamed(counters->rxbadfcs));
    connection_counters.push_back(CounterConfigs::FW_RX_GIANT_FRAMES.unnamed(counters->rxgiant));
    connection_counters.push_back(CounterConfigs::FW_RX_NO_SCB.unnamed(counters->rxnoscb));
    connection_counters.push_back(CounterConfigs::FW_RX_BAD_SRC_MAC.unnamed(counters->rxbadsrcmac));
    connection_counters.push_back(
        CounterConfigs::FW_RX_DECRYPT_FAILURES.unnamed(counters->rxundec));
  }

  connection_stats_builder.driver_specific_counters(fidl::VectorView(arena, connection_counters));
  *out_stats = stats_builder.connection_stats(connection_stats_builder.Build()).Build();
  return ZX_OK;
}

zx_status_t brcmf_if_get_iface_histogram_stats(
    net_device* ndev, fuchsia_wlan_stats::wire::IfaceHistogramStats* out_stats,
    fidl::AnyArena& arena) {
  if (ndev == nullptr) {
    return ZX_ERR_INTERNAL;
  }

  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid() || !ndev->is_up) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping get iface histogram stats");
    return ZX_ERR_INTERNAL;
  }
  struct brcmf_if* ifp = ndev_to_if(ndev);
  if (ifp == nullptr || ifp->vif == nullptr || ifp->drvr == nullptr) {
    BRCMF_IFDBG(WLANIF, ndev, "interface not ready -- skipping get iface histogram stats");
    return ZX_ERR_INTERNAL;
  }
  auto stats_builder = fuchsia_wlan_stats::wire::IfaceHistogramStats::Builder(arena);

  ndev->stats.noise_floor_histograms = {};
  ndev->stats.rssi_histograms = {};
  ndev->stats.rx_rate_index_histograms = {};
  ndev->stats.snr_histograms = {};

  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
    // MFG builds do not support many of the stats iovars.
    return ZX_ERR_NOT_SUPPORTED;
  }

  // If detailed histogram feature is not enabled, do nothing.
  if (!brcmf_feat_is_enabled(ifp->drvr, BRCMF_FEAT_DHIST)) {
    return ZX_ERR_NOT_CONNECTED;
  }

  if (ifp->vif == nullptr ||
      !brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state)) {
    return ZX_ERR_NOT_CONNECTED;
  }

  histograms_report_t histograms_report;
  const auto hist_status = brcmf_get_histograms_report(ifp, &histograms_report);
  if (hist_status != ZX_OK) {
    return hist_status;
  }
  fuchsia_wlan_stats::wire::AntennaId antenna_id;
  const auto antenna_id_status = brcmf_convert_antenna_id(histograms_report, &antenna_id);
  if (antenna_id_status != ZX_OK) {
    BRCMF_ERR("Invalid antenna ID, freq: %d idx: %d", histograms_report.antennaid.freq,
              histograms_report.antennaid.idx);
    return antenna_id_status;
  }
  ndev->stats.noise_floor_histograms.resize(1);
  brcmf_convert_histograms_report_noise_floor(histograms_report, antenna_id,
                                              ndev->stats.noise_floor_histograms.data(), arena);
  ndev->stats.rssi_histograms.resize(1);
  brcmf_convert_histograms_report_rssi(histograms_report, antenna_id,
                                       ndev->stats.rssi_histograms.data(), arena);
  ndev->stats.rx_rate_index_histograms.resize(1);
  brcmf_convert_histograms_report_rx_rate_index(histograms_report, antenna_id,
                                                ndev->stats.rx_rate_index_histograms.data(), arena);
  ndev->stats.snr_histograms.resize(1);
  brcmf_convert_histograms_report_snr(histograms_report, antenna_id,
                                      ndev->stats.snr_histograms.data(), arena);

  // Conversion from banjo to FIDL table.
  stats_builder.noise_floor_histograms(
      fidl::VectorView<fuchsia_wlan_stats::wire::NoiseFloorHistogram>(
          arena, ndev->stats.noise_floor_histograms));
  stats_builder.rssi_histograms(fidl::VectorView<fuchsia_wlan_stats::wire::RssiHistogram>(
      arena, ndev->stats.rssi_histograms));
  stats_builder.rx_rate_index_histograms(
      fidl::VectorView<fuchsia_wlan_stats::wire::RxRateIndexHistogram>(
          arena, ndev->stats.rx_rate_index_histograms));
  stats_builder.snr_histograms(
      fidl::VectorView<fuchsia_wlan_stats::wire::SnrHistogram>(arena, ndev->stats.snr_histograms));

  *out_stats = stats_builder.Build();

  return ZX_OK;
}

zx_status_t brcmf_if_get_signal_report(net_device* ndev,
                                       fuchsia_wlan_stats::wire::SignalReport* out_signal_report,
                                       fidl::AnyArena& arena) {
  if (ndev == nullptr) {
    return ZX_ERR_INTERNAL;
  }

  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid() || !ndev->is_up) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping get signal report");
    return ZX_ERR_INTERNAL;
  }

  struct brcmf_if* ifp = ndev_to_if(ndev);
  if (ifp == nullptr || ifp->vif == nullptr || ifp->drvr == nullptr ||
      ifp->drvr->config == nullptr) {
    BRCMF_IFDBG(WLANIF, ndev, "interface not ready -- skipping get signal report");
    return ZX_ERR_INTERNAL;
  }
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  auto stats_builder = fuchsia_wlan_stats::wire::SignalReport::Builder(arena);

  if (ifp->vif == nullptr ||
      !brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state)) {
    *out_signal_report = stats_builder.Build();
    return ZX_OK;
  }

  auto connection_signal_report_builder =
      fuchsia_wlan_stats::wire::ConnectionSignalReport::Builder(arena);

  int8_t rssi = 0, snr = 0;
  zx_status_t status = brcmf_get_rssi_snr(ndev, &rssi, &snr);
  if (status == ZX_OK) {
    connection_signal_report_builder.rssi_dbm(rssi).snr_db(snr);
  } else {
    BRCMF_INFO("Failed to get RSSI and SNR: %s", zx_status_get_string(status));
  }

  uint32_t fw_rate = 0;
  bcme_status_t fw_err = BCME_OK;
  status = brcmf_fil_cmd_data_get(ifp, BRCMF_C_GET_RATE, &fw_rate, sizeof(fw_rate), &fw_err);
  if (status == ZX_OK) {
    connection_signal_report_builder.tx_rate_500kbps(fw_rate);
  } else {
    BRCMF_INFO("Failed to get rate: %s, fw err %s", zx_status_get_string(status),
               brcmf_fil_get_errstr(fw_err));
  }

  uint16_t chanspec = 0;
  uint8_t ctl_chan = 0;
  status = brcmf_get_ctrl_channel(ifp, &chanspec, &ctl_chan);
  if (status == ZX_OK) {
    connection_signal_report_builder
        .primary(chanspec_to_primary_channel_number(&cfg->d11inf, chanspec))
        .bandwidth(chanspec_to_channel_bandwidth(&cfg->d11inf, chanspec))
        .vht_secondary_80_channel(chanspec_to_secondary80(&cfg->d11inf, chanspec));
  } else {
    BRCMF_INFO("Failed to get control channel: %s", zx_status_get_string(status));
  }

  *out_signal_report =
      stats_builder.connection_signal_report(connection_signal_report_builder.Build()).Build();
  return ZX_OK;
}

static void brcmf_clear_assoc_ies(struct brcmf_cfg80211_info* cfg) {
  struct brcmf_cfg80211_connect_info* conn_info = cfg_to_conn(cfg);

  free(conn_info->req_ie);
  conn_info->req_ie = nullptr;
  conn_info->req_ie_len = 0;
  free(conn_info->resp_ie);
  conn_info->resp_ie = nullptr;
  conn_info->resp_ie_len = 0;
}

static zx_status_t brcmf_get_assoc_ies(struct brcmf_cfg80211_info* cfg, struct brcmf_if* ifp) {
  struct brcmf_cfg80211_assoc_ielen_le* assoc_info;
  struct brcmf_cfg80211_connect_info* conn_info = cfg_to_conn(cfg);
  uint32_t req_len;
  uint32_t resp_len;
  zx_status_t err = ZX_OK;
  bcme_status_t fw_err = BCME_OK;

  brcmf_clear_assoc_ies(cfg);
  err = brcmf_fil_iovar_data_get(ifp, "assoc_info", cfg->extra_buf, WL_ASSOC_INFO_MAX, &fw_err);
  if (err != ZX_OK) {
    BRCMF_ERR("could not get assoc info: %s, fw err %s", zx_status_get_string(err),
              brcmf_fil_get_errstr(fw_err));
    return err;
  }
  assoc_info = (struct brcmf_cfg80211_assoc_ielen_le*)cfg->extra_buf;
  req_len = assoc_info->req_len;
  resp_len = assoc_info->resp_len;
  if (req_len) {
    err =
        brcmf_fil_iovar_data_get(ifp, "assoc_req_ies", cfg->extra_buf, WL_ASSOC_INFO_MAX, &fw_err);
    if (err != ZX_OK) {
      BRCMF_ERR("Could not get assoc req IEs: %s, fw err %s", zx_status_get_string(err),
                brcmf_fil_get_errstr(fw_err));
      return err;
    }
    conn_info->req_ie_len = req_len;
    conn_info->req_ie = static_cast<decltype(conn_info->req_ie)>(
        brcmu_alloc_and_copy(cfg->extra_buf, conn_info->req_ie_len));
    if (conn_info->req_ie == nullptr) {
      conn_info->req_ie_len = 0;
    }
  } else {
    conn_info->req_ie_len = 0;
    conn_info->req_ie = nullptr;
  }
  if (resp_len) {
    err =
        brcmf_fil_iovar_data_get(ifp, "assoc_resp_ies", cfg->extra_buf, WL_ASSOC_INFO_MAX, &fw_err);
    if (err != ZX_OK) {
      BRCMF_ERR("Could not get assoc resp IEs: %s, fw err %s", zx_status_get_string(err),
                brcmf_fil_get_errstr(fw_err));
      return err;
    }
    conn_info->resp_ie_len = resp_len;
    conn_info->resp_ie = static_cast<decltype(conn_info->resp_ie)>(
        brcmu_alloc_and_copy(cfg->extra_buf, conn_info->resp_ie_len));
    if (conn_info->resp_ie == nullptr) {
      conn_info->resp_ie_len = 0;
    }
  } else {
    BRCMF_WARN("No assoc resp IEs available");
    conn_info->resp_ie_len = 0;
    conn_info->resp_ie = nullptr;
  }
  BRCMF_DBG(CONN, "req len (%d) resp len (%d)", conn_info->req_ie_len, conn_info->resp_ie_len);
  return err;
}

static void brcmf_log_conn_status(brcmf_if* ifp, brcmf_connect_status_t connect_status) {
  BRCMF_DBG(CONN, "connect_status %s", brcmf_get_connect_status_str(connect_status));

  // We track specific failures that are of interest on inspect.
  switch (connect_status) {
    case brcmf_connect_status_t::CONNECTED:
      ifp->drvr->device->GetInspect()->LogConnSuccess();
      break;
    case brcmf_connect_status_t::AUTHENTICATION_FAILED:
      ifp->drvr->device->GetInspect()->LogConnAuthFail();
      break;
    case brcmf_connect_status_t::NO_NETWORK:
      ifp->drvr->device->GetInspect()->LogConnNoNetworkFail();
      break;
    default:
      ifp->drvr->device->GetInspect()->LogConnOtherFail();
      break;
  }
}

// Issues commands to clear firmware and AP connection states.
// Driver expects that firmware will send a deauthenticate frame to the current AP, but
// this is best effort.
static zx_status_t brcmf_clear_firmware_connection_state(brcmf_if* ifp) {
  struct brcmf_cfg80211_profile* prof = &ifp->vif->profile;
  zx_status_t status = ZX_OK;
  bcme_status_t fw_err = BCME_OK;

  struct brcmf_scb_val_le scbval;
  memcpy(&scbval.ea, prof->bssid, ETH_ALEN);
  scbval.val = static_cast<uint16_t>(fuchsia_wlan_ieee80211::ReasonCode::kStaLeaving);
  status = brcmf_fil_cmd_data_set(ifp, BRCMF_C_SCB_DEAUTHENTICATE_FOR_REASON, &scbval,
                                  sizeof(scbval), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("Failed to issue BRCMF_C_SCB_DEAUTHENTICATE_FOR_REASON to firmware: %s, fw err %s",
              zx_status_get_string(status), brcmf_fil_get_errstr(fw_err));
  }

  // Once a deauth has been sent, client is not connected.
  brcmf_clear_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state);
  status = brcmf_bss_reset(ifp);
  return status;
}

// Sync driver channel to match firmware channel.
static zx_status_t sync_driver_channel_to_firmware_channel(struct brcmf_if* ifp) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  zx_status_t status = ZX_OK;
  chanspec_t fw_chanspec;
  uint8_t fw_ctl_chan;
  status = brcmf_get_ctrl_channel(ifp, &fw_chanspec, &fw_ctl_chan);
  if (status != ZX_OK) {
    BRCMF_ERR(
        "Synchronizing driver channel to firmware channel impossible, channel lookup failed: %d",
        status);
    return status;
  }
  BRCMF_DBG(CONN, "Setting driver channel to chanspec 0x%x", fw_chanspec);
  cfg->channel = fw_chanspec;
  return status;
}

zx_status_t brcmf_update_bss_info(struct brcmf_if* ifp) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  struct brcmf_cfg80211_profile* prof = &ifp->vif->profile;
  bcme_status_t fw_err = BCME_OK;
  if (ifp->roam_req.has_value()) {
    cfg->capability = ifp->roam_req->selected_bss()->capability_info();

    ifp->connect_req.selected_bss().emplace(ifp->roam_req->selected_bss().value());

    brcmf_init_prof(prof);
    memcpy(&prof->bssid, ifp->roam_req->selected_bss()->bssid().data(), ETH_ALEN);
    prof->beacon_period = ifp->roam_req->selected_bss()->beacon_period();
    return ZX_OK;
  }

  if (!ifp->connect_req.selected_bss().has_value()) {
    BRCMF_ERR("Unexpected brcmf_update_bss_info() without valid connect_req.");
    return ZX_ERR_BAD_STATE;
  }

  // No roam request, so attempt to get the current BSS info from firmware.
  BRCMF_INFO("Getting current BSS info from firmware");
  // Firmware returns the BSS info data after a small offset.
  constexpr size_t kBssInfoOffset = 4;
  constexpr size_t kBssInfoBufLen = sizeof(brcmf_bss_info_le) + kBssInfoOffset;

  // TODO(b/519259848): Check that kBssInfoBufLen is correct here.
  // Is kBssInfoBufLen the correct length? It seems like we expect that there is BSS info data
  // after the brcmf_bss_info_le struct. But this will only read up to sizeof(brcmf_info_le_struct)
  // + 4, which means that it *won't* return any data after the brcmf_bss_info_le struct.
  //
  // But this path is only used for firmware-initiated roaming, which isn't enabled in prod right
  // now, and I don't have the means to test it. So I'm keeping this as-is for now until
  // firmware-initiated roaming is tested and enabled (if ever).
  const auto bss_info_status =
      brcmf_fil_cmd_data_get(ifp, BRCMF_C_GET_BSS_INFO, cfg->extra_buf, kBssInfoBufLen, &fw_err);
  if (bss_info_status != ZX_OK) {
    BRCMF_ERR("Could not get BSS info from firmware: %s, fw err %s",
              zx_status_get_string(bss_info_status), brcmf_fil_get_errstr(fw_err));
    return bss_info_status;
  }

  // Ignore any data before kBssInfoOffset.
  const auto bss_info_buf = cfg->extra_buf + kBssInfoOffset;
  auto bss_info = reinterpret_cast<brcmf_bss_info_le*>(bss_info_buf);

  // Copy info into relevant fields.
  cfg->capability = bss_info->capability;

  // Check that the IEs don't extend beyond the size of cfg->extra_buf. We don't accidentally want
  // to read data beyond cfg->extra_buf during the memcpy.
  constexpr size_t max_ie_buf_len = WL_EXTRA_BUF_MAX - kBssInfoOffset;
  if (bss_info->ie_offset > max_ie_buf_len ||
      bss_info->ie_length > max_ie_buf_len - bss_info->ie_offset) {
    BRCMF_ERR("Invalid BSS info: ie_offset %u, ie_length %u exceed buffer size %d",
              bss_info->ie_offset, bss_info->ie_length, WL_EXTRA_BUF_MAX);
    return ZX_ERR_INVALID_ARGS;
  }

  // TODO(b/519259848): Should this also update selected_bss().bssid()?
  // It's possible that reconnects after a firmware-initiated roam don't work because the BSSID
  // isn't updated.
  ifp->connect_req.selected_bss()->ies().resize(bss_info->ie_length);
  const uint8_t* ie_ptr = bss_info_buf + bss_info->ie_offset;
  memcpy(ifp->connect_req.selected_bss()->ies().data(), ie_ptr, bss_info->ie_length);

  brcmf_init_prof(prof);
  memcpy(&prof->bssid, &bss_info->BSSID, ETH_ALEN);
  prof->beacon_period = bss_info->beacon_period;
  return ZX_OK;
}

static bool brcmf_roaming_offload_enabled(struct brcmf_if* ifp) {
  return brcmf_feat_is_enabled(ifp, BRCMF_FEAT_ROAM_ENGINE) ||
         brcmf_feat_is_enabled(ifp, BRCMF_FEAT_WNM_BTM);
}

static zx_status_t brcmf_bss_roam_done(brcmf_if* ifp, brcmf_connect_status_t connect_status,
                                       fuchsia_wlan_ieee80211_wire::StatusCode status_code) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  struct net_device* ndev = ifp->ndev;
  BRCMF_DBG(TRACE, "Enter");

  if (brcmf_test_and_clear_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    cfg->roam_timer->Stop();
    brcmf_log_conn_status(ifp, connect_status);
    switch (connect_status) {
      case brcmf_connect_status_t::CONNECTED: {
        brcmf_get_assoc_ies(cfg, ifp);
        const auto sync_channel_status = sync_driver_channel_to_firmware_channel(ifp);
        const auto update_bss_info_status = brcmf_update_bss_info(ifp);
        // Here we check for various internal errors, any of which will trigger a disconnect.
        if (sync_channel_status != ZX_OK || update_bss_info_status != ZX_OK ||
            !cfg->target_bssid.has_value()) {
          BRCMF_ERR(
              "Firmware reported roam success but driver encountered an internal error, need to reset firmware state.");
          const zx_status_t err = brcmf_clear_firmware_connection_state(ifp);
          if (err != ZX_OK) {
            BRCMF_ERR("Failed to clear firmware connection state.");
          }
          status_code = fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified;
          break;
        }
        brcmf_set_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state);
        ifp->connection_id += 1;
        BRCMF_INFO("Roam succeeded");
        if (!brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
          // Start the signal report timer
          cfg->connect_log_cnt = 0;
          cfg->signal_report_timer->Start(BRCMF_SIGNAL_REPORT_TIMER_DUR_MS);
          // Indicate the rssi soon after connection
          cfg80211_signal_ind(ndev);
        }
        break;
      }
      case brcmf_connect_status_t::ROAM_INTERRUPTED: {
        BRCMF_INFO("Reassociation failed because roam attempt was interrupted by SME.");
        // SME has already issued the disconnect, so we just need to reset the interface.
        const auto err = brcmf_bss_reset(ifp);
        if (err != ZX_OK) {
          BRCMF_ERR("Failed to reset connection state.");
        }
        break;
      }
      case brcmf_connect_status_t::AUTHENTICATION_FAILED: {
        // Authentication step(s) failed, so reassociation could not proceed.
        cfg->target_bss_authenticated = false;
        __FALLTHROUGH;
      }
      case brcmf_connect_status_t::REASSOC_REQ_FAILED: {
        // Reassociation request step failed.
        __FALLTHROUGH;
      }
      case brcmf_connect_status_t::INTERNAL_ERROR: {
        // Driver or firmware internal error caused roam to fail, e.g. firmware command could not be
        // issued or firmware event contents were malformed.
        __FALLTHROUGH;
      }
      case brcmf_connect_status_t::CONNECTING_TIMEOUT: {
        // Roam attempt timeout reached.
        __FALLTHROUGH;
      }
      default: {
        BRCMF_WARN("Reassociation failed with connect_status %s, status_code %d",
                   brcmf_get_connect_status_str(connect_status),
                   static_cast<int>(fidl::ToUnderlying(status_code)));
        BRCMF_INFO("Reassociation failed, need to reset firmware state.");
        const zx_status_t err = brcmf_clear_firmware_connection_state(ifp);
        if (err != ZX_OK) {
          BRCMF_ERR("Failed to clear firmware connection state.");
        }
      }
    }
    // This is a safeguard against a driver internal error in the roam failure path. At this point,
    // either cfg->target_bssid is set (as expected), or a disconnect has been initiated.
    if (!cfg->target_bssid.has_value()) {
      BRCMF_ERR("Missing target BSSID, cannot notify SME of roam result/conf");

      // Roam cannot proceed at this point.
      clear_roam_attempt(ifp);
      return ZX_ERR_INTERNAL;
    }
    const auto target_bssid = static_cast<uint8_t*>(cfg->target_bssid->data());

    if (brcmf_roaming_offload_enabled(ifp)) {
      // Make sure we have sent roam start to SME before sending result. Some roam attempts fail
      // without generating a ROAM_PREP event, and when that happens no roam start has been sent.
      if (!cfg->roam_start_sent) {
        brcmf_return_roam_start(ndev);
      }

      brcmf_return_roam_result(ndev, target_bssid, status_code);
      // Roam is done, so reset roam_start_sent.
      cfg->roam_start_sent = false;
    } else {
      if (!ifp->roam_req.has_value()) {
        BRCMF_ERR("Missing roam request, cannot notify SME of roam conf");
        clear_roam_attempt(ifp);
        return ZX_ERR_INTERNAL;
      }
      brcmf_return_roam_conf(ndev, target_bssid, status_code);
    }

    // Roam result/conf has been sent, so we can reset fields that were needed during the attempt.
    clear_roam_attempt(ifp);
  }

  BRCMF_DBG(TRACE, "Exit");
  return ZX_OK;
}

// Issue firmware command for MLME-initiated roam attempt. If this handler returns error status,
// caller must send RoamConf to MLME.
zx_status_t brcmf_cfg80211_roam(struct net_device* ndev) {
  zx_status_t status = ZX_OK;
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  bcme_status_t fw_status = BCME_OK;
  chanspec_t chspec = INVCHANSPEC;

  BRCMF_INFO("MLME-initiated roam requested");
  if (!ifp->roam_req.has_value() || ifp->roam_req->IsEmpty()) {
    BRCMF_ERR(
        "Roam attempt failed due to internal error: MLME requested roam, but driver does not have a stored roam request");
    return ZX_ERR_INTERNAL;
  }

  wl_reassoc_params_t reassoc_params;
  reassoc_params.bssid_cnt = 0;

  memcpy(&reassoc_params.bssid, ifp->roam_req->selected_bss()->bssid().data(), ETH_ALEN);

  // TODO(https://fxbug.dev/42144507) - Remove this override, it is only for logging.
  const auto& selected_bss = ifp->roam_req->selected_bss().value();
  const auto old_chanspec = channel_to_chanspec(&cfg->d11inf, selected_bss.primary().number(),
                                                selected_bss.primary().band(),
                                                fuchsia_wlan_ieee80211::ChannelBandwidth::kCbw20);

  const auto chanspec_result = channel_to_chanspec(
      &cfg->d11inf, ifp->roam_req->selected_bss()->primary().number(),
      ifp->roam_req->selected_bss()->primary().band(), ifp->roam_req->selected_bss()->bandwidth());
  if (chanspec_result.is_ok()) {
    chspec = chanspec_result.value();
  } else {
    BRCMF_WARN("Roam chanspec conversion error: %s", chanspec_result.status_string());
    if (old_chanspec.is_error()) {
      BRCMF_ERR("Roam failed due to chanspec conversion errors");
      return chanspec_result.error_value();
    }
    BRCMF_WARN("Roam using legacy chanspec 0x%x", old_chanspec.value());
    chspec = old_chanspec.value();
  }

  reassoc_params.chanspec_num = 1;
  reassoc_params.chanspec_list[0] = chspec;
  cfg->channel = chspec;

  status = brcmf_fil_cmd_data_set(ifp, BRCMF_C_REASSOC, &reassoc_params, sizeof(reassoc_params),
                                  &fw_status);
  if (status != ZX_OK) {
    if (chanspec_result.is_ok() && old_chanspec.is_ok() &&
        chanspec_result.value() != old_chanspec.value()) {
      BRCMF_WARN("Used new chanspec 0x%x instead of old chanspec 0x%x", chanspec_result.value(),
                 old_chanspec.value());
    }
    BRCMF_ERR("Roam failed due to firmware REASSOC command failure, firmware status: %s",
              zx_status_get_string(fw_status));
  }
  return status;
}

void brcmf_if_roam_req(net_device* ndev,
                       const fuchsia_wlan_fullmac_wire::WlanFullmacImplRoamRequest* req) {
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  struct brcmf_cfg80211_vif* vif = ndev_to_vif(ndev);
  brcmf_connect_status_t connect_status;

  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping connect request");
    return;
  }

  if (brcmf_is_apmode(ifp->vif)) {
    BRCMF_ERR("Received RoamReq but not in client mode - ignoring");
    return;
  }

  if (!req->has_selected_bss()) {
    BRCMF_ERR("Missing required field, selected_bss: %d", req->has_selected_bss());
    return;
  }

  // Max size of WlanFullmacImplRoamRequest.
  constexpr size_t kWlanFullmacImplRoamRequestBufferSize =
      fidl::MaxSizeInChannel<fuchsia_wlan_fullmac::wire::WlanFullmacImplRoamRequest,
                             fidl::MessageDirection::kSending>();
  fidl::Arena<kWlanFullmacImplRoamRequestBufferSize> arena;
  fidl::Array<uint8_t, ETH_ALEN> selected_bssid;
  memcpy(selected_bssid.data(), req->selected_bss().bssid.data(), ETH_ALEN);
  auto conf = fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcRoamConfRequest::Builder(arena)
                  .status_code(fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified)
                  .selected_bssid(selected_bssid);

  auto ssid =
      brcmf_find_ssid_in_ies(req->selected_bss().ies.data(), req->selected_bss().ies.size());

  if (ssid.empty()) {
    BRCMF_DBG(WLANIF, "Roam request from SME exited: no SSID in BSS description");
    connect_status = brcmf_connect_status_t::REASSOC_REQ_FAILED;
    conf.status_code(fuchsia_wlan_ieee80211_wire::StatusCode::kJoinFailure);
    goto fail;
  }

  // Saving the request as FIDL natural type.
  // Note that below this point, `req` and `ifp->roam_req` refer to the same roam request and
  // are equivalent.
  ifp->roam_req = fidl::ToNatural(*req);
  cfg->target_bssid = ifp->roam_req->selected_bss()->bssid();

  brcmf_set_bit(brcmf_vif_status_bit_t::ROAMING, &vif->sme_state);

  // Any ongoing scan is canceled to prevent interference with the roam attempt.
  if (brcmf_test_bit(brcmf_scan_status_bit_t::BUSY, &cfg->scan_status)) {
    std::string scan_status_report;
    brcmf_check_scan_status(cfg->scan_status.load(), &scan_status_report);
    BRCMF_DBG(CONN, "Incoming roam request during scan_status %s", scan_status_report.c_str());
    BRCMF_INFO("Aborting scan due to incoming roam request.");
    brcmf_abort_scanning(cfg);
  }
  if (brcmf_cfg80211_roam(ndev) != ZX_OK) {
    // Firmware reassoc command could not be sent.
    connect_status = brcmf_connect_status_t::INTERNAL_ERROR;
    conf.status_code(fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedExternalReason);
    goto fail;
  }
  return;

fail:
  brcmf_bss_roam_done(ifp, connect_status,
                      fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified);
}

zx_status_t brcmf_if_sae_handshake_resp(
    net_device* ndev,
    const fuchsia_wlan_fullmac_wire::WlanFullmacImplSaeHandshakeRespRequest* resp) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  bcme_status_t fw_err = BCME_OK;
  zx_status_t status = ZX_OK;

  if (!resp || !resp->has_peer_sta_address() || !resp->has_status_code()) {
    if (!resp) {
      BRCMF_ERR("Invalid arguments, resp is nullptr");
    } else {
      BRCMF_ERR("Invalid arguments, has_peer_sta_address: %u has_status_code: %u.",
                resp->has_peer_sta_address(), resp->has_status_code());
    }
    if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
      brcmf_bss_roam_done(ifp, brcmf_connect_status_t::AUTHENTICATION_FAILED,
                          fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedExternalReason);
      return ZX_ERR_INVALID_ARGS;
    }
    brcmf_return_assoc_result(ndev,
                              fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedExternalReason);
    return ZX_ERR_INVALID_ARGS;
  }

  if (memcmp(resp->peer_sta_address().data(), ifp->connect_req.selected_bss()->bssid().data(),
             ETH_ALEN)) {
    BRCMF_ERR("Auth MAC != Join MAC");
#if !defined(NDEBUG)
    const uint8_t* old_mac = ifp->connect_req.selected_bss()->bssid().data();
    const uint8_t* new_mac = resp->peer_sta_address().data();
    BRCMF_DBG(CONN, " auth mac: " FMT_MAC ", join mac: " FMT_MAC, FMT_MAC_ARGS(new_mac),
              FMT_MAC_ARGS(old_mac));
#endif /* !defined(NDEBUG) */
    // Just in case, in debug builds, we should investigate why the MLME is giving us inconsistent
    // requests.
    ZX_DEBUG_ASSERT(0);

    // In release builds, ignore and continue.
    BRCMF_ERR("Ignoring mismatch and using join MAC address");
  }

  auto ssid = brcmf_find_ssid_in_ies(ifp->connect_req.selected_bss()->ies().data(),
                                     ifp->connect_req.selected_bss()->ies().size());
  if (ssid.empty()) {
    BRCMF_ERR("No SSID IE in BSS");
    if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
      brcmf_bss_roam_done(ifp, brcmf_connect_status_t::AUTHENTICATION_FAILED,
                          fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified);
    } else {
      brcmf_return_assoc_result(ndev,
                                fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified);
    }
  }

  brcmf_clear_bit(brcmf_vif_status_bit_t::SAE_AUTHENTICATING, &ifp->vif->sme_state);

  // Issue assoc_mgr_cmd to resume firmware from waiting for the success of SAE authentication.
  assoc_mgr_cmd_t cmd;
  cmd.version = ASSOC_MGR_CURRENT_VERSION;
  cmd.length = sizeof(cmd);
  cmd.cmd = ASSOC_MGR_CMD_PAUSE_ON_EVT;
  cmd.params = ASSOC_MGR_PARAMS_EVENT_NONE;

  status = brcmf_fil_iovar_data_set(ifp, "assoc_mgr_cmd", &cmd, sizeof(cmd), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("Set iovar assoc_mgr_cmd fail. err: %s, fw_err: %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
      // Note: ignoring the return value from brcmf_bss_roam_done, will use `status` instead.
      brcmf_bss_roam_done(ifp, brcmf_connect_status_t::AUTHENTICATION_FAILED,
                          fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedExternalReason);
    } else {
      brcmf_return_assoc_result(ndev,
                                fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified);
    }
  }

  // If roaming, note that SAE has succeeded for the target BSS.
  if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    cfg->target_bss_authenticated = true;
  }

  return status;
}

zx_status_t brcmf_if_sae_frame_tx(net_device* ndev,
                                  const fuchsia_wlan_fullmac_wire::SaeFrame* frame) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  bcme_status_t fw_err = BCME_OK;
  zx_status_t err = ZX_OK;

  if (!frame->has_status_code() || !frame->has_peer_sta_address() || !frame->has_sae_fields() ||
      !frame->has_seq_num()) {
    BRCMF_ERR(
        "SaeFrameTx missing fields: has_status_code=%u, has_peer_sta_address=%u, has_sae_fields=%u, has_seq_num=%u",
        frame->has_status_code(), frame->has_peer_sta_address(), frame->has_sae_fields(),
        frame->has_seq_num());
    return ZX_ERR_INVALID_ARGS;
  }

  // Mac header(24 bytes) + Auth frame header(6 bytes) + sae_fields length.
  uint32_t frame_size =
      sizeof(wlan::MgmtFrameHeader) + sizeof(wlan::Authentication) + frame->sae_fields().size();
  // Carry the SAE authentication frame in the last field of assoc_mgr_cmd.
  uint32_t cmd_buf_len = sizeof(assoc_mgr_cmd_t) + frame_size;
  uint8_t cmd_buf[cmd_buf_len];
  assoc_mgr_cmd_t* cmd = reinterpret_cast<assoc_mgr_cmd_t*>(cmd_buf);
  cmd->version = ASSOC_MGR_CURRENT_VERSION;
  // As the description of "length" field in this structure, it should be used to store the length
  // of the entire structure, here is a special case where we store the length of the frame here.
  // After confirming with vendor, this is the way they deal with extra data for this iovar, the
  // value of "length" field should be the length of extra data.
  cmd->length = frame_size;
  cmd->cmd = ASSOC_MGR_CMD_SEND_AUTH;

  auto sae_frame =
      reinterpret_cast<brcmf_sae_auth_frame*>(cmd_buf + offsetof(assoc_mgr_cmd_t, params));

  // Set MAC addresses in MAC header, firmware will check these parts, and fill other missing parts.
  sae_frame->mac_hdr.addr1 = MacAddr(frame->peer_sta_address().data());  // DA
  sae_frame->mac_hdr.addr2 = MacAddr(ifp->mac_addr);                     // SA
  sae_frame->mac_hdr.addr3 = MacAddr(frame->peer_sta_address().data());  // BSSID

  BRCMF_DBG(CONN,
            "The peer_sta_address: " FMT_MAC ", the ifp mac is: " FMT_MAC
            ", the seq_num is %u, the status_code is %hu",
            FMT_MAC_ARGS(frame->peer_sta_address()), FMT_MAC_ARGS(ifp->mac_addr), frame->seq_num(),
            fidl::ToUnderlying(frame->status_code()));

  // Fill the authentication frame header fields.
  sae_frame->auth_hdr.auth_algorithm_number = BRCMF_AUTH_MODE_SAE;
  sae_frame->auth_hdr.auth_txn_seq_number = frame->seq_num();
  sae_frame->auth_hdr.status_code = static_cast<uint16_t>(frame->status_code());

  BRCMF_DBG(CONN, "auth_algorithm_number: %u, auth_txn_seq_number: %u, status_code: %u",
            sae_frame->auth_hdr.auth_algorithm_number, sae_frame->auth_hdr.auth_txn_seq_number,
            sae_frame->auth_hdr.status_code);

  // Attach SAE payload after authentication frame header.
  memcpy(sae_frame->sae_payload, frame->sae_fields().data(), frame->sae_fields().size());

  err = brcmf_fil_iovar_data_set(ifp, "assoc_mgr_cmd", cmd_buf, cmd_buf_len, &fw_err);
  if (err != ZX_OK) {
    BRCMF_ERR("Error sending SAE auth frame. err: %s, fw_err: %s", zx_status_get_string(err),
              brcmf_fil_get_errstr(fw_err));
    brcmf_return_assoc_result(
        ndev, fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedUnauthenticatedAccessNotSupported);
  }

  return err;
}

zx_status_t brcmf_if_set_multicast_promisc(net_device* ndev, bool enable) {
  BRCMF_IFDBG(WLANIF, ndev, "%s promiscuous mode", enable ? "Enabling" : "Disabling");
  ndev->multicast_promisc = enable;
  brcmf_netdev_set_allmulti(ndev);
  return ZX_OK;
}

static void brcmf_if_convert_ac_param(
    const edcf_acparam_t* acparam,
    fuchsia_wlan_driver::wire::WlanWmmAccessCategoryParameters* out_ac_params) {
  out_ac_params->aifsn = acparam->aci & EDCF_AIFSN_MASK;
  out_ac_params->acm = (acparam->aci & EDCF_ACM_MASK) != 0;
  out_ac_params->ecw_min = acparam->ecw & EDCF_ECWMIN_MASK;
  out_ac_params->ecw_max = (acparam->ecw & EDCF_ECWMAX_MASK) >> EDCF_ECWMAX_SHIFT;
  out_ac_params->txop_limit = acparam->txop;
}

void brcmf_if_wmm_status_req(net_device* ndev) {
  zx_status_t status = ZX_OK;
  bcme_status_t fw_err = BCME_OK;
  edcf_acparam_t ac_params[AC_COUNT];
  fuchsia_wlan_driver::wire::WlanWmmParameters resp = {};
  uint32_t wme_bss_disable;
  brcmf_if* ifp = ndev_to_if(ndev);

  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- ignoring wmm status req");
    return;
  }
  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return;
  }

  if (ifp == nullptr) {
    BRCMF_ERR("ifp is null");
    auto result = ndev->if_proto.buffer(*arena)->OnWmmStatusResp(ZX_ERR_INTERNAL, resp);
    if (!result.ok()) {
      BRCMF_ERR("Failed to send wmm status resp result.status: %s", result.status_string());
    }
    return;
  }
  // Retrieve the value of iovar wme_bss_disable. If the iovar is not present or
  // it returns a non-zero value, indicate the error to SME.
  status = brcmf_fil_bsscfg_int_get(ifp, "wme_bss_disable", &wme_bss_disable);
  if ((status != ZX_OK) || wme_bss_disable) {
    if (status != ZX_OK) {
      BRCMF_ERR("get wme_bss_disable error (%d)", status);
    } else {
      status = ZX_ERR_NOT_SUPPORTED;
    }
    auto result = ndev->if_proto.buffer(*arena)->OnWmmStatusResp(status, resp);
    if (!result.ok()) {
      BRCMF_ERR("Failed to send wmm status resp result.status: %s", result.status_string());
    }
    return;
  }

  status = brcmf_fil_iovar_data_get(ifp, "wme_ac_sta", &ac_params, sizeof(ac_params), &fw_err);
  // TODO(https://fxbug.dev/42146778): Check what happens when WMM is not enabled.
  if (status != ZX_OK) {
    BRCMF_ERR("could not get STA WMM status: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    auto result = ndev->if_proto.buffer(*arena)->OnWmmStatusResp(status, resp);
    if (!result.ok()) {
      BRCMF_ERR("Failed to send wmm status resp result.status: %s", result.status_string());
    }
    return;
  }

  uint32_t apsd = 0;
  status = brcmf_fil_iovar_data_get(ifp, "wme_apsd", &apsd, sizeof(apsd), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("could not get WMM APSD: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    auto result = ndev->if_proto.buffer(*arena)->OnWmmStatusResp(status, resp);
    if (!result.ok()) {
      BRCMF_ERR("Failed to send wmm status resp result.status: %s", result.status_string());
    }
    return;
  }

  resp.apsd = apsd != 0;
  brcmf_if_convert_ac_param(&ac_params[AC_BE], &resp.ac_be_params);
  brcmf_if_convert_ac_param(&ac_params[AC_BK], &resp.ac_bk_params);
  brcmf_if_convert_ac_param(&ac_params[AC_VI], &resp.ac_vi_params);
  brcmf_if_convert_ac_param(&ac_params[AC_VO], &resp.ac_vo_params);
  auto result = ndev->if_proto.buffer(*arena)->OnWmmStatusResp(status, resp);
  if (!result.ok()) {
    BRCMF_ERR("Failed to send wmm status resp result.status: %s", result.status_string());
  }
}

zx_status_t brcmf_alloc_vif(struct brcmf_cfg80211_info* cfg, fuchsia_wlan_common::WlanMacRole type,
                            struct brcmf_cfg80211_vif** vif_out) {
  struct brcmf_cfg80211_vif* vif_walk;
  struct brcmf_cfg80211_vif* vif;
  bool mbss;

  BRCMF_DBG(TRACE, "allocating virtual interface (size=%zu)", sizeof(*vif));
  vif = static_cast<decltype(vif)>(calloc(1, sizeof(*vif)));
  if (!vif) {
    if (vif_out) {
      *vif_out = nullptr;
    }
    return ZX_ERR_NO_MEMORY;
  }

  vif->wdev.iftype = type;
  vif->saved_ie.assoc_req_ie_len = 0;

  brcmf_init_prof(&vif->profile);

  if (type == fuchsia_wlan_common::WlanMacRole::kAp) {
    mbss = false;
    list_for_every_entry (&cfg->vif_list, vif_walk, struct brcmf_cfg80211_vif, list) {
      if (vif_walk->wdev.iftype == fuchsia_wlan_common::WlanMacRole::kAp) {
        mbss = true;
        break;
      }
    }
    vif->mbss = mbss;
  }

  list_add_tail(&cfg->vif_list, &vif->list);
  if (vif_out) {
    *vif_out = vif;
  }
  return ZX_OK;
}

void brcmf_free_vif(struct brcmf_cfg80211_vif* vif) {
  if (!vif) {
    return;
  }
  if (list_in_list(&vif->list)) {
    list_delete(&vif->list);
  }
  free(vif);
}

void brcmf_free_net_device_vif(struct net_device* ndev) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  if (ifp && ifp->vif) {
    brcmf_free_vif(ifp->vif);
    ifp->vif = nullptr;
  }
}

// Returns true if client is connected (also includes CONNECTING, ROAMING, and DISCONNECTING).
static bool brcmf_is_client_connected(brcmf_if* ifp) {
  if (!ifp || !ifp->vif) {
    return false;
  }
  return (brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state) ||
          brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state) ||
          brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state) ||
          brcmf_test_bit(brcmf_vif_status_bit_t::DISCONNECTING, &ifp->vif->sme_state));
}

static const char* brcmf_get_client_connect_state_string(brcmf_if* ifp) {
  if (!ifp || !ifp->vif) {
    return "Not connected";
  }
  if (brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state)) {
    return "Connected";
  }
  if (brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state)) {
    return "Connecting";
  }
  if (brcmf_test_bit(brcmf_vif_status_bit_t::DISCONNECTING, &ifp->vif->sme_state)) {
    return "Disconnecting";
  }
  return "Not connected";
}

// Notify SME of channel switch
zx_status_t brcmf_notify_channel_switch(struct brcmf_if* ifp, const struct brcmf_event_msg* e,
                                        void* data) {
  if (!ifp) {
    return ZX_ERR_INVALID_ARGS;
  }
  struct net_device* ndev = ifp->ndev;
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping channel switch callback");
    return ZX_ERR_INVALID_ARGS;
  }

  uint16_t chanspec = 0;
  uint8_t ctl_chan;
  fuchsia_wlan_fullmac_wire::WlanFullmacChannelSwitchInfo info = {};
  zx_status_t err = ZX_OK;
  struct brcmf_cfg80211_info* cfg = nullptr;
  struct wireless_dev* wdev = nullptr;

  if (e != nullptr) {
    BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });
  }

  if (!ifp->vif) {
    BRCMF_ERR("Null virtual interface during channel switch");
    return ZX_ERR_BAD_STATE;
  }

  cfg = ifp->drvr->config;
  wdev = ndev_to_wdev(ndev);

  // For client IF, ensure it is connected.
  if (wdev->iftype == fuchsia_wlan_common::WlanMacRole::kClient) {
    // Status should be connected.
    if (!brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state)) {
      BRCMF_ERR("CSA on %s. Not associated.", ndev->name);
      return ZX_ERR_BAD_STATE;
    }
  }
  if ((err = brcmf_get_ctrl_channel(ifp, &chanspec, &ctl_chan)) != ZX_OK) {
    return err;
  }
  BRCMF_DBG(CONN, "Channel switch ind IF: %d chanspec: 0x%x control channel: %d", ifp->ifidx,
            chanspec, ctl_chan);
  info.new_primary_channel = chanspec_to_primary_channel_number(&cfg->d11inf, chanspec);
  info.bandwidth = chanspec_to_channel_bandwidth(&cfg->d11inf, chanspec);
  info.vht_secondary_80_channel = chanspec_to_secondary80(&cfg->d11inf, chanspec);

  // Inform wlanif of the channel switch.
  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return ZX_ERR_INTERNAL;
  }
  auto result = ndev->if_proto.buffer(*arena)->OnChannelSwitch(info);
  if (!result.ok()) {
    BRCMF_ERR("Failed to send channel switch info result.status: %s", result.status_string());
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

static zx_status_t brcmf_notify_ap_started(struct brcmf_if* ifp, const struct brcmf_event_msg* e,
                                           void* data) {
  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });
  return brcmf_notify_channel_switch(ifp, e, data);
}

static zx_status_t brcmf_notify_start_auth(struct brcmf_if* ifp, const struct brcmf_event_msg* e,
                                           void* data) {
  struct net_device* ndev = ifp->ndev;
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping SAE auth start notifications.");
    return ZX_ERR_BAD_HANDLE;
  }
  assoc_mgr_cmd_t cmd;
  zx_status_t err = ZX_OK;
  bcme_status_t fw_err = BCME_OK;
  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return ZX_ERR_INTERNAL;
  }

  auto builder =
      fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcSaeHandshakeIndRequest::Builder(*arena);
  brcmf_ext_auth* auth_start_evt = (brcmf_ext_auth*)data;

  if (!(brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state) ||
        brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state))) {
    BRCMF_INFO("Received AUTH event when we are not even connecting to an AP.");
    return ZX_ERR_BAD_STATE;
  }

  BRCMF_DBG(EVENT,
            "The peer addr received from data is: " FMT_MAC ", the addr in event_msg is: " FMT_MAC
            "\n",
            FMT_MAC_ARGS(auth_start_evt->bssid), FMT_MAC_ARGS(e->addr));

  fidl::Array<uint8_t, ETH_ALEN> peer_sta_address;
  std::copy(std::begin(auth_start_evt->bssid), std::end(auth_start_evt->bssid),
            peer_sta_address.begin());
  builder.peer_sta_address(peer_sta_address);

  // SAE four-way authentication start.
  brcmf_set_bit(brcmf_vif_status_bit_t::SAE_AUTHENTICATING, &ifp->vif->sme_state);

  // Issue assoc_mgr_cmd to update the the state machine of firmware, so that the firmware will wait
  // for SAE frame from external supplicant.
  cmd.version = ASSOC_MGR_CURRENT_VERSION;
  cmd.length = sizeof(cmd);
  cmd.cmd = ASSOC_MGR_CMD_PAUSE_ON_EVT;
  cmd.params = ASSOC_MGR_PARAMS_PAUSE_EVENT_AUTH_RESP;
  err = brcmf_fil_iovar_data_set(ifp, "assoc_mgr_cmd", &cmd, sizeof(cmd), &fw_err);
  if (err != ZX_OK) {
    BRCMF_ERR("Set assoc_mgr_cmd fail. err: %s, fw_err: %s", zx_status_get_string(err),
              brcmf_fil_get_errstr(fw_err));
    return err;
  }

  auto result = ndev->if_proto.buffer(*arena)->SaeHandshakeInd(builder.Build());
  if (!result.ok()) {
    BRCMF_ERR("Failed to send sae handshake ind result.status: %s", result.status_string());
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

static zx_status_t brcmf_rx_auth_frame(struct brcmf_if* ifp, const uint32_t datalen, void* data) {
  struct net_device* ndev = ifp->ndev;
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping SAE auth frame receive handler.");
    return ZX_ERR_BAD_HANDLE;
  }

  auto pframe = (uint8_t*)data;
  auto pframe_hdr = reinterpret_cast<wlan::Authentication*>(pframe);

  BRCMF_DBG(TRACE, "Receive SAE authentication frame.");
  BRCMF_DBG(CONN, "SAE authentication frame: ");
  BRCMF_DBG(CONN, " status code: %u", pframe_hdr->status_code);
  BRCMF_DBG(CONN, " sequence number: %u", pframe_hdr->auth_txn_seq_number);

  auto arena = fdf::Arena::Create(0, 0);
  // Sending SAE authentication up to SME, not rx from SME.
  if (arena.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
    return ZX_ERR_INTERNAL;
  }

  // Copy authentication frame header information.
  fidl::Array<uint8_t, ETH_ALEN> peer_sta_address;
  memcpy(peer_sta_address.data(), ifp->connect_req.selected_bss()->bssid().data(), ETH_ALEN);

  auto frame =
      fuchsia_wlan_fullmac_wire::SaeFrame::Builder(*arena)
          .peer_sta_address(peer_sta_address)
          .status_code(
              static_cast<fuchsia_wlan_ieee80211_wire::StatusCode>(pframe_hdr->status_code))
          .seq_num(pframe_hdr->auth_txn_seq_number)
          .sae_fields(::fidl::VectorView<uint8_t>::FromExternal(
              pframe + sizeof(wlan::Authentication), datalen - sizeof(wlan::Authentication)))
          .Build();

  auto result = ndev->if_proto.buffer(*arena)->SaeFrameRx(frame);
  if (!result.ok()) {
    BRCMF_ERR("Failed to send sae frame rx result.status: %s", result.status_string());
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

static zx_status_t brcmf_bss_connect_done(brcmf_if* ifp, brcmf_connect_status_t connect_status,
                                          fuchsia_wlan_ieee80211_wire::StatusCode assoc_result) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  struct net_device* ndev = ifp->ndev;
  BRCMF_DBG(TRACE, "Enter");

  if (brcmf_test_and_clear_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state)) {
    // Stop connect timer no matter connect success or not, this timer only times out when nothing
    // is heard from firmware.
    cfg->connect_timer->Stop();
    brcmf_log_conn_status(ifp, connect_status);

    switch (connect_status) {
      case brcmf_connect_status_t::CONNECTED: {
        brcmf_get_assoc_ies(cfg, ifp);
        brcmf_set_bit(brcmf_vif_status_bit_t::CONNECTED, &ifp->vif->sme_state);
        ifp->connection_id += 1;
        if (!brcmf_feat_is_enabled(ifp, BRCMF_FEAT_MFG)) {
          // Start the signal report timer
          cfg->connect_log_cnt = 0;
          cfg->signal_report_timer->Start(BRCMF_SIGNAL_REPORT_TIMER_DUR_MS);
          // Indicate the rssi soon after connection
          cfg80211_signal_ind(ndev);
        }
        assoc_result = fuchsia_wlan_ieee80211_wire::StatusCode::kSuccess;
        break;
      }
      case brcmf_connect_status_t::ASSOC_REQ_FAILED: {
        BRCMF_INFO("Association is rejected, need to reset firmware state.");
        const zx_status_t err = brcmf_clear_firmware_connection_state(ifp);
        if (err != ZX_OK) {
          BRCMF_ERR("Failed to clear firmware connection state.");
        }
        break;
      }
      default: {
        BRCMF_WARN("Unsuccessful connection: connect_status %s, assoc_result %d",
                   brcmf_get_connect_status_str(connect_status),
                   static_cast<int>(fidl::ToUnderlying(assoc_result)));
        const zx_status_t err = brcmf_clear_firmware_connection_state(ifp);
        if (err != ZX_OK) {
          BRCMF_ERR("Failed to clear firmware connection state.");
        }
        break;
      }
    }
    brcmf_return_assoc_result(ndev, assoc_result);
  }

  BRCMF_DBG(TRACE, "Exit");
  return ZX_OK;
}

static void brcmf_connect_timeout_worker(WorkItem* work) {
  struct brcmf_cfg80211_info* cfg =
      containerof(work, struct brcmf_cfg80211_info, connect_timeout_work);
  struct brcmf_if* ifp = cfg_to_if(cfg);
  BRCMF_WARN(
      "Connection timeout, clearing firmware connection state, and sending "
      "assoc result to SME.");
  zx_status_t err = brcmf_clear_firmware_connection_state(ifp);
  if (err != ZX_OK) {
    BRCMF_ERR("Failed to clear firmware connection state.");
  }
  // In case the timeout happens in SAE process.
  brcmf_clear_bit(brcmf_vif_status_bit_t::SAE_AUTHENTICATING, &ifp->vif->sme_state);
  brcmf_bss_connect_done(ifp, brcmf_connect_status_t::CONNECTING_TIMEOUT,
                         fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified);
}

static zx_status_t brcmf_indicate_client_connect(struct brcmf_if* ifp,
                                                 const struct brcmf_event_msg* e, void* data) {
  zx_status_t status = ZX_OK;

  BRCMF_DBG(TRACE, "Enter\n");
  BRCMF_DBG(CONN, "Connect Event %d, status %s reason %d auth %s flags 0x%x\n", e->event_code,
            brcmf_fweh_get_event_status_str(e->status), e->reason,
            brcmf_fweh_get_auth_type_str(e->auth_type), e->flags);
  BRCMF_DBG(CONN, "Linkup\n");

  if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    status = brcmf_bss_roam_done(ifp, brcmf_connect_status_t::CONNECTED,
                                 fuchsia_wlan_ieee80211_wire::StatusCode::kSuccess);
  } else {
    brcmf_bss_connect_done(ifp, brcmf_connect_status_t::CONNECTED,
                           fuchsia_wlan_ieee80211_wire::StatusCode::kSuccess);
  }
  brcmf_net_setcarrier(ifp, true);

  BRCMF_DBG(TRACE, "Exit\n");
  return status;
}

// Handler for ASSOC event (client only)
static zx_status_t brcmf_handle_assoc_event(struct brcmf_if* ifp, const struct brcmf_event_msg* e,
                                            void* data) {
  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });
  ZX_DEBUG_ASSERT(!brcmf_is_apmode(ifp->vif));

  // For this event, e->reason is in the fuchsia_wlan_ieee80211_wire::StatusCode enum space.
  fuchsia_wlan_ieee80211_wire::StatusCode reason_code =
      static_cast<fuchsia_wlan_ieee80211_wire::StatusCode>(e->reason);

  // Vendor confirmed the firmware can return reason_code 0 while status_code > 0. See
  // http://b/201803254#comment12. This is a design that they would like to not change in the
  // firmware.
  if ((BRCMF_E_STATUS_SUCCESS != e->status) &&
      (fuchsia_wlan_ieee80211_wire::StatusCode::kSuccess == reason_code)) {
    BRCMF_INFO(
        "Reason is SUCCESS(%u) while status indicates error: %u. Overriding reason to "
        "REFUSED_REASON_UNSPECIFIED(%u).",
        fidl::ToUnderlying(fuchsia_wlan_ieee80211_wire::StatusCode::kSuccess), e->status,
        fidl::ToUnderlying(fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified));
    reason_code = fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified;
  }

  return brcmf_bss_connect_done(ifp,
                                (e->status == BRCMF_E_STATUS_SUCCESS)
                                    ? brcmf_connect_status_t::CONNECTED
                                    : brcmf_connect_status_t::ASSOC_REQ_FAILED,
                                reason_code);
}

// Handler to ASSOC_IND and REASSOC_IND events. These are explicitly meant for SoftAP
static zx_status_t brcmf_handle_assoc_ind(struct brcmf_if* ifp, const struct brcmf_event_msg* e,
                                          void* data) {
  struct net_device* ndev = ifp->ndev;
  std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
  if (!ndev->if_proto.is_valid()) {
    BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping assoc ind callback");
    return ZX_OK;
  }

  BRCMF_DBG(EVENT, "IF: %d event %s (%u) status %s reason %d auth %s flags 0x%x", ifp->ifidx,
            brcmf_fweh_event_name(static_cast<brcmf_fweh_event_code>(e->event_code)), e->event_code,
            brcmf_fweh_get_event_status_str(e->status), e->reason,
            brcmf_fweh_get_auth_type_str(e->auth_type), e->flags);
  ZX_DEBUG_ASSERT(brcmf_is_apmode(ifp->vif));

  if (e->reason != BRCMF_E_STATUS_SUCCESS) {
    return ZX_OK;
  }

  if (data == nullptr || e->datalen == 0) {
    BRCMF_ERR("Received ASSOC_IND with no IEs");
    return ZX_ERR_INVALID_ARGS;
  }

  const struct brcmf_tlv* ssid_ie = brcmf_parse_tlvs(data, e->datalen, WLAN_IE_TYPE_SSID);
  if (ssid_ie == nullptr) {
    BRCMF_ERR("Received ASSOC_IND with no SSID IE");
    return ZX_ERR_INVALID_ARGS;
  }

  if (ssid_ie->len > fuchsia_wlan_ieee80211::kMaxSsidByteLen) {
    BRCMF_ERR("Received ASSOC_IND with invalid SSID IE");
    return ZX_ERR_INVALID_ARGS;
  }

  const struct brcmf_tlv* rsn_ie = brcmf_parse_tlvs(data, e->datalen, WLAN_IE_TYPE_RSNE);
  if (rsn_ie && rsn_ie->len > fuchsia::wlan::ieee80211::WLAN_IE_BODY_MAX_LEN) {
    BRCMF_ERR("Received ASSOC_IND with invalid RSN IE");
    return ZX_ERR_INVALID_ARGS;
  }

  // Create arena before populating vectors
  zx::result<fdf::Arena> arena_result = fdf::Arena::Create(0, 0);
  if (arena_result.is_error()) {
    BRCMF_ERR("Failed to create Arena status=%s", arena_result.status_string());
    return ZX_ERR_INTERNAL;
  }
  fdf::Arena& arena = arena_result.value();
  fidl::Array<uint8_t, ETH_ALEN> peer_sta_address;
  memcpy(peer_sta_address.data(), e->addr, ETH_ALEN);

  // Unfortunately, we have to ask the firmware to provide the associated station's
  // listen interval.
  struct brcmf_sta_info_le sta_info;
  std::vector<uint8_t> ssid{};
  if (brcmf_cfg80211_get_station(ndev, peer_sta_address.data(), &sta_info) == ZX_OK) {
    // Extract the SSID from the IEs
    ssid.resize(ssid_ie->len);
    memcpy(ssid.data(), ssid_ie->data, ssid_ie->len);
  }

  // Extract the RSN information from the IEs
  std::vector<uint8_t> rsne{};
  if (rsn_ie != nullptr) {
    size_t rsn_len = rsn_ie->len + TLV_HDR_LEN;
    const uint8_t* rsn_ie_ptr = reinterpret_cast<const uint8_t*>(rsn_ie);
    cpp20::span<const uint8_t> rsne_span = {rsn_ie_ptr, rsn_len};
    if (rsne_span.size() <= fuchsia_wlan_ieee80211_wire::kWlanIeBodyMaxLen) {
      rsne.assign(rsne_span.begin(), rsne_span.end());
    } else {
      BRCMF_ERR("Received ASSOC_IND with invalid RSN IE length %zu", rsne_span.size());
      brcmf_cfg80211_del_station(ndev, peer_sta_address.data(),
                                 fuchsia_wlan_ieee80211::ReasonCode::kInvalidRsneCapabilities);
      return ZX_OK;
    }
  }
  auto assoc_ind_builder =
      fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcAssocIndRequest::Builder(arena)
          .peer_sta_address(peer_sta_address)
          .listen_interval(sta_info.listen_interval_inms / ifp->vif->profile.beacon_period)
          .ssid(ssid)
          .rsne(rsne)
          .Build();

  BRCMF_IFDBG(WLANIF, ndev, "Sending assoc indication to SME.");
#if !defined(NDEBUG)
  BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC "", FMT_MAC_ARGS(peer_sta_address.data()));
#endif /* !defined(NDEBUG) */

  auto result = ndev->if_proto.buffer(arena)->AssocInd(assoc_ind_builder);
  if (!result.ok()) {
    BRCMF_ERR("Failed to send assoc ind  result.status: %s", result.status_string());
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

static void brcmf_roam_timeout_worker(WorkItem* work) {
  struct brcmf_cfg80211_info* cfg =
      containerof(work, struct brcmf_cfg80211_info, roam_timeout_work);
  struct brcmf_if* ifp = cfg_to_if(cfg);

  BRCMF_WARN("Roam timeout");
  brcmf_bss_roam_done(ifp, brcmf_connect_status_t::CONNECTING_TIMEOUT,
                      fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified);
}

// Some status codes correspond to authentication failure; others (like join failure) imply it.
static bool status_code_is_authentication_failure(
    fuchsia_wlan_ieee80211_wire::StatusCode status_code) {
  switch (status_code) {
    // Join failure is not an authentication failure, but authentication cannot succeed without
    // a successful join.
    case fuchsia_wlan_ieee80211_wire::StatusCode::kJoinFailure:
      __FALLTHROUGH;
    case fuchsia_wlan_ieee80211_wire::StatusCode::kUnsupportedAuthAlgorithm:
      __FALLTHROUGH;
    case fuchsia_wlan_ieee80211_wire::StatusCode::kTransactionSequenceError:
      __FALLTHROUGH;
    case fuchsia_wlan_ieee80211_wire::StatusCode::kChallengeFailure:
      __FALLTHROUGH;
    case fuchsia_wlan_ieee80211_wire::StatusCode::kRejectedSequenceTimeout:
      __FALLTHROUGH;
    case fuchsia_wlan_ieee80211_wire::StatusCode::kAntiCloggingTokenRequired:
      __FALLTHROUGH;
    case fuchsia_wlan_ieee80211_wire::StatusCode::kUnsupportedFiniteCyclicGroup:
      __FALLTHROUGH;
    case fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedUnauthenticatedAccessNotSupported:
      return true;
    default:
      return false;
  }
}

// A roam begins with a REASSOC command (or a REASSOC event, if firmware-initiated).
// Here's a rough guide to the firmware events that occur in a successful roam:
// - REASSOC event, status NEWASSOC; or, for firmware-initiated, status ATTEMPT
// - ROAM_PREP event, status SUCCESS
// - AUTH event(s), which differ depending on ESS security config
// - REASSOC event, status SUCCESS
// - LINK event, status SUCCESS
// - only if firmware-initiated: ROAM event, status SUCCESS
static zx_status_t brcmf_handle_reassoc_event(struct brcmf_if* ifp, const struct brcmf_event_msg* e,
                                              void* data) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  struct net_device* ndev = ifp->ndev;
  struct wireless_dev* wdev = ndev_to_wdev(ndev);
  struct brcmf_cfg80211_vif* vif = containerof(wdev, struct brcmf_cfg80211_vif, wdev);
  if (!check_vif_up(vif)) {
    return ZX_ERR_IO;
  }

  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });
  if (brcmf_is_apmode(ifp->vif)) {
    BRCMF_ERR("Unexpected REASSOC event received for AP interface");
    return ZX_ERR_INTERNAL;
  }
  ZX_DEBUG_ASSERT(!brcmf_is_apmode(ifp->vif));

  // Ignore REASSOC events unless roaming is expected.
  if (!brcmf_roaming_offload_enabled(ifp) && !ifp->roam_req.has_value()) {
    BRCMF_WARN("Ignoring REASSOC (%s) event because reassociation/roaming is not expected",
               brcmf_fweh_get_event_status_str(e->status));
    return ZX_ERR_NOT_SUPPORTED;
  }

  if (e->status == BRCMF_E_STATUS_ATTEMPT || e->status == BRCMF_E_STATUS_NEWASSOC) {
    BRCMF_DBG(CONN, "REASSOC event: attempting roam to " FMT_MAC, FMT_MAC_ARGS(e->addr));
    std::array<uint8_t, ETH_ALEN> target_bssid;
    memcpy(&target_bssid, e->addr, ETH_ALEN);
    cfg->target_bssid = target_bssid;
    cfg->roam_timer->Start(BRCMF_ROAM_TIMER_DUR);
    // TODO(https://fxbug.dev/356393937) Suppress SoftAP start/stop during roam.
    brcmf_set_bit(brcmf_vif_status_bit_t::ROAMING, &vif->sme_state);
    // TODO(https://fxbug.dev/42068734) Firmware-initiated REASSOC fails if scan overlaps.
    // Note: canceling in-progress scan here does not prevent the firmware-initiated REASSOC event
    // failure. The roam timer will timeout and cleanup if an overlapping scan causes the roam to
    // fail.
    if (brcmf_test_bit(brcmf_scan_status_bit_t::BUSY, &cfg->scan_status)) {
      std::string scan_status_report;
      brcmf_check_scan_status(cfg->scan_status.load(), &scan_status_report);
      BRCMF_DBG(CONN, "REASSOC event incoming during scan_status %s", scan_status_report.c_str());
      BRCMF_INFO("Aborting scan due to in-progress roam attempt.");
      brcmf_abort_scanning(cfg);
    }
  } else if (e->status == BRCMF_E_STATUS_SUCCESS) {
    BRCMF_DBG(CONN, "REASSOC event: success");
    // REASSOC success is the clearest signal that authentication succeeded.
    cfg->target_bss_authenticated = true;
  } else {
    BRCMF_DBG(CONN, "REASSOC event: failure, status %s",
              brcmf_fweh_get_event_status_str(e->status));
    // Reassociation failed, so roam will not succeed, and we may not see further roam-related
    // events. For this event, e->reason is in the StatusCode enum space.
    const fuchsia_wlan_ieee80211_wire::StatusCode reason_code =
        static_cast<fuchsia_wlan_ieee80211_wire::StatusCode>(e->reason);

    const auto connect_status = status_code_is_authentication_failure(reason_code)
                                    ? brcmf_connect_status_t::AUTHENTICATION_FAILED
                                    : brcmf_connect_status_t::REASSOC_REQ_FAILED;
    return brcmf_bss_roam_done(ifp, connect_status, reason_code);
  }
  return ZX_OK;
}

// Prints extra information about auth events, based on information obtained from the vendor.
static void brcmf_print_auth_event_details(const struct brcmf_event_msg* e) {
  switch (e->status) {
    case BRCMF_E_STATUS_SUCCESS:
      // Nothing to see here
      break;
    case BRCMF_E_STATUS_NO_ACK:
      // Vendor reported the timeout is 300ms within firmware and is not configurable.
      // Typical acks happen in approx 1ms, so this timeout should be more than long enough.
      BRCMF_WARN("No MAC-level ack received for auth request within 300ms firmware timeout");
      break;
    case BRCMF_E_STATUS_TIMEOUT:
      // Vendor reported the timeout is 300ms within firmware and is not configurable. Note
      // this timeout is separate from the timeout for BRCMF_E_STATUS_NO_ACK.
      BRCMF_WARN("No auth response received from AP within 300ms firmware timeout");
      break;
    case BRCMF_E_STATUS_UNSOLICITED:
      // Vendor reported this is an unexpected Ack response from AP, or an out-of-sequence auth
      // response.
      BRCMF_WARN("Received an unexpected or out-of-sequence auth response from AP");
      break;
    case BRCMF_E_STATUS_FAIL:
      // Vendor informed us that when e->event_code is 16, the e->reason corresponds to 802.11
      // deauth reason codes.
      if (e->event_code == 16) {
        BRCMF_WARN("AP rejected auth attempt, reference reason code against 802.11 deauth reasons");
      } else {
        BRCMF_ERR(
            "Unexpected event code for BRCMF_E_STATUS_FAIL, follow up with vendor for more "
            "information");
      }
      break;
    case BRCMF_E_STATUS_ABORT:
      // This status occurs very rarely and transiently. We haven't confirmed the meaning with
      // the vendor, but it's not a cause for concern (see https://fxbug.dev/42052783#c17).
      break;
    default:
      BRCMF_ERR("Unexpected status, follow up with vendor for more information");
  }
}

// Handler for AUTH event (client only)
static zx_status_t brcmf_process_auth_event(struct brcmf_if* ifp, const struct brcmf_event_msg* e,
                                            void* data) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return static_cast<int>(reason); });

  ZX_DEBUG_ASSERT(!brcmf_is_apmode(ifp->vif));

  if (e->status != BRCMF_E_STATUS_SUCCESS) {
    BRCMF_INFO("Auth Failure auth %s status %s reason %d flags 0x%x event %lu",
               brcmf_fweh_get_auth_type_str(e->auth_type),
               brcmf_fweh_get_event_status_str(e->status), static_cast<int>(e->reason), e->flags,
               static_cast<unsigned long>(e->event_code));
    brcmf_print_auth_event_details(e);

    // It appears FW continues to be busy with authentication when this event is received
    // specifically with WEP. Attempt to shutdown the IF.
    brcmf_bss_reset(ifp);

    if (brcmf_test_bit(brcmf_vif_status_bit_t::SAE_AUTHENTICATING, &ifp->vif->sme_state)) {
      // Issue assoc_mgr_cmd to resume firmware from waiting for the success of SAE authentication.
      bcme_status_t fwerr = BCME_OK;
      zx_status_t status;
      assoc_mgr_cmd_t cmd;

      cmd.version = ASSOC_MGR_CURRENT_VERSION;
      cmd.length = sizeof(cmd);
      cmd.cmd = ASSOC_MGR_CMD_PAUSE_ON_EVT;
      cmd.params = ASSOC_MGR_PARAMS_EVENT_NONE;

      status = brcmf_fil_iovar_data_set(ifp, "assoc_mgr_cmd", &cmd, sizeof(cmd), &fwerr);
      if (status != ZX_OK) {
        // An error will always be returned here until the firmware bug is fixed.
        // TODO(zhiyichen): Remove the comment once the firmware bug is fixed.
        BRCMF_ERR("Set iovar assoc_mgr_cmd fail. err: %s, fw_err: %s", zx_status_get_string(status),
                  brcmf_fil_get_errstr(fwerr));
      }
      brcmf_clear_bit(brcmf_vif_status_bit_t::SAE_AUTHENTICATING, &ifp->vif->sme_state);
    }
    if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
      brcmf_bss_roam_done(
          ifp, brcmf_connect_status_t::AUTHENTICATION_FAILED,
          fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedUnauthenticatedAccessNotSupported);
    } else {
      brcmf_bss_connect_done(
          ifp, brcmf_connect_status_t::AUTHENTICATION_FAILED,
          fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedUnauthenticatedAccessNotSupported);
    }
  }

  // Only care about the authentication frames during SAE process.
  if (brcmf_test_bit(brcmf_vif_status_bit_t::SAE_AUTHENTICATING, &ifp->vif->sme_state) &&
      e->datalen > 0) {
    BRCMF_INFO("SAE frame received from driver.");
    return brcmf_rx_auth_frame(ifp, e->datalen, data);
  }

  if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    // For these auth types, event success means target BSS is authenticated.
    if (e->auth_type == BRCMF_AUTH_MODE_OPEN || e->auth_type == BRCMF_AUTH_MODE_SHARED_KEY) {
      cfg->target_bss_authenticated = true;
    }
  }

  return ZX_OK;
}

// AUTH_IND handler. AUTH_IND is meant only for SoftAP IF
static zx_status_t brcmf_process_auth_ind_event(struct brcmf_if* ifp,
                                                const struct brcmf_event_msg* e, void* data) {
  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });
  ZX_DEBUG_ASSERT(brcmf_is_apmode(ifp->vif));

  if (e->reason == BRCMF_E_STATUS_SUCCESS) {
    struct net_device* ndev = ifp->ndev;
    std::shared_lock<std::shared_mutex> guard(ndev->if_proto_lock);
    if (!ndev->if_proto.is_valid()) {
      BRCMF_IFDBG(WLANIF, ndev, "interface stopped -- skipping auth ind callback");
      return ZX_OK;
    }
    auto arena = fdf::Arena::Create(0, 0);
    if (arena.is_error()) {
      BRCMF_ERR("Failed to create Arena status=%s", arena.status_string());
      return ZX_ERR_INTERNAL;
    }
    fidl::Array<uint8_t, ETH_ALEN> peer_sta_address;
    memcpy(peer_sta_address.data(), e->addr, ETH_ALEN);
    auto auth_ind_builder =
        fuchsia_wlan_fullmac_wire::WlanFullmacImplIfcAuthIndRequest::Builder(*arena)
            // We always authenticate as an open system for WPA
            .auth_type(fuchsia_wlan_fullmac_wire::WlanAuthType::kOpenSystem)
            .peer_sta_address(peer_sta_address)
            .Build();

    BRCMF_IFDBG(WLANIF, ndev, "Sending auth indication to SME. type: open");
#if !defined(NDEBUG)
    BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC, FMT_MAC_ARGS(peer_sta_address.data()));
#endif /* !defined(NDEBUG) */

    auto result = ndev->if_proto.buffer(*arena)->AuthInd(auth_ind_builder);
    if (!result.ok()) {
      BRCMF_ERR("Failed to send auth ind result.status: %s", result.status_string());
      return ZX_ERR_INTERNAL;
    }
  }
  return ZX_OK;
}

static void brcmf_indicate_no_network(struct brcmf_if* ifp) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  BRCMF_DBG(CONN, "No network\n");
  if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    brcmf_bss_roam_done(ifp, brcmf_connect_status_t::NO_NETWORK,
                        fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedExternalReason);
  } else {
    brcmf_bss_connect_done(ifp, brcmf_connect_status_t::NO_NETWORK,
                           fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedExternalReason);
  }
  brcmf_disconnect_done(cfg);
}

static zx_status_t brcmf_indicate_client_disconnect(struct brcmf_if* ifp,
                                                    const struct brcmf_event_msg* e, void* data,
                                                    brcmf_connect_status_t connect_status) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  struct net_device* ndev = ifp->ndev;
  zx_status_t status = ZX_OK;

  BRCMF_DBG(TRACE, "Enter\n");
  if (!brcmf_is_client_connected(ifp)) {
    // Client is already disconnected.
    return status;
  }
  // Start of disconnect process. Reset disconnect_done.
  sync_completion_reset(&ifp->disconnect_done);

  // TODO(https://fxbug.dev/42139586): Remove once this verbose logging is no longer needed in
  // brcmf_indicate_client_disconnect(). This log should be moved to CONN
  // for production code.
  BRCMF_INFO("client disconnect indicated. state %s, rssi, %d snr, %d",
             brcmf_get_client_connect_state_string(ifp), ndev->last_known_rssi_dbm,
             ndev->last_known_snr_db);
  BRCMF_INFO_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });
  if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    // Disconnect happened during a roam attempt, so report that the roam failed.
    brcmf_bss_roam_done(ifp, brcmf_connect_status_t::ROAM_INTERRUPTED,
                        fuchsia_wlan_ieee80211_wire::StatusCode::kCanceled);
  } else if (brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state)) {
    // Default assoc_result for disconnections.
    auto assoc_result = fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified;
    // Some connect_status values get a specific assoc_result.
    if (connect_status == brcmf_connect_status_t::AUTHENTICATION_FAILED) {
      assoc_result =
          fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedUnauthenticatedAccessNotSupported;
    } else if (connect_status == brcmf_connect_status_t::NO_NETWORK) {
      assoc_result = fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedExternalReason;
    }
    brcmf_bss_connect_done(ifp, connect_status, assoc_result);
  } else {
    brcmf_set_bit(brcmf_vif_status_bit_t::DISCONNECTING, &ifp->vif->sme_state);
  }

  fuchsia_wlan_ieee80211::ReasonCode reason_code =
      (connect_status == brcmf_connect_status_t::LINK_FAILED)
          ? fuchsia_wlan_ieee80211::ReasonCode::kMlmeLinkFailed
          : static_cast<fuchsia_wlan_ieee80211::ReasonCode>(e->reason);
  brcmf_disconnect_done(cfg);
  brcmf_link_down(ifp->vif, reason_code, e->event_code, e->addr);
  brcmf_clear_profile_on_client_disconnect(ndev_to_prof(ndev));
  if (ndev != cfg_to_ndev(cfg)) {
    sync_completion_signal(&cfg->vif_disabled);
  }
  brcmf_net_setcarrier(ifp, false);
  // Signal completion of disconnect process.
  BRCMF_DBG(CONN, "Indicate disconnect done");
  sync_completion_signal(&ifp->disconnect_done);
  BRCMF_DBG(TRACE, "Exit\n");
  return status;
}

// Check firmware connection state, typically to compare firmware state to driver state.
static bool is_firmware_connected(struct brcmf_if* ifp) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  bcme_status_t fw_err = BCME_OK;
  uint8_t fw_bssid[ETH_ALEN];
  const auto get_bssid_status =
      brcmf_fil_cmd_data_get(ifp, BRCMF_C_GET_BSSID, &fw_bssid, ETH_ALEN, &fw_err);
  if (get_bssid_status != ZX_OK) {
    if (fw_err == BCME_NOTASSOCIATED) {
      return false;
    }
    BRCMF_ERR("Could not get current BSSID from firmware: %s, fw err %s",
              zx_status_get_string(get_bssid_status), brcmf_fil_get_errstr(fw_err));
    return false;
  }
  if (is_current_bss(cfg, fw_bssid) || is_target_bss(cfg, fw_bssid)) {
    return true;
  }
  return false;
}

static zx_status_t brcmf_process_link_event(struct brcmf_if* ifp, const struct brcmf_event_msg* e,
                                            void* data) {
  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });
  if (brcmf_is_apmode(ifp->vif)) {
    struct net_device* ndev = ifp->ndev;
    struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

    // TODO(karthikrish): Confirm with vendor if flags is indeed a bitmask.
    if (!(e->flags & BRCMF_EVENT_MSG_LINK)) {
      BRCMF_DBG(CONN, "AP mode link down\n");
      sync_completion_signal(&cfg->vif_disabled);
      return ZX_OK;
    }
    BRCMF_DBG(CONN, "AP mode link up\n");
    struct brcmf_if* ifp = ndev_to_if(ndev);

    // Indicate status only if AP is in start pending state (could have been cleared if
    // a stop request comes in before this event is received).
    if (brcmf_test_and_clear_bit(brcmf_vif_status_bit_t::AP_START_PENDING, &ifp->vif->sme_state)) {
      // Stop the timer when we get a result from firmware.
      cfg->ap_start_timer->Stop();
      // confirm AP Start
      brcmf_if_start_conf(ndev, fuchsia_wlan_fullmac_wire::StartResult::kSuccess);
      // Set AP_CREATED
      brcmf_set_bit(brcmf_vif_status_bit_t::AP_CREATED, &ifp->vif->sme_state);
    }
  } else {
    BRCMF_DBG(CONN, "Client mode link event.");
    if (e->status == BRCMF_E_STATUS_SUCCESS && (e->flags & BRCMF_EVENT_MSG_LINK)) {
      return brcmf_indicate_client_connect(ifp, e, data);
    }
    if (!brcmf_is_client_connected(ifp)) {
      // Client is already disconnected.
      BRCMF_DBG(CONN, "LINK down but client is already disconnected.");
    }
    if (!(e->flags & BRCMF_EVENT_MSG_LINK)) {
      return brcmf_indicate_client_disconnect(ifp, e, data, brcmf_connect_status_t::LINK_FAILED);
    }
    if (e->status == BRCMF_E_STATUS_NO_NETWORKS) {
      brcmf_indicate_no_network(ifp);
    }
  }
  return ZX_OK;
}

static zx_status_t brcmf_process_deauth_ind_event(struct brcmf_if* ifp,
                                                  const struct brcmf_event_msg* e, void* data) {
  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });

  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  brcmf_proto_delete_peer(ifp->drvr, ifp->ifidx, (uint8_t*)e->addr);
  if (brcmf_is_apmode(ifp->vif)) {
    brcmf_notify_deauth_ind(ifp->ndev, e->addr,
                            static_cast<fuchsia_wlan_ieee80211::ReasonCode>(e->reason), false);
    return ZX_OK;
  }

  // Sometimes FW sends E_DEAUTH when a unicast packet is received before association
  // is complete. Ignore it. We are not sure if this is true for E_DEAUTH_IND as well,
  // but this is the logic we have had for quite some time now.
  if (brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state) &&
      e->reason == BRCMF_E_REASON_UCAST_FROM_UNASSOC_STA) {
    BRCMF_DBG(EVENT, "E_DEAUTH_IND because data rcvd before assoc...ignore");
    return ZX_OK;
  }

  const auto& peer_sta_address = e->addr;

#if !defined(NDEBUG)
  struct net_device* ndev = cfg_to_ndev(cfg);
  if (is_current_bss(cfg, peer_sta_address)) {
    BRCMF_IFDBG(WLANIF, ndev, "Received deauth indication from current BSS: " FMT_MAC "",
                FMT_MAC_ARGS(peer_sta_address));
  } else if (is_target_bss(cfg, peer_sta_address)) {
    BRCMF_IFDBG(WLANIF, ndev, "Received deauth indication from target BSS: " FMT_MAC "",
                FMT_MAC_ARGS(peer_sta_address));
  }
#endif /* !defined(NDEBUG) */

  bool is_disconnect = false;
  // While roaming, a deauth_ind from the target BSS is a roam failure, which currently means a
  // disconnect too.
  if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    is_disconnect = is_target_bss(cfg, peer_sta_address);
  } else {
    is_disconnect = is_current_bss(cfg, peer_sta_address);
  }
  if (is_disconnect) {
    return brcmf_indicate_client_disconnect(ifp, e, data, brcmf_connect_status_t::DEAUTHENTICATING);
  }
  if (!is_firmware_connected(ifp)) {
    BRCMF_WARN("Firmware not connected, received deauth ind from unexpected BSS");
    return brcmf_indicate_client_disconnect(ifp, e, data, brcmf_connect_status_t::DEAUTHENTICATING);
  }
  BRCMF_INFO("Ignoring deauth indication from unexpected BSS");
#if !defined(NDEBUG)
  BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC "", FMT_MAC_ARGS(peer_sta_address));
#endif /* !defined(NDEBUG) */
  return ZX_OK;
}

static zx_status_t brcmf_process_deauth_event(struct brcmf_if* ifp, const struct brcmf_event_msg* e,
                                              void* data) {
  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });

  brcmf_proto_delete_peer(ifp->drvr, ifp->ifidx, (uint8_t*)e->addr);
  if (brcmf_is_apmode(ifp->vif)) {
    brcmf_notify_deauth(ifp->ndev, e->addr);
    return ZX_OK;
  }

  // Sometimes FW sends E_DEAUTH when a unicast packet is received before association
  // is complete. Ignore it.
  if (brcmf_test_bit(brcmf_vif_status_bit_t::CONNECTING, &ifp->vif->sme_state) &&
      e->reason == BRCMF_E_REASON_UCAST_FROM_UNASSOC_STA) {
    BRCMF_DBG(EVENT, "E_DEAUTH because data rcvd before assoc...ignore");
    return ZX_OK;
  }

#if !defined(NDEBUG)
  struct brcmf_cfg80211_profile* prof = &ifp->vif->profile;
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  struct net_device* ndev = cfg_to_ndev(cfg);
  const auto& peer_sta_address = e->addr;
  if (memcmp(prof->bssid, peer_sta_address, ETH_ALEN) == 0) {
    BRCMF_IFDBG(WLANIF, ndev, "Received deauth event for current BSS: " FMT_MAC "",
                FMT_MAC_ARGS(peer_sta_address));
  } else if (cfg->target_bssid.has_value() &&
             memcmp(peer_sta_address, cfg->target_bssid->data(), ETH_ALEN) == 0) {
    BRCMF_IFDBG(WLANIF, ndev, "Received deauth event for target BSS: " FMT_MAC "",
                FMT_MAC_ARGS(peer_sta_address));
  } else {
    BRCMF_IFDBG(WLANIF, ndev, "Received deauth event for unexpected BSS: " FMT_MAC "",
                FMT_MAC_ARGS(peer_sta_address));
  }
#endif /* !defined(NDEBUG) */

  // For now, a deauth event incurs a full disconnect. This may change in the future.
  return brcmf_indicate_client_disconnect(ifp, e, data, brcmf_connect_status_t::DEAUTHENTICATING);
}

static zx_status_t brcmf_process_disassoc_ind_event(struct brcmf_if* ifp,
                                                    const struct brcmf_event_msg* e, void* data) {
  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });

  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;

  brcmf_proto_delete_peer(ifp->drvr, ifp->ifidx, (uint8_t*)e->addr);
  if (brcmf_is_apmode(ifp->vif)) {
    brcmf_notify_disassoc_ind(ifp->ndev, e->addr,
                              static_cast<fuchsia_wlan_ieee80211::ReasonCode>(e->reason), false);
    return ZX_OK;
  }

  const auto& peer_sta_address = e->addr;
#if !defined(NDEBUG)
  struct net_device* ndev = cfg_to_ndev(cfg);
  if (is_current_bss(cfg, peer_sta_address)) {
    BRCMF_IFDBG(WLANIF, ndev, "Received disassoc indication from current BSS: " FMT_MAC "",
                FMT_MAC_ARGS(peer_sta_address));
  } else if (is_target_bss(cfg, peer_sta_address)) {
    BRCMF_IFDBG(WLANIF, ndev, "Received disassoc indication from target BSS: " FMT_MAC "",
                FMT_MAC_ARGS(peer_sta_address));
  }
#endif /* !defined(NDEBUG) */

  bool is_disconnect = false;
  // While roaming, a disassoc_ind from the target BSS is a roam failure, which currently means a
  // disconnect too.
  if (brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    is_disconnect = is_target_bss(cfg, peer_sta_address);
  } else {
    is_disconnect = is_current_bss(cfg, peer_sta_address);
  }
  if (is_disconnect) {
    return brcmf_indicate_client_disconnect(ifp, e, data, brcmf_connect_status_t::DISASSOCIATING);
  }
  if (!is_firmware_connected(ifp)) {
    BRCMF_WARN("Firmware not connected, received disassoc ind from unexpected BSS");
    return brcmf_indicate_client_disconnect(ifp, e, data, brcmf_connect_status_t::DISASSOCIATING);
  }
  BRCMF_INFO("Ignoring disassoc indication from unexpected BSS");
#if !defined(NDEBUG)
  BRCMF_IFDBG(WLANIF, ndev, "  address: " FMT_MAC "", FMT_MAC_ARGS(peer_sta_address));
#endif /* !defined(NDEBUG) */
  return ZX_OK;
}

static zx_status_t brcmf_process_disassoc_event(struct brcmf_if* ifp,
                                                const struct brcmf_event_msg* e, void* data) {
  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });

  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  brcmf_proto_delete_peer(ifp->drvr, ifp->ifidx, (uint8_t*)e->addr);
  if (brcmf_is_apmode(ifp->vif)) {
    brcmf_notify_disassoc(ifp->ndev, ZX_OK);
    return ZX_OK;
  }
  // For now, any disassoc event incurs a full disconnect. This may change in the future.
  auto connect_status = brcmf_connect_status_t::DISASSOCIATING;
  if (brcmf_test_bit(brcmf_disconnect_request_bit_t::DEAUTH_CURRENT_BSS,
                     &cfg->disconnect_request_state)) {
    connect_status = brcmf_connect_status_t::DEAUTHENTICATING;
  }
  return brcmf_indicate_client_disconnect(ifp, e, data, connect_status);
}

static zx_status_t brcmf_process_set_ssid_event(struct brcmf_if* ifp,
                                                const struct brcmf_event_msg* e, void* data) {
  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });

  if (e->status == BRCMF_E_STATUS_SUCCESS) {
    BRCMF_DBG(CONN, "set ssid success\n");
    memcpy(ifp->vif->profile.bssid, e->addr, ETH_ALEN);
  } else {
    BRCMF_DBG(CONN, "set ssid failed - no network found\n");
    brcmf_indicate_no_network(ifp);
  }
  return ZX_OK;
}

// Check whether a BSS info (brcmf_bss_info_le) has a well-formed IE buffer.
// This check is not meant to be exhaustive; it is intended to catch
// obviously invalid IE buffers (occasionally seen in data retrieved from
// firmware). Higher WLAN layers must perform their own IE validation.
static bool brcmf_bss_info_le_ie_buffer_well_formed(brcmf_bss_info_le* bi) {
  const auto& ies = reinterpret_cast<uint8_t*>(bi) + bi->ie_offset;
  const auto& ies_len = bi->ie_length;

  const auto ssid = brcmf_find_ssid_in_ies(ies, ies_len);
  if (ssid.empty()) {
    BRCMF_WARN("BSS description IE buffer does not contain SSID IE");
    return false;
  }

  size_t offset = 0;
  while (offset + TLV_HDR_LEN <= ies_len) {
    const auto elem_len = ies[offset + TLV_LEN_OFF];
    offset += TLV_HDR_LEN;
    if (offset + elem_len > ies_len) {
      break;
    }
    offset += elem_len;
  }
  if (offset != ies_len) {
    BRCMF_WARN("BSS description IE buffer sum of bytes (%ld) does not match IE buffer length %u",
               offset, ies_len);
    return false;
  }
  return true;
}

static bool brcmf_bssid_matches_bss_info(const uint8_t* bssid, brcmf_bss_info_le* bss_info) {
  const auto bss_info_bssid = cpp20::span{bss_info->BSSID};
  return memcmp(bssid, bss_info_bssid.data(), ETH_ALEN) == 0;
}

// Retrieve target BSS info from the firmware, storing it in the driver for later use.
static zx_status_t brcmf_get_target_bss_info(struct brcmf_if* ifp) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  bcme_status_t fw_err = BCME_OK;

  auto target_bss_info_status = brcmf_fil_iovar_data_get(
      ifp, "target_bss_info", cfg->target_bss_info_buf, WL_EXTRA_BUF_MAX, &fw_err);
  if (target_bss_info_status != ZX_OK) {
    BRCMF_ERR("Could not get target BSS info from firmware: %s, fw err %s",
              zx_status_get_string(target_bss_info_status), brcmf_fil_get_errstr(fw_err));
    return target_bss_info_status;
  }
  const auto& target_bss_info = reinterpret_cast<brcmf_bss_info_le*>(cfg->target_bss_info_buf);

  if (!brcmf_bss_info_le_ie_buffer_well_formed(target_bss_info)) {
    BRCMF_ERR(
        "target_bss_info firmware retrieval reported success, but IE buffer is not well-formed");
    return ZX_ERR_INTERNAL;
  }
  if (!cfg->target_bssid.has_value()) {
    BRCMF_ERR("Driver cfg has no target_bssid");
    return ZX_ERR_INTERNAL;
  }
  if (!brcmf_bssid_matches_bss_info(cfg->target_bssid->data(), target_bss_info)) {
    BRCMF_ERR("Driver cfg target_bssid does not match BSSID in target BSS info");
    return ZX_ERR_INTERNAL;
  }
  return target_bss_info_status;
}

// Gather info on an in-progress roam attempt and notify upper layers that the attempt has started.
// ROAM_PREP occurs just before the authentication to the target BSS begins.
static zx_status_t brcmf_notify_roam_prep_status(struct brcmf_if* ifp,
                                                 const struct brcmf_event_msg* e, void* data) {
  const brcmf_fweh_event_status_t event_status = e->status;
  zx_status_t status = ZX_OK;
  auto ndev = ifp->ndev;
  auto cfg = ifp->drvr->config;

  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });

  if (!brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    // Roam failure was already reported.
    return ZX_OK;
  }

  // ROAM_PREP occurs before attempt has started, so target BSS cannot be authenticated.
  cfg->target_bss_authenticated = false;
  switch (event_status) {
    case BRCMF_E_STATUS_ATTEMPT: {
      BRCMF_DBG(CONN, "ROAM_PREP event: attempt");
      if (!cfg->target_bssid.has_value()) {
        std::array<uint8_t, ETH_ALEN> target_bssid;
        memcpy(target_bssid.data(), e->addr, ETH_ALEN);
        cfg->target_bssid = target_bssid;
      }
      // Event status of attempt is otherwise ignored by the driver.
      return ZX_OK;
    }
    case BRCMF_E_STATUS_SUCCESS: {
      BRCMF_DBG(CONN, "ROAM_PREP event: success");
      if (!cfg->target_bssid.has_value()) {
        std::array<uint8_t, ETH_ALEN> target_bssid;
        memcpy(target_bssid.data(), e->addr, ETH_ALEN);
        cfg->target_bssid = target_bssid;
      }
      if (brcmf_roaming_offload_enabled(ifp)) {
        // Target BSS info must be available here. If not, roam cannot succeed.
        status = brcmf_get_target_bss_info(ifp);
      }
      break;
    }
    case BRCMF_E_STATUS_FAIL:
      __FALLTHROUGH;
    default: {
      // Interpret any other event status as failure.
      BRCMF_DBG(CONN, "Roam attempt failed with ROAM_PREP event status %s",
                brcmf_fweh_get_event_status_str(event_status));
      status = ZX_ERR_INTERNAL;
    }
  }

  if (brcmf_roaming_offload_enabled(ifp)) {
    // This is a Fullmac-initiated roam. Regardless of whether the ROAM_PREP event succeeded, we
    // must notify upper layers that a roam attempt started.
    brcmf_return_roam_start(ndev);
  }
  if (status != ZX_OK) {
    const auto connect_status = status == ZX_ERR_INTERNAL
                                    ? brcmf_connect_status_t::INTERNAL_ERROR
                                    : brcmf_connect_status_t::REASSOC_REQ_FAILED;
    return brcmf_bss_roam_done(ifp, connect_status,
                               fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified);
  }

  return status;
}

static zx_status_t brcmf_notify_roaming_status(struct brcmf_if* ifp,
                                               const struct brcmf_event_msg* e, void* data) {
  const brcmf_fweh_event_status_t status = e->status;

  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });

  if (!brcmf_test_bit(brcmf_vif_status_bit_t::ROAMING, &ifp->vif->sme_state)) {
    // Roam failure was already reported, or this is a spurious ROAM event.
    return ZX_OK;
  }

  switch (status) {
    case BRCMF_E_STATUS_ATTEMPT: {
      BRCMF_DBG(CONN, "ROAM event: attempt");
      // ROAM attempt is currently ignored by driver.
      return ZX_OK;
    }
    case BRCMF_E_STATUS_SUCCESS: {
      BRCMF_DBG(CONN, "ROAM event: success");
      // ROAM success is currently a no-op in the driver; LINK up handler performs the post-roam
      // actions.
      return ZX_OK;
    }
    case BRCMF_E_STATUS_NO_NETWORKS: {
      BRCMF_DBG(CONN, "ROAM event: NO_NETWORKS");
      __FALLTHROUGH;
    }
    case BRCMF_E_STATUS_FAIL: {
      BRCMF_DBG(CONN, "ROAM event: fail");
      __FALLTHROUGH;
    }
    default: {
      // Interpret any other event status as failure.
      BRCMF_WARN("Roam attempt failed with ROAM event status %s",
                 brcmf_fweh_get_event_status_str(status));
    }
  }

  const auto status_code = fuchsia_wlan_ieee80211_wire::StatusCode::kRefusedReasonUnspecified;
  const auto connect_status = brcmf_connect_status_t::REASSOC_REQ_FAILED;
  return brcmf_bss_roam_done(ifp, connect_status, status_code);
}

static zx_status_t brcmf_notify_mic_status(struct brcmf_if* ifp, const struct brcmf_event_msg* e,
                                           void* data) {
  uint16_t flags = e->flags;
  enum nl80211_key_type key_type;

  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });

  if (flags & BRCMF_EVENT_MSG_GROUP) {
    key_type = NL80211_KEYTYPE_GROUP;
  } else {
    key_type = NL80211_KEYTYPE_PAIRWISE;
  }

  cfg80211_michael_mic_failure(ifp->ndev, (uint8_t*)&e->addr, key_type, -1, nullptr);

  return ZX_OK;
}

static zx_status_t brcmf_notify_vif_event(struct brcmf_if* ifp, const struct brcmf_event_msg* e,
                                          void* data) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  if (cfg == nullptr) {
    return ZX_ERR_BAD_STATE;
  }
  struct brcmf_if_event* ifevent = (struct brcmf_if_event*)data;
  struct brcmf_cfg80211_vif_event* event = &cfg->vif_event;
  struct brcmf_cfg80211_vif* vif;

  BRCMF_DBG_EVENT(ifp, e, "%d", [](uint32_t reason) { return reason; });
  BRCMF_DBG(EVENT, "IF event: action %u flags %u ifidx %u bsscfgidx %u", ifevent->action,
            ifevent->flags, ifevent->ifidx, ifevent->bsscfgidx);

  mtx_lock(&event->vif_event_lock);
  event->action = ifevent->action;
  vif = event->vif;

  switch (ifevent->action) {
    case BRCMF_E_IF_ADD:
      /* waiting process may have timed out */
      if (!cfg->vif_event.vif) {
        mtx_unlock(&event->vif_event_lock);
        return ZX_ERR_SHOULD_WAIT;
      }

      ifp->vif = vif;
      vif->ifp = ifp;
      if (ifp->ndev) {
        vif->wdev.netdev = ifp->ndev;
      }
      mtx_unlock(&event->vif_event_lock);
      if (event->action == cfg->vif_event_pending_action) {
        sync_completion_signal(&event->vif_event_wait);
      }
      return ZX_OK;

    case BRCMF_E_IF_DEL:
      mtx_unlock(&event->vif_event_lock);
      /* event may not be upon user request */
      if (brcmf_cfg80211_vif_event_armed(cfg) && event->action == cfg->vif_event_pending_action) {
        sync_completion_signal(&event->vif_event_wait);
      }
      return ZX_OK;

    case BRCMF_E_IF_CHANGE:
      mtx_unlock(&event->vif_event_lock);
      if (event->action == cfg->vif_event_pending_action) {
        sync_completion_signal(&event->vif_event_wait);
      }
      return ZX_OK;

    default:
      mtx_unlock(&event->vif_event_lock);
      break;
  }
  return ZX_ERR_INVALID_ARGS;
}

static void brcmf_init_conf(struct brcmf_cfg80211_conf* conf) {
  conf->frag_threshold = static_cast<uint32_t>(-1);
  conf->rts_threshold = static_cast<uint32_t>(-1);
  conf->retry_short = static_cast<uint32_t>(-1);
  conf->retry_long = static_cast<uint32_t>(-1);
}

static void brcmf_register_event_handlers(struct brcmf_cfg80211_info* cfg) {
  brcmf_fweh_register(cfg->pub, BRCMF_E_LINK, brcmf_process_link_event);
  brcmf_fweh_register(cfg->pub, BRCMF_E_AUTH, brcmf_process_auth_event);
  brcmf_fweh_register(cfg->pub, BRCMF_E_AUTH_IND, brcmf_process_auth_ind_event);
  brcmf_fweh_register(cfg->pub, BRCMF_E_DEAUTH_IND, brcmf_process_deauth_ind_event);
  brcmf_fweh_register(cfg->pub, BRCMF_E_DEAUTH, brcmf_process_deauth_event);
  brcmf_fweh_register(cfg->pub, BRCMF_E_DISASSOC_IND, brcmf_process_disassoc_ind_event);
  brcmf_fweh_register(cfg->pub, BRCMF_E_DISASSOC, brcmf_process_disassoc_event);
  brcmf_fweh_register(cfg->pub, BRCMF_E_ASSOC, brcmf_handle_assoc_event);
  brcmf_fweh_register(cfg->pub, BRCMF_E_ASSOC_IND, brcmf_handle_assoc_ind);
  brcmf_fweh_register(cfg->pub, BRCMF_E_REASSOC_IND, brcmf_handle_assoc_ind);
  brcmf_fweh_register(cfg->pub, BRCMF_E_MIC_ERROR, brcmf_notify_mic_status);
  brcmf_fweh_register(cfg->pub, BRCMF_E_SET_SSID, brcmf_process_set_ssid_event);
  brcmf_fweh_register(cfg->pub, BRCMF_E_IF, brcmf_notify_vif_event);
  brcmf_fweh_register(cfg->pub, BRCMF_E_CSA_COMPLETE_IND, brcmf_notify_channel_switch);
  brcmf_fweh_register(cfg->pub, BRCMF_E_AP_STARTED, brcmf_notify_ap_started);
  brcmf_fweh_register(cfg->pub, BRCMF_E_JOIN_START, brcmf_notify_start_auth);
  brcmf_fweh_register(cfg->pub, BRCMF_E_REASSOC, brcmf_handle_reassoc_event);
  brcmf_fweh_register(cfg->pub, BRCMF_E_ROAM_PREP, brcmf_notify_roam_prep_status);
  brcmf_fweh_register(cfg->pub, BRCMF_E_ROAM, brcmf_notify_roaming_status);
}

static void brcmf_deinit_cfg_mem(struct brcmf_cfg80211_info* cfg) {
  // Delete (which also causes a stop) all timers first. Their callbacks may use the data being
  // free'd below so we must ensure they are not called after those free calls.
  delete cfg->disconnect_timer;
  delete cfg->roam_timer;
  delete cfg->escan_timer;
  delete cfg->signal_report_timer;
  delete cfg->ap_start_timer;
  delete cfg->connect_timer;
  free(cfg->conf);
  cfg->conf = nullptr;
  free(cfg->extra_buf);
  cfg->extra_buf = nullptr;
  free(cfg->wowl.nd);
  cfg->wowl.nd = nullptr;
  free(cfg->wowl.nd_info);
  cfg->wowl.nd_info = nullptr;
  free(cfg->target_bss_info_buf);
  cfg->target_bss_info_buf = nullptr;
}

static zx_status_t brcmf_init_cfg_mem(struct brcmf_cfg80211_info* cfg) {
  cfg->conf = static_cast<decltype(cfg->conf)>(calloc(1, sizeof(*cfg->conf)));
  if (!cfg->conf) {
    goto init_priv_mem_out;
  }
  cfg->extra_buf = static_cast<decltype(cfg->extra_buf)>(calloc(1, WL_EXTRA_BUF_MAX));
  if (!cfg->extra_buf) {
    goto init_priv_mem_out;
  }
  cfg->wowl.nd =
      static_cast<decltype(cfg->wowl.nd)>(calloc(1, sizeof(*cfg->wowl.nd) + sizeof(uint32_t)));
  if (!cfg->wowl.nd) {
    goto init_priv_mem_out;
  }
  cfg->wowl.nd_info = static_cast<decltype(cfg->wowl.nd_info)>(
      calloc(1, sizeof(*cfg->wowl.nd_info) + sizeof(struct cfg80211_wowlan_nd_match*)));
  if (!cfg->wowl.nd_info) {
    goto init_priv_mem_out;
  }
  cfg->target_bss_info_buf =
      static_cast<decltype(cfg->target_bss_info_buf)>(calloc(1, WL_EXTRA_BUF_MAX));
  if (!cfg->target_bss_info_buf) {
    goto init_priv_mem_out;
  }
  return ZX_OK;

init_priv_mem_out:
  brcmf_deinit_cfg_mem(cfg);

  return ZX_ERR_NO_MEMORY;
}

static zx_status_t brcmf_init_cfg(struct brcmf_cfg80211_info* cfg) {
  zx_status_t err = ZX_OK;
  async_dispatcher_t* dispatcher = cfg->pub->device->GetTimerDispatcher();

  cfg->scan_in_progress = false;
  cfg->dongle_up = false; /* dongle is not up yet */
  err = brcmf_init_cfg_mem(cfg);
  if (err != ZX_OK) {
    return err;
  }
  brcmf_register_event_handlers(cfg);
  mtx_init(&cfg->usr_sync, mtx_plain);
  brcmf_init_escan(cfg);
  brcmf_init_conf(cfg->conf);

  // Initialize the disconnect timer
  cfg->disconnect_timer =
      new Timer(dispatcher, [cfg] { brcmf_disconnect_timeout(cfg); }, Timer::Type::OneShot);
  cfg->disconnect_timeout_work = WorkItem(brcmf_disconnect_timeout_worker);
  // Initialize the signal report timer
  cfg->signal_report_timer =
      new Timer(dispatcher, [cfg] { brcmf_signal_report_timeout(cfg); }, Timer::Type::Periodic);
  cfg->signal_report_work = WorkItem(brcmf_signal_report_worker);
  // Initialize the ap start timer
  cfg->ap_start_timer =
      new Timer(dispatcher, [cfg] { brcmf_ap_start_timeout(cfg); }, Timer::Type::OneShot);
  cfg->ap_start_timeout_work = WorkItem(brcmf_ap_start_timeout_worker);
  // Initialize the connect timer
  cfg->connect_timer =
      new Timer(dispatcher, [cfg] { brcmf_connect_timeout(cfg); }, Timer::Type::OneShot);
  cfg->connect_timeout_work = WorkItem(brcmf_connect_timeout_worker);
  // Initialize the roam timer.
  cfg->roam_timer = new Timer(dispatcher, [cfg] { brcmf_roam_timeout(cfg); }, Timer::Type::OneShot);
  cfg->roam_timeout_work = WorkItem(brcmf_roam_timeout_worker);

  cfg->vif_disabled = {};
  return err;
}

static void brcmf_deinit_cfg(struct brcmf_cfg80211_info* cfg) {
  cfg->dongle_up = false; /* dongle down */
  brcmf_abort_scanning_immediately(cfg);
  brcmf_deinit_cfg_mem(cfg);
}

static void init_vif_event(struct brcmf_cfg80211_vif_event* event) {
  event->vif_event_wait = {};
  mtx_init(&event->vif_event_lock, mtx_plain);
}

// Setup roam engine firmware offload (if supported).
// Note: there is a separate function for configuring the offload.
static zx_status_t brcmf_setup_roam_engine(struct brcmf_if* ifp) {
  bcme_status_t fwerr;
  zx_status_t status = ZX_OK;
  ifp->drvr->settings->roam_engine_enabled = false;

  // Iovar is "roam_off" which makes enable 0, while disable is 1.
  enum {
    ROAM_ENGINE_ENABLED = 0,
    ROAM_ENGINE_DISABLED = 1,
  };
  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_ROAM_ENGINE)) {
    status = brcmf_fil_iovar_int_set(ifp, "roam_off", ROAM_ENGINE_ENABLED, &fwerr);
    if (status == ZX_OK) {
      BRCMF_INFO("Roam engine firmware offload is enabled");
      ifp->drvr->settings->roam_engine_enabled = true;
    } else {
      BRCMF_WARN("Could not enable roam engine offload, firmware error %s",
                 brcmf_fil_get_errstr(fwerr));
    }
  } else {
    status = brcmf_fil_iovar_int_set(ifp, "roam_off", ROAM_ENGINE_DISABLED, &fwerr);
    if (status == ZX_OK || status == ZX_ERR_NOT_SUPPORTED) {
      // Note: if iovar is not supported, then roam engine is effectively disabled.
      BRCMF_INFO("Roam engine firmware offload is disabled");
      status = ZX_OK;
    } else {
      BRCMF_WARN("Could not disable roam engine offload, firmware error %s",
                 brcmf_fil_get_errstr(fwerr));
    }
  }
  return status;
}

static zx_status_t brcmf_configure_roam_engine(struct brcmf_if* ifp) {
  if (!brcmf_feat_is_enabled(ifp, BRCMF_FEAT_ROAM_ENGINE)) {
    return ZX_OK;
  }
  bcme_status_t fw_err = BCME_OK;
  uint32_t roam_trigger[2];
  uint32_t roam_delta[2];
  roam_trigger[0] = WL_ROAM_TRIGGER_LEVEL;
  roam_trigger[1] = BRCM_BAND_ALL;
  zx_status_t status = brcmf_fil_cmd_data_set(ifp, BRCMF_C_SET_ROAM_TRIGGER, roam_trigger,
                                              sizeof(roam_trigger), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("Failed to set roam trigger, error: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
    return status;
  }

  roam_delta[0] = WL_ROAM_DELTA;
  roam_delta[1] = BRCM_BAND_ALL;
  status =
      brcmf_fil_cmd_data_set(ifp, BRCMF_C_SET_ROAM_DELTA, roam_delta, sizeof(roam_delta), &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("Failed to set roam delta, error: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
  }
  return status;
}

static zx_status_t brcmf_configure_beacon_timeout(struct brcmf_if* ifp) {
  uint32_t bcn_timeout = BRCMF_DEFAULT_BCN_TIMEOUT_ROAM_ENGINE_OFF;
  /* Configure beacon timeout value based upon roaming setting */
  if (ifp->drvr->settings->roam_engine_enabled) {
    bcn_timeout = BRCMF_DEFAULT_BCN_TIMEOUT_ROAM_ENGINE_ON;
  }
  bcme_status_t fw_err;
  const zx_status_t status = brcmf_fil_iovar_int_set(ifp, "bcn_timeout", bcn_timeout, &fw_err);
  if (status != ZX_OK) {
    BRCMF_ERR("Failed to set bcn_timeout, error: %s, fw err %s", zx_status_get_string(status),
              brcmf_fil_get_errstr(fw_err));
  }
  return status;
}

static zx_status_t brcmf_dongle_roam(struct brcmf_if* ifp) {
  zx_status_t err;

  if (brcmf_feat_is_quirk_enabled(ifp, BRCMF_FEAT_QUIRK_IS_4359)) {
    return ZX_OK;  // TODO(https://fxbug.dev/42104096) Find out why, and document.
  }
  err = brcmf_setup_roam_engine(ifp);
  if (err != ZX_OK) {
    return err;
  }
  err = brcmf_configure_roam_engine(ifp);

  return err;
}

static zx_status_t brcmf_dongle_scantime(struct brcmf_if* ifp) {
  zx_status_t err = ZX_OK;
  bcme_status_t fw_err = BCME_OK;

  err = brcmf_fil_cmd_int_set(ifp, BRCMF_C_SET_SCAN_CHANNEL_TIME, BRCMF_SCAN_CHANNEL_TIME, &fw_err);
  if (err != ZX_OK) {
    BRCMF_ERR("Scan assoc time error: %s, fw err %s", zx_status_get_string(err),
              brcmf_fil_get_errstr(fw_err));
    goto dongle_scantime_out;
  }
  err = brcmf_fil_cmd_int_set(ifp, BRCMF_C_SET_SCAN_UNASSOC_TIME, BRCMF_SCAN_UNASSOC_TIME, &fw_err);
  if (err != ZX_OK) {
    BRCMF_ERR("Scan unassoc time error %s, fw err %s", zx_status_get_string(err),
              brcmf_fil_get_errstr(fw_err));
    goto dongle_scantime_out;
  }

  err = brcmf_fil_cmd_int_set(ifp, BRCMF_C_SET_SCAN_PASSIVE_TIME, BRCMF_SCAN_PASSIVE_TIME, &fw_err);
  if (err != ZX_OK) {
    BRCMF_ERR("Scan passive time error %s, fw err %s", zx_status_get_string(err),
              brcmf_fil_get_errstr(fw_err));
    goto dongle_scantime_out;
  }

dongle_scantime_out:
  return err;
}

static zx_status_t brcmf_enable_bw40_2g(struct brcmf_cfg80211_info* cfg) {
  struct brcmf_if* ifp = cfg_to_if(cfg);
  struct brcmf_fil_bwcap_le band_bwcap;
  uint32_t val;
  zx_status_t err;

  /* verify support for bw_cap command */
  val = WLC_BAND_5G;
  err = brcmf_fil_iovar_int_get(ifp, "bw_cap", &val, nullptr);

  if (err == ZX_OK) {
    /* only set 2G bandwidth using bw_cap command */
    band_bwcap.band = WLC_BAND_2G;
    band_bwcap.bw_cap = WLC_BW_CAP_40MHZ;
    err = brcmf_fil_iovar_data_set(ifp, "bw_cap", &band_bwcap, sizeof(band_bwcap), nullptr);
  } else {
    BRCMF_DBG(FIL, "Falling back to mimo_bw_cap to set 40MHz bandwidth for 2.4GHz bands.");
    val = WLC_N_BW_40ALL;
    err = brcmf_fil_iovar_int_set(ifp, "mimo_bw_cap", val, nullptr);
  }

  return err;
}

// Enable/disable Wireless Network Management firmware offloads.
// WNM firmware offloads (e.g. BSS Transition Management) are enabled or
// disabled depending on the brcmf features that are enabled.
// Individual offloads (e.g. BTM) may require additional configuration.
static zx_status_t brcmf_configure_wnm_offloads(struct brcmf_if* ifp) {
  bcme_status_t fwerr;
  uint32_t wnm;
  zx_status_t status = brcmf_fil_iovar_int_get(ifp, "wnm", &wnm, &fwerr);
  if (status != ZX_OK) {
    BRCMF_DBG(FIL, "BSS Transition Management firmware offload lookup failed, firmware error %s",
              brcmf_fil_get_errstr(fwerr));
  }

  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_WNM_BTM)) {
    BRCMF_DBG(FIL, "WNM BTM firmware offload will be enabled in firmware");
    wnm |= WL_WNM_BSSTRANS;
  } else {
    BRCMF_DBG(FIL, "WNM BTM firmware offload will be disabled in firmware");
    wnm &= ~WL_WNM_BSSTRANS;
  }

  status = brcmf_fil_iovar_int_set(ifp, "wnm", wnm, &fwerr);
  if (wnm & WL_WNM_BSSTRANS) {
    if (status == ZX_OK) {
      BRCMF_DBG(FIL, "WNM firmware offload(s) enabled in firmware");
    }
  } else {
    // Disabling everything has one special case.
    if (status == ZX_OK || status == ZX_ERR_NOT_SUPPORTED) {
      // Note: if iovar is not supported, then WNM is effectively disabled.
      BRCMF_DBG(FIL, "WNM firmware offloads were disabled in firmware");
      status = ZX_OK;
    }
  }
  if (status != ZX_OK) {
    BRCMF_WARN("Could not configure WNM offload(s), firmware error %s",
               brcmf_fil_get_errstr(fwerr));
  }
  return status;
}

// Determine whether BSS Transition Management firmware offload is enabled.
static bool brcmf_wnm_btm_offload_enabled(struct brcmf_if* ifp) {
  bcme_status_t fwerr;
  uint32_t wnm;
  const auto status = brcmf_fil_iovar_int_get(ifp, "wnm", &wnm, &fwerr);
  if (wnm & WL_WNM_BSSTRANS) {
    BRCMF_DBG(FIL, "BSS Transition Management firmware offload is enabled");
    return true;
  }
  if (status != ZX_OK) {
    BRCMF_DBG(FIL, "BSS Transition Management firmware offload lookup failed, firmware error %s",
              brcmf_fil_get_errstr(fwerr));
  }
  return false;
}

// Configure BSS Transition Management offload if feature is enabled.
static zx_status_t brcmf_configure_wnm_btm_offload(struct brcmf_if* ifp) {
  zx_status_t status = ZX_OK;

  if (!(brcmf_feat_is_enabled(ifp, BRCMF_FEAT_WNM_BTM) || brcmf_wnm_btm_offload_enabled(ifp))) {
    BRCMF_DBG(FIL, "BSS Transition Management firmware offload is not available");
    return status;
  }

  // Attempt to configure WNM in firmware.
  bcme_status_t fwerr;
  status =
      brcmf_fil_iovar_int_set(ifp, "wnm_bsstrans_resp", WL_BSSTRANS_POLICY_ROAM_ALWAYS, &fwerr);
  if (status == ZX_OK) {
    BRCMF_INFO("BSS Transition Management firmware offload configured, set to ROAM_ALWAYS");
  } else {
    BRCMF_WARN("BSS Transition Management offload could not be configured, firmware error %s",
               brcmf_fil_get_errstr(fwerr));
  }
  return status;
}

static zx_status_t brcmf_config_dongle(struct brcmf_cfg80211_info* cfg) {
  struct net_device* ndev;
  struct wireless_dev* wdev;
  struct brcmf_if* ifp;
  zx_status_t err = ZX_OK;
  bool enable_arp_nd_offload;

  BRCMF_DBG(TEMP, "Enter");
  if (cfg->dongle_up) {
    BRCMF_ERR("Dongle is already up");
    return err;
  }

  ndev = cfg_to_ndev(cfg);
  wdev = ndev_to_wdev(ndev);
  ifp = ndev_to_if(ndev);

  /* make sure RF is ready for work */
  brcmf_fil_cmd_int_set(ifp, BRCMF_C_UP, 0, nullptr);

  brcmf_dongle_scantime(ifp);

  err = brcmf_dongle_roam(ifp);
  if (err != ZX_OK) {
    goto default_conf_out;
  }

  err = brcmf_configure_beacon_timeout(ifp);
  if (err != ZX_OK) {
    goto default_conf_out;
  }

  err = brcmf_configure_wnm_offloads(ifp);
  if (err != ZX_OK) {
    goto default_conf_out;
  }

  // Enable and configure BSS Transition Management offload (if supported).
  err = brcmf_configure_wnm_btm_offload(ifp);
  if (err != ZX_OK) {
    goto default_conf_out;
  }

  err = brcmf_cfg80211_change_iface_mac_role(cfg, ndev, wdev->iftype);
  if (err != ZX_OK) {
    goto default_conf_out;
  }

  enable_arp_nd_offload = !brcmf_feat_is_enabled(ifp, BRCMF_FEAT_AP);
  brcmf_configure_arp_nd_offload(ifp, enable_arp_nd_offload);

  cfg->dongle_up = true;
default_conf_out:
  BRCMF_DBG(TEMP, "Returning %d", err);

  return err;
}

static zx_status_t __brcmf_cfg80211_up(struct brcmf_if* ifp) {
  brcmf_set_bit(brcmf_vif_status_bit_t::READY, &ifp->vif->sme_state);

  return brcmf_config_dongle(ifp->drvr->config);
}

static zx_status_t __brcmf_cfg80211_down(struct brcmf_if* ifp) {
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  /*
   * While going down, if associated with AP disassociate
   * from AP to save power
   */
  if (check_vif_up(ifp->vif)) {
    auto ndev = cfg_to_ndev(cfg);
    auto prof = ndev_to_prof(ndev);
    brcmf_link_down(ifp->vif, fuchsia_wlan_ieee80211::ReasonCode::kUnspecifiedReason, 0,
                    prof->bssid);

    /* Make sure WPA_Supplicant receives all the event
       generated due to DISASSOC call to the fw to keep
       the state fw and WPA_Supplicant state consistent
     */
    msleep(500);
  }

  brcmf_abort_scanning_immediately(cfg);
  brcmf_clear_bit(brcmf_vif_status_bit_t::READY, &ifp->vif->sme_state);

  return ZX_OK;
}

zx_status_t brcmf_cfg80211_up(struct net_device* ndev) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  zx_status_t err = ZX_OK;

  mtx_lock(&cfg->usr_sync);
  err = __brcmf_cfg80211_up(ifp);
  mtx_unlock(&cfg->usr_sync);

  return err;
}

zx_status_t brcmf_cfg80211_down(struct net_device* ndev) {
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_info* cfg = ifp->drvr->config;
  zx_status_t err = ZX_OK;

  mtx_lock(&cfg->usr_sync);
  err = __brcmf_cfg80211_down(ifp);
  mtx_unlock(&cfg->usr_sync);

  return err;
}

fuchsia_wlan_common::WlanMacRole brcmf_cfg80211_get_iftype(struct brcmf_if* ifp) {
  struct wireless_dev* wdev = &ifp->vif->wdev;

  return wdev->iftype;
}

const char* brcmf_cfg80211_get_iface_str(struct net_device* ndev) {
  if (ndev_to_vif(ndev)->wdev.iftype == fuchsia_wlan_common::WlanMacRole::kClient)
    return "Client";
  else
    return "SoftAP";
}

bool brcmf_get_vif_state_any(struct brcmf_cfg80211_info* cfg, brcmf_vif_status_bit_t state) {
  struct brcmf_cfg80211_vif* vif;

  list_for_every_entry (&cfg->vif_list, vif, struct brcmf_cfg80211_vif, list) {
    if (brcmf_test_bit(state, &vif->sme_state)) {
      return true;
    }
  }
  return false;
}

void brcmf_cfg80211_arm_vif_event(struct brcmf_cfg80211_info* cfg, struct brcmf_cfg80211_vif* vif,
                                  uint8_t pending_action) {
  if (cfg == nullptr) {
    return;
  }
  struct brcmf_cfg80211_vif_event* event = &cfg->vif_event;

  mtx_lock(&event->vif_event_lock);
  event->vif = vif;
  event->action = 0;
  sync_completion_reset(&event->vif_event_wait);
  cfg->vif_event_pending_action = pending_action;
  mtx_unlock(&event->vif_event_lock);
}

void brcmf_cfg80211_disarm_vif_event(struct brcmf_cfg80211_info* cfg) {
  if (cfg == nullptr) {
    return;
  }
  struct brcmf_cfg80211_vif_event* event = &cfg->vif_event;

  mtx_lock(&event->vif_event_lock);
  event->vif = nullptr;
  event->action = 0;
  mtx_unlock(&event->vif_event_lock);
}

bool brcmf_cfg80211_vif_event_armed(struct brcmf_cfg80211_info* cfg) {
  if (cfg == nullptr) {
    return false;
  }
  struct brcmf_cfg80211_vif_event* event = &cfg->vif_event;
  bool armed;

  mtx_lock(&event->vif_event_lock);
  armed = event->vif != nullptr;
  mtx_unlock(&event->vif_event_lock);

  return armed;
}

zx_status_t brcmf_cfg80211_wait_vif_event(struct brcmf_cfg80211_info* cfg, zx_duration_t timeout) {
  if (cfg == nullptr) {
    return ZX_ERR_INVALID_ARGS;
  }
  struct brcmf_cfg80211_vif_event* event = &cfg->vif_event;

  return sync_completion_wait(&event->vif_event_wait, timeout);
}

zx_status_t brcmf_cfg80211_del_iface(struct brcmf_cfg80211_info* cfg, struct wireless_dev* wdev) {
  if (wdev == nullptr) {
    BRCMF_ERR("wdev is null");
    return ZX_ERR_INVALID_ARGS;
  }
  struct net_device* ndev = wdev->netdev;
  if (ndev == nullptr) {
    BRCMF_ERR("ndev is null");
    return ZX_ERR_INVALID_ARGS;
  }
  struct brcmf_if* ifp = ndev_to_if(ndev);
  struct brcmf_cfg80211_profile* prof = ndev_to_prof(ndev);

  /* vif event pending in firmware */
  if (brcmf_cfg80211_vif_event_armed(cfg)) {
    return ZX_ERR_UNAVAILABLE;
  }

  if (ndev) {
    if (brcmf_test_bit(brcmf_scan_status_bit_t::BUSY, &cfg->scan_status) &&
        cfg->escan_info.ifp == ndev_to_if(ndev)) {
      BRCMF_WARN("Aborting scan, interface being removed");
      brcmf_abort_scanning_immediately(cfg);
    }

    // Check if there is a client interface by calling cfg_to_ndev first. In case of shutdown this
    // may be called to destroy the AP interface after the client interface has been destroyed.
    if (cfg_to_ndev(cfg)) {
      struct brcmf_if* client_ifp = cfg_to_if(cfg);
      brcmf_enable_mpc(client_ifp, 1);
    }
  }

  zx_status_t err = brcmf_bus_flush_txq(ifp->drvr->bus_if, ifp->ifidx);
  if (err != ZX_OK) {
    BRCMF_ERR("Failed to flush TXQ on iface %d: %s", ifp->ifidx, zx_status_get_string(err));
  }

  switch (wdev->iftype) {
    case fuchsia_wlan_common::WlanMacRole::kAp:
      // Stop the AP in an attempt to exit gracefully.
      brcmf_cfg80211_stop_ap(ndev);
      ndev->sme_channel.reset();
      return brcmf_cfg80211_del_ap_iface(cfg, wdev);
    case fuchsia_wlan_common::WlanMacRole::kClient:
      // Make sure the signal reporter does not access the interface after it has been destroyed.
      cfg->signal_report_work.Cancel();
      // Disconnect the client in an attempt to exit gracefully.
      if (ifp->vif != nullptr) {
        brcmf_link_down(ifp->vif, fuchsia_wlan_ieee80211::ReasonCode::kUnspecifiedReason, false,
                        prof->bssid);
      }
      // The default client iface 0 is always assumed to exist by the driver, and is never
      // explicitly deleted.
      ndev->sme_channel.reset();
      {
        std::lock_guard<std::shared_mutex> guard(ndev->if_proto_lock);
        ndev->if_proto = fidl::WireSyncClient<fuchsia_wlan_fullmac::WlanFullmacImplIfc>();
      }
      ndev->needs_free_net_device = true;
      brcmf_write_net_device_name(ndev, kPrimaryNetworkInterfaceName);
      return ZX_OK;
    default:
      return ZX_ERR_NOT_SUPPORTED;
  }
}

zx_status_t brcmf_cfg80211_attach(struct brcmf_pub* drvr) {
  struct brcmf_cfg80211_info* cfg;
  struct brcmf_cfg80211_vif* vif;
  struct brcmf_if* ifp = brcmf_get_ifp(drvr, 0);
  struct net_device* ndev = ifp->ndev;
  zx_status_t err = ZX_OK;
  bcme_status_t fw_err = BCME_OK;
  int32_t io_type;

  BRCMF_DBG(TEMP, "Enter");
  if (!ndev) {
    BRCMF_ERR("ndev is invalid");
    return ZX_ERR_UNAVAILABLE;
  }

  cfg = static_cast<decltype(cfg)>(calloc(1, sizeof(struct brcmf_cfg80211_info)));
  if (cfg == nullptr) {
    goto cfg80211_info_out;
  }

  cfg->pub = drvr;
  init_vif_event(&cfg->vif_event);
  list_initialize(&cfg->vif_list);
  err = brcmf_alloc_vif(cfg, fuchsia_wlan_common::WlanMacRole::kClient, &vif);
  if (err != ZX_OK) {
    goto cfg80211_info_out;
  }

  vif->ifp = ifp;
  vif->wdev.netdev = ndev;

  err = brcmf_init_cfg(cfg);
  if (err != ZX_OK) {
    BRCMF_ERR("Failed to init cfg (%d)", err);
    brcmf_free_vif(vif);
    goto cfg80211_info_out;
  }
  ifp->vif = vif;

  /* determine d11 io type before wiphy setup */
  err = brcmf_fil_cmd_int_get(ifp, BRCMF_C_GET_VERSION, (uint32_t*)&io_type, &fw_err);
  if (err != ZX_OK) {
    BRCMF_ERR("Failed to get D11 version: %s, fw err %s", zx_status_get_string(err),
              brcmf_fil_get_errstr(fw_err));
    goto cfg_out;
  }
  cfg->d11inf.io_type = (uint8_t)io_type;
  brcmu_d11_attach(&cfg->d11inf);

  // NOTE: linux first verifies that 40 MHz operation is enabled in 2.4 GHz channels.
  err = brcmf_enable_bw40_2g(cfg);
  if (err == ZX_OK) {
    err = brcmf_fil_iovar_int_set(ifp, "obss_coex", BRCMF_OBSS_COEX_AUTO, nullptr);
  }

  drvr->config = cfg;
  err = brcmf_btcoex_attach(cfg);
  if (err != ZX_OK) {
    BRCMF_ERR("BT-coex initialisation failed (%d)", err);
    goto unreg_out;
  }

  if (brcmf_feat_is_enabled(ifp, BRCMF_FEAT_TDLS)) {
    err = brcmf_fil_iovar_int_set(ifp, "tdls_enable", 1, &fw_err);
    if (err != ZX_OK) {
      BRCMF_INFO("TDLS not enabled: %s, fw err %s", zx_status_get_string(err),
                 brcmf_fil_get_errstr(fw_err));
      goto btcoex_out;
    } else {
      brcmf_fweh_register(cfg->pub, BRCMF_E_TDLS_PEER_EVENT, brcmf_notify_tdls_peer_event);
    }
  }

  BRCMF_DBG(TEMP, "Exit");
  return ZX_OK;

btcoex_out:
  brcmf_btcoex_detach(cfg);
unreg_out:
  BRCMF_DBG(TEMP, "* * Would have called wiphy_unregister(cfg->wiphy);");
cfg_out:
  brcmf_deinit_cfg(cfg);
  brcmf_free_vif(vif);
  ifp->vif = nullptr;
cfg80211_info_out:
  free(cfg);
  return err;
}

void brcmf_cfg80211_detach(struct brcmf_cfg80211_info* cfg) {
  if (!cfg) {
    return;
  }

  brcmf_btcoex_detach(cfg);
  BRCMF_DBG(TEMP, "* * Would have called wiphy_unregister(cfg->wiphy);");
  brcmf_deinit_cfg(cfg);
  brcmf_clear_assoc_ies(cfg);
  if (cfg->pub != nullptr) {
    cfg->pub->config = nullptr;
  }
  free(cfg);
}

zx_status_t brcmf_clear_states(struct brcmf_cfg80211_info* cfg) {
  struct brcmf_pub* drvr = cfg->pub;
  struct brcmf_cfg80211_vif* client_vif = drvr->iflist[0]->vif;
  struct net_device* client = client_vif->wdev.netdev;
  struct net_device* softap = cfg_to_softap_ndev(cfg);

  // Stop all interfaces.
  brcmf_if_stop(client);
  if (softap != nullptr)
    brcmf_if_stop(softap);

  // Stop all the timers(for all interfaces).
  cfg->disconnect_timer->Stop();
  cfg->signal_report_timer->Stop();
  cfg->ap_start_timer->Stop();
  cfg->connect_timer->Stop();

  // Clear all driver scan states.
  brcmf_clear_bit(brcmf_scan_status_bit_t::BUSY, &cfg->scan_status);
  brcmf_clear_bit(brcmf_scan_status_bit_t::SUPPRESS, &cfg->scan_status);

  // Clear connect and disconnect states for primary iface.
  brcmf_clear_bit(brcmf_vif_status_bit_t::SAE_AUTHENTICATING, &client_vif->sme_state);
  brcmf_clear_bit(brcmf_vif_status_bit_t::CONNECTING, &client_vif->sme_state);
  brcmf_clear_bit(brcmf_vif_status_bit_t::CONNECTED, &client_vif->sme_state);
  brcmf_clear_bit(brcmf_vif_status_bit_t::DISCONNECTING, &client_vif->sme_state);

  // Always return ZX_OK.
  return ZX_OK;
}

zx_status_t brcmf_suspend_chip(brcmf_pub* drvr) {
  zx_status_t status;

  if (status = brcmf_reset(drvr); status != ZX_OK) {
    BRCMF_ERR("Reset cfg80211 layer failed -- error: %s", zx_status_get_string(status));
    brcmf_detach(drvr);
    return status;
  }
  if (status = brcmf_bus_suspend(drvr->bus_if); status != ZX_OK) {
    // Log the error and proceed. This is more or less a point of no return scenario.
    // Attempt to resume and see if it recovers.
    BRCMF_ERR("Bus suspend failed: %s", zx_status_get_string(status));
  }
  return status;
}

zx_status_t brcmf_resume_chip(brcmf_pub* drvr) {
  zx_status_t status;
  status = brcmf_bus_resume(drvr->bus_if);
  if (status == ZX_OK) {
    BRCMF_INFO("Bus resumed successfully");
  } else {
    BRCMF_INFO("Bus resume failed: %s", zx_status_get_string(status));
  }
  return status;
}
