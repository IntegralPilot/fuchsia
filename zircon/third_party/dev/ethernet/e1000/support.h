/*-
 * SPDX-License-Identifier: BSD-2-Clause
 *
 * Copyright (c) 2016 Nicole Graziano <nicole@nextbsd.org>
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
 * ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
 * ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE
 * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
 * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
 * OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
 * HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
 * LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
 * OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
 * SUCH DAMAGE.
 */

#ifndef ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_SUPPORT_H_
#define ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_SUPPORT_H_

#include <zircon/compiler.h>

#include "adapter.h"
#include "rings.h"

#define em_mac_min e1000_82547
#define igb_mac_min e1000_82575

#define IGB_MEDIA_RESET (1 << 0)

/*
 * See Intel 82574 Driver Programming Interface Manual, Section 10.2.6.9
 */
#define TARC_SPEED_MODE_BIT (1 << 21) /* On PCI-E MACs only */
#define TARC_ERRATA_BIT (1 << 26)     /* Note from errata on 82574 */

namespace e1000 {

// This file contains support functions lifted out of if_em.c and ported to work with Fuchsia

void em_reset(struct adapter* adapter, TxRing<kTxDepth>& tx_ring);

void em_get_hw_control(struct adapter* adapter);
void em_release_hw_control(struct adapter* adapter);

}  // namespace e1000

#endif  // ZIRCON_THIRD_PARTY_DEV_ETHERNET_E1000_SUPPORT_H_
