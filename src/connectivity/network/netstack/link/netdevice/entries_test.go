// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netdevice

import (
	"fmt"
	"math/bits"
	"testing"
)

func TestEntries(t *testing.T) {
	var e entries

	var maxDepth uint16

	// Use unsigned integer underflow to determine the maximum permitted depth.
	maxDepth = 0
	maxDepth--
	// The maximum permitted depth is half the range of the index type used.
	maxDepth >>= 1

	for _, depth := range []uint16{2, 50, maxDepth} {
		t.Run(fmt.Sprintf("depth=%d", depth), func(t *testing.T) {
			e.init(depth)
			capacity := uint16(len(e.storage))

			if ones := bits.OnesCount16(capacity); ones != 1 {
				t.Fatalf("got len(storage)=%d (binary=%b) want power of two", capacity, capacity)
			}

			scratch := make([]uint16, capacity)

			for _, delta := range []uint16{depth, 1, depth / 2, depth - 1, depth} {
				t.Run(fmt.Sprintf("delta=%d", delta), func(t *testing.T) {
					if e.haveReadied() {
						t.Errorf("got haveReadied=true want=false; %#v", e)
					}
					if got, want := e.addReadied(scratch), int(capacity); got != want {
						t.Errorf("got addReadied=%d want=%d; %#v", got, want, e)
					}

					e.incrementReadied(delta)

					if got, want := e.haveReadied(), delta != 0; got != want {
						t.Errorf("got haveReadied=%t want=%t; %#v", got, want, e)
					}
					if got, want := e.addReadied(scratch), int(capacity-delta); got != want {
						t.Errorf("got addReadied=%d want=%d; %#v", got, want, e)
					}
					if e.haveQueued() {
						t.Errorf("got haveQueued=true want=false; %#v", e)
					}
					if got, want := e.getQueued(scratch), 0; got != want {
						t.Errorf("got getQueued=%d want=%d; %#v", got, want, e)
					}

					e.incrementQueued(delta)

					if got, want := e.haveQueued(), delta != 0; got != want {
						t.Errorf("got haveQueued=%t want=%t; %#v", got, want, e)
					}
					if e.haveReadied() {
						t.Errorf("got haveReadied=true want=false; %#v", e)
					}
					if delta == 0 {
						if got, want := e.getQueued(scratch), int(capacity); got != want {
							t.Errorf("got getQueued=%d want=%d; %#v", got, want, e)
						}
						if inFlight := e.inFlight(); inFlight != 0 {
							t.Errorf("got inFlight()=%d want=zero; %#v", inFlight, e)
						}
					} else {
						if got, want := e.getQueued(scratch), int(delta); got != want {
							t.Errorf("got getQueued=%d want=%d; %#v", got, want, e)
						}
						if got, want := e.inFlight(), capacity-delta-e.extraCapacity; got != want {
							t.Errorf("got inFlight()=%d want=%d; %#v", got, want, e)
						}
					}

					e.incrementSent(delta)

					if got, want := e.inFlight(), capacity-e.extraCapacity; got != want {
						t.Errorf("got inFlight()=%d want=%d; %#v", got, want, e)
					}
					if got, want := e.addReadied(scratch), int(capacity); got != want {
						t.Errorf("got addReadied=%d want=%d; %#v", got, want, e)
					}
					if got, want := e.getQueued(scratch), 0; got != want {
						t.Errorf("got getQueued=%d want=%d; %#v", got, want, e)
					}
					if e.haveQueued() {
						t.Errorf("got haveQueued=true want=false; %#v", e)
					}
					if e.haveReadied() {
						t.Fatalf("got haveReadied=true want=false; %#v", e)
					}
				})
			}
		})
	}
}
