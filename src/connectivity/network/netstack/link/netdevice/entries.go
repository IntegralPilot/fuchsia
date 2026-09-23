// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netdevice

import (
	"fmt"
	"math"
	"math/bits"
)

// entries provides basic logic used in link devices to operate on queues of
// entries that can be in one of three states:
// - queued: entries describing buffers already operated on, queued to be sent
// to the driver (populated buffers for Tx, already processed buffers for Rx).
// - in-flight: entries describing buffers currently owned by the driver, not
// yet returned.
// - ready: entries describing buffers retrieved from the driver, but not yet
// operated on (free unpopulated buffers for Tx, buffers containing inbound
// traffic for Rx).
type entries struct {
	// sent, queued, readied are indices modulo (capacity << 1). They
	// implement a ring buffer with 3 regions:
	//
	// - sent:queued:    "queued" entries
	// - queued:readied: "ready" entries
	// - readied:sent:   "in-flight" entries
	//
	// boundary conditions:
	// - readied == sent:   all entries are "in-flight" (implies == queued).
	// - sent == queued:    0 queued entries.
	// - queued == readied: 0 ready entries.
	sent, queued, readied uint16
	storage               []uint16
	// extraCapacity keeps track of extra capacity allocated because storage is
	// rounded up to the next power of 2 to speed up the math. This extra
	// capacity is always notionally in the "in-flight" region of the buffer and
	// is never used by the netdevice client to write valid descriptors into.
	extraCapacity uint16
}

// init initializes entries with a given capacity. After Init, all the
// entries are in the "in-flight" state.
func (e *entries) init(capacity uint16) {
	originalCapacity := capacity
	if capacity != 0 {
		// Round up to power of 2.
		capacity = 1 << bits.Len16(capacity-1)
	}
	if capacity > (math.MaxUint16>>1)+1 {
		panic(fmt.Sprintf("invalid capacity %d, must be smaller than 2^15", capacity))
	}
	*e = entries{
		storage:       make([]uint16, capacity),
		extraCapacity: capacity - originalCapacity,
	}
}

// mask masks an index to the range [0, capacity).
func (e *entries) mask(val uint16) uint16 {
	return val & (uint16(len(e.storage)) - 1)
}

// mask2 masks an index to the range [0, capacity*2).
func (e *entries) mask2(val uint16) uint16 {
	return val & ((uint16(len(e.storage)) << 1) - 1)
}

// incrementSent marks delta entries as moved from "queued" to "in-flight".
// Delta must be limited to the number of queued entries, otherwise entries may
// become inconsistent.
func (e *entries) incrementSent(delta uint16) {
	e.sent = e.mask2(e.sent + delta)
}

// incrementQueued marks delta entries as moved from "ready" to "queued".
// Delta must be limited to the number of ready entries, otherwise entries may
// become inconsistent.
func (e *entries) incrementQueued(delta uint16) {
	e.queued = e.mask2(e.queued + delta)
}

// incrementReadied marks delta entries as moved from "in-flight" to "ready".
// Delta must be limited to the number of in-flight buffers, which is the
// size of the range returned by GetInFlightRange.
func (e *entries) incrementReadied(delta uint16) {
	e.readied = e.mask2(e.readied + delta)
}

// haveQueued returns true if there are entries in the "queued" state.
func (e *entries) haveQueued() bool {
	return e.sent != e.queued
}

// haveReadied returns true if there are entries in the "ready" state.
func (e *entries) haveReadied() bool {
	return e.queued != e.readied
}

// unFlight returns the number of buffers entries in flight (owned by the
// driver).
func (e *entries) inFlight() uint16 {
	var inFlight uint16
	if readied, sent := e.getInFlightRange(); readied < sent {
		inFlight = sent - readied
	} else {
		inFlight = uint16(len(e.storage)) - (readied - sent)
	}
	// The extra capacity is always kept in the "inFlight" state so we must
	// subtract it out when reporting out the numbers.
	return inFlight - e.extraCapacity
}

// getInFlightRange returns the range of indices for entries in "in-flight"
// state that can be move to readied. The end of the range is always exclusive.
// If range start that is larger than or equal to the range end, it must be
// interpreted as two ranges: (start:) and (:end) as opposed to (start:end).
func (e *entries) getInFlightRange() (uint16, uint16) {
	if readied, sent := e.mask(e.readied), e.mask(e.sent); readied == sent && e.sent != e.readied {
		return uint16(len(e.storage)), 0
	} else {
		return readied, sent
	}
}

// getQueuedRange returns the range of indices for entries in "queued" state.
// The end of the range is always exclusive. If range start that is larger than
// or equal to the range end, it must be interpreted as two ranges: (start:) and
// (:end) as opposed to (start:end).
func (e *entries) getQueuedRange() (uint16, uint16) {
	if e.sent == e.queued {
		return uint16(len(e.storage)), 0
	}
	return e.mask(e.sent), e.mask(e.queued)
}

// getReadied returns the first readied entry. Only valid if HaveReadied is
// true.
func (e *entries) getReadied() uint16 {
	return e.storage[e.mask(e.queued)]
}

// addReadied copies the contents of a slice into as many available "in-flight"
// entries as possible, returning the number of copied items.
func (e *entries) addReadied(src []uint16) int {
	if readied, sent := e.getInFlightRange(); readied < sent {
		return copy(e.storage[readied:sent], src)
	} else {
		n := copy(e.storage[readied:], src)
		n += copy(e.storage[:sent], src[n:])
		return n
	}
}

// getQueued copies as many queued entries as possible into the slice, returning
// the number of copied items.
func (e *entries) getQueued(dst []uint16) int {
	if sent, queued := e.getQueuedRange(); sent < queued {
		return copy(dst, e.storage[sent:queued])
	} else {
		n := copy(dst, e.storage[sent:])
		n += copy(dst[n:], e.storage[:queued])
		return n
	}
}

// GoString implements fmt.GoStringer.
func (e entries) GoString() string {
	return fmt.Sprintf("%T{cap=%d, sent=%d, queued=%d, readied=%d}", e, len(e.storage), e.sent, e.queued, e.readied)
}
