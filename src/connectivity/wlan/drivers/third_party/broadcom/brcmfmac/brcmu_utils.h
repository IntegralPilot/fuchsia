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

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_BRCMU_UTILS_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_BRCMU_UTILS_H_

#include <fidl/fuchsia.wlan.fullmac/cpp/driver/wire.h>
#include <stdlib.h>
#include <string.h>
#include <zircon/compiler.h>

#include <string>

#include <third_party/bcmdhd/crossdriver/dhd.h>

/*
 * Spin at most 'us' microseconds while 'exp' is true.
 * Caller should explicitly test 'exp' when this completes
 * and take appropriate error action if 'exp' is still true.
 */
#define SPINWAIT(exp, us)                           \
  {                                                 \
    uint countdown = (us) + 9;                      \
    while ((exp) && (countdown >= 10)) {            \
      zx_nanosleep(zx_deadline_after(ZX_USEC(10))); \
      countdown -= 10;                              \
    }                                               \
  }

#define BCME_STRLEN 64 /* Max string length for BCM errors */

/* the largest reasonable packet buffer driver uses for ethernet MTU in bytes */
#define PKTBUFSZ 2048

#ifndef setbit
#ifndef NBBY   /* the BSD family defines NBBY */
#define NBBY 8 /* 8 bits per byte */
#endif         /* #ifndef NBBY */
#define setbit(a, i) (((uint8_t*)a)[(i) / NBBY] |= 1 << ((i) % NBBY))
#define clrbit(a, i) (((uint8_t*)a)[(i) / NBBY] &= ~(1 << ((i) % NBBY)))
#define isset(a, i) (((const uint8_t*)a)[(i) / NBBY] & (1 << ((i) % NBBY)))
#define isclr(a, i) ((((const uint8_t*)a)[(i) / NBBY] & (1 << ((i) % NBBY))) == 0)
#endif /* setbit */

#define NBITS(type) (sizeof(type) * 8)
#define NBITVAL(nbits) (1 << (nbits))
#define MAXBITVAL(nbits) ((1 << (nbits)) - 1)
#define NBITMASK(nbits) MAXBITVAL(nbits)
#define MAXNBVAL(nbyte) MAXBITVAL((nbyte) * 8)

/* crc defines */
#define CRC16_INIT_VALUE 0xffff /* Initial CRC16 checksum value */
#define CRC16_GOOD_VALUE 0xf0b8 /* Good final CRC16 checksum value */

/* 18-bytes of Ethernet address buffer length */
#define ETHER_ADDR_STR_LEN 18

/* externs */
/* ip address */
struct ipv4_addr;

/*
 * bitfield macros using masking and shift
 *
 * remark: the mask parameter should be a shifted mask.
 */
static inline void brcmu_maskset32(uint32_t* var, uint32_t mask, uint8_t shift, uint32_t value) {
  value = (value << shift) & mask;
  *var = (*var & ~mask) | value;
}
static inline uint32_t brcmu_maskget32(uint32_t var, uint32_t mask, uint8_t shift) {
  return (var & mask) >> shift;
}
static inline void brcmu_maskset16(uint16_t* var, uint16_t mask, uint8_t shift, uint16_t value) {
  value = (value << shift) & mask;
  *var = (*var & ~mask) | value;
}
static inline uint16_t brcmu_maskget16(uint16_t var, uint16_t mask, uint8_t shift) {
  return (var & mask) >> shift;
}

static inline void* brcmu_alloc_and_copy(const void* buf, size_t size) {
  void* copy = malloc(size);
  if (copy != NULL) {
    memcpy(copy, buf, size);
  }
  return copy;
}

/* externs */
/* format/print */
#if !defined(NDEBUG)
__PRINTFLIKE(3, 4) void brcmu_dbg_hex_dump(const void* data, size_t size, const char* fmt, ...);
#else   // !defined(NDEBUG)
__PRINTFLIKE(3, 4)
static inline void brcmu_dbg_hex_dump(const void* data, size_t size, const char* fmt, ...) {}
#endif  // !defined(NDEBUG)

#define BRCMU_BOARDREV_LEN 8
#define BRCMU_DOTREV_LEN 16

char* brcmu_boardrev_str(uint32_t brev, char* buf);
char* brcmu_dotrev_str(uint32_t dotrev, char* buf);

/*
 * Convert the buckets of a wstats_counter `rx11b[WSTATS_RATE_RANGE_11B]` histogram
 * into a wlanif RxRateIndex histogram. The `out_rx_rate` is expected to be an
 * array of size `WLAN_FULLMAC_MAX_RX_RATE_INDEX_SAMPLES`.
 */
void brcmu_set_rx_rate_index_hist_rx11b(const uint32_t (&rx11b)[WSTATS_RATE_RANGE_11B],
                                        uint32_t* out_rx_rate);
/*
 * Convert the buckets of a wstats_counter `rx11g[WSTATS_RATE_RANGE_11G]` histogram
 * into a wlanif RxRateIndex histogram. The `out_rx_rate` is expected to be an
 * array of size `WLAN_FULLMAC_MAX_RX_RATE_INDEX_SAMPLES`.
 */
void brcmu_set_rx_rate_index_hist_rx11g(const uint32_t (&rx11g)[WSTATS_RATE_RANGE_11G],
                                        uint32_t* out_rx_rate);
/*
 * Convert the buckets of a wstats_counter
 * `rx11n[WSTATS_SGI_RANGE][WSTATS_BW_RANGE_11N][WSTATS_MCS_RANGE_11N]` histogram
 * into a wlanif RxRateIndex histogram. The `out_rx_rate` is expected to be an
 * array of size `WLAN_FULLMAC_MAX_RX_RATE_INDEX_SAMPLES`.
 */
void brcmu_set_rx_rate_index_hist_rx11n(
    const uint32_t (&rx11n)[WSTATS_SGI_RANGE][WSTATS_BW_RANGE_11N][WSTATS_MCS_RANGE_11N],
    uint32_t out_rx_rate[fuchsia_wlan_stats::wire::kMaxRxRateIndexSamples]);
/*
 * Convert the buckets of a wstats_counter
 * `rx11ac[WSTATS_NSS_RANGE][WSTATS_SGI_RANGE][WSTATS_BW_RANGE_11AC][WSTATS_MCS_RANGE_11AC]`
 * histogram into a wlanif RxRateIndex histogram. The `out_rx_rate` is expected to be an
 * array of size `WLAN_FULLMAC_MAX_RX_RATE_INDEX_SAMPLES`.
 */
void brcmu_set_rx_rate_index_hist_rx11ac(
    const uint32_t (
        &rx11ac)[WSTATS_NSS_RANGE][WSTATS_SGI_RANGE][WSTATS_BW_RANGE_11AC][WSTATS_MCS_RANGE_11AC],
    uint32_t* out_rx_rate);

/*
 * Return a string representing the `ssid` vector as "<ssid-BYTES>" where BYTES is
 * a string of hexadecimal characters corresponding to the bytes of the SSID.
 */
std::string brcmu_ssid_format_vector(const std::vector<uint8_t>& ssid);

/*
 * Return a string representing the `ssid_bytes` array of length `ssid_len` as
 * "<ssid-BYTES>" where BYTES is a string of hexadecimal characters
 * corresponding to the bytes of the SSID.
 */
std::string brcmu_ssid_format_bytes(uint8_t const ssid_bytes[], size_t ssid_len);

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_BROADCOM_BRCMFMAC_BRCMU_UTILS_H_
