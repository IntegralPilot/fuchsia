#ifndef ZIRCON_THIRD_PARTY_ULIB_CKSUM_INCLUDE_LIB_CKSUM_H_
#define ZIRCON_THIRD_PARTY_ULIB_CKSUM_INCLUDE_LIB_CKSUM_H_

#include <stddef.h>
#include <stdint.h>
#include <zircon/compiler.h>

__BEGIN_CDECLS

uint32_t crc32(uint32_t crc, const uint8_t *buf, size_t len);

uint32_t crc32_combine(uint32_t crc1, uint32_t crc2, size_t len2);

__END_CDECLS

#endif  // ZIRCON_THIRD_PARTY_ULIB_CKSUM_INCLUDE_LIB_CKSUM_H_
