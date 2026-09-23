// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netstack

import (
	"context"
	"fmt"
	"syscall/zx"
	"testing"
	"time"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/udp_serde"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/util"
	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/faketime"
	"gvisor.dev/gvisor/pkg/tcpip/header"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv4"
	"gvisor.dev/gvisor/pkg/waiter"
)

func TestDatagramSocketWithBlockingEndpoint(t *testing.T) {
	for _, test := range []struct {
		name              string
		closeWhileBlocked bool
	}{
		{name: "closeWhileBlocked", closeWhileBlocked: true},
		{name: "closeAfterUnblocked", closeWhileBlocked: false},
	} {
		t.Run(test.name, func(t *testing.T) {
			ns, _ := newNetstack(t, netstackTestOptions{})
			linkEp := &sentinelEndpoint{}
			linkEp.SetBlocking(true)
			ifState := installAndValidateIface(t, ns, func(t *testing.T, ns *Netstack, name string) *ifState {
				return addLinkEndpoint(t, ns, name, linkEp)
			})
			t.Cleanup(ifState.RemoveByUser)

			addr := tcpip.ProtocolAddress{
				Protocol: ipv4.ProtocolNumber,
				AddressWithPrefix: tcpip.AddressWithPrefix{
					Address:   util.Parse("240.240.240.240"),
					PrefixLen: 24,
				},
			}
			addAddressAndRoute(t, ns, ifState, addr)

			wq := new(waiter.Queue)
			ep := func() tcpip.Endpoint {
				ep, err := ns.stack.NewEndpoint(header.UDPProtocolNumber, ipv4.ProtocolNumber, wq)
				if err != nil {
					t.Fatalf("NewEndpoint(header.UDPProtocolNumber, ipv4.ProtocolNumber, _) = %s", err)
				}
				return ep
			}()

			s, err := newDatagramSocketImpl(ns, ipv4.ProtocolNumber, ep, wq)
			if err != nil {
				t.Fatalf("got newDatagramSocketImpl(_, %d, _, _): %s", ipv4.ProtocolNumber, err)
			}

			// Increment refcount and provide a cancel callback so the endpoint can be
			// closed below.
			s.endpoint.incRef()
			ctx, cancel := context.WithCancel(context.Background())
			s.cancel = cancel

			io, err := s.Describe(context.Background())
			if err != nil {
				t.Fatalf("got s.Describe(): %s", err)
			}

			data := []byte{0, 1, 2, 3, 4}
			preludeSize := io.TxMetaBufSize
			buf := make([]byte, len(data)+int(preludeSize))

			toAddr := &tcpip.FullAddress{
				Addr: addr.AddressWithPrefix.Address,
				Port: 42,
			}
			if err := udp_serde.SerializeSendMsgMeta(
				ipv4.ProtocolNumber,
				*toAddr,
				tcpip.SendableControlMessages{},
				buf[:preludeSize],
			); err != nil {
				t.Fatalf("SerializeSendMsgAddress(%d, %#v, _): %s", ipv4.ProtocolNumber, toAddr, err)
			}
			copy(buf[preludeSize:], data)

			writeUntilBlocked := func() uint {
				written := 0
				for {
					n, err := io.Socket.Write(buf, 0)
					if err == nil {
						if got, want := n, len(buf); got != want {
							t.Fatalf("got zx.socket.Write(_) = (%d, %s), want (%d, nil)", got, err, want)
						}
						written += 1
					} else {
						if err, ok := err.(*zx.Error); ok && err.Status == zx.ErrShouldWait {
							break
						}
						t.Fatalf("got zx.socket.Write(_) = (_, %s), want (_, %s)", err, zx.ErrShouldWait)
					}
				}
				return uint(written)
			}

			const numPayloadsFittingInSendBuf = 10
			bytesPerPayload := len(data) + header.UDPMinimumSize + header.IPv4MaximumHeaderSize
			ep.SocketOptions().SetSendBufferSize(int64(numPayloadsFittingInSendBuf*bytesPerPayload), false)

			var enqueuedSoFar uint
			expectLinkEpEnqueued := func(expected uint) error {
				if got, want := linkEp.Enqueued(), expected+enqueuedSoFar; got != want {
					return fmt.Errorf("got linkEp.Enqueued() = %d, want %d", got, want)
				}
				enqueuedSoFar += expected
				return nil
			}

			// Expect that the sender becomes blocked once the link endpoint has enqueued
			// enough payloads to exhaust the send buffer.

			inflightPayloads := func() uint {
				waiter := linkEp.WaitFor(numPayloadsFittingInSendBuf)
				inflightPayloads := writeUntilBlocked()
				if inflightPayloads < numPayloadsFittingInSendBuf {
					t.Fatalf("wrote %d payloads, want at least %d", inflightPayloads, numPayloadsFittingInSendBuf)
				}
				<-waiter
				if err := expectLinkEpEnqueued(numPayloadsFittingInSendBuf); err != nil {
					t.Fatal(err)
				}
				inflightPayloads -= numPayloadsFittingInSendBuf
				return inflightPayloads
			}()

			// Expect draining N packets lets N more be processed.
			{
				drained, waiter := linkEp.Drain()
				if got, want := drained, uint(numPayloadsFittingInSendBuf); got != want {
					t.Fatalf("got blockingLinkEp.Drain() = %d, want %d", got, want)
				}
				if inflightPayloads < drained {
					t.Fatalf("wrote %d payloads, want at least %d", inflightPayloads, drained)
				}
				<-waiter
				if err := expectLinkEpEnqueued(drained); err != nil {
					t.Fatal(err)
				}
				inflightPayloads -= drained
			}

			validateClose := func() error {
				// Expect the cancel routine is not called before the endpoint was closed.
				if err := ctx.Err(); err != nil {
					return fmt.Errorf("ctx unexpectedly closed with error: %w", err)
				}
				if _, err := s.Close(context.Background()); err != nil {
					return fmt.Errorf("s.Close(): %w", err)
				}

				// Expect the cancel routine is called when the endpoint is closed.
				<-ctx.Done()
				return nil
			}

			if test.closeWhileBlocked {
				if err := validateClose(); err != nil {
					t.Fatal(err)
				}
				// Closing the endpoint while it is blocked drops outgoing payloads
				// on the floor.
				if err := expectLinkEpEnqueued(0); err != nil {
					t.Fatal(err)
				}
			} else {
				linkEp.SetBlocking(false)
				// Wait until the write loop becomes unblocked and begins writing again before
				// closing the socket; otherwise the notifications of the endpoint being
				// writable and the socket closing can race, and the write loop will exit before
				// enqueueing the remaining packets in the zircon socket.
				<-linkEp.WaitFor(1)

				if err := validateClose(); err != nil {
					t.Fatal(err)
				}
				// When the endpoint is unblocked, Close() should block until all
				// remaining payloads are sent.
				if err := expectLinkEpEnqueued(inflightPayloads); err != nil {
					t.Fatal(err)
				}
			}
		})
	}
}

func newNetstackAndEndpoint(t *testing.T, transProto tcpip.TransportProtocolNumber) (*Netstack, *faketime.ManualClock, *waiter.Queue, tcpip.Endpoint) {
	t.Helper()

	ns, clock := newNetstack(t, netstackTestOptions{})
	wq := new(waiter.Queue)
	ep := func() tcpip.Endpoint {
		ep, err := ns.stack.NewEndpoint(transProto, ipv4.ProtocolNumber, wq)
		if err != nil {
			t.Fatalf("NewEndpoint(%d, %d, _): %s", transProto, ipv4.ProtocolNumber, err)
		}
		return ep
	}()
	return ns, clock, wq, ep
}

func addEndpoint(t *testing.T, ns *Netstack, ep *endpoint, stats socketOptionStats) {
	t.Helper()

	ns.onAddEndpoint(ep, stats)
	ep.incRef()
}

func verifyZirconSocketClosed(t *testing.T, e *endpointWithSocket) {
	t.Helper()

	if e.local.Handle().IsValid() {
		t.Error("got e.local.Handle().IsValid() = true, want = false")
	}
	if e.peer.Handle().IsValid() {
		t.Error("got e.peer.Handle().IsValid() = true, want = false")
	}
}

func TestCloseDatagramSocketClosesHandles(t *testing.T) {

	ns, _, wq, ep := newNetstackAndEndpoint(t, header.UDPProtocolNumber)
	s, err := newDatagramSocketImpl(ns, ipv4.ProtocolNumber, ep, wq)
	if err != nil {
		t.Fatalf("newDatagramSocketImpl(_, %d, _, _): %s", ipv4.ProtocolNumber, err)
	}
	addEndpoint(t, ns, &s.endpoint, &s.endpointWithSocket.endpoint.sockOptStats)
	// Provide a cancel callback so the endpoint can be closed below.
	s.cancel = func() {}

	if _, err := s.Close(context.Background()); err != nil {
		t.Fatalf("s.Close(): %s", err)
	}

	// Verify that the handles associated with the socket have been closed.
	verifyZirconSocketClosed(t, s.endpointWithSocket)
	if s.sharedState.destinationCacheMu.destinationCache.local.IsValid() {
		t.Error("got s.sharedState.destinationCacheMu.destinationCache.local.IsValid() = true, want = false")
	}
	if s.sharedState.destinationCacheMu.destinationCache.peer.IsValid() {
		t.Error("got s.sharedState.destinationCacheMu.destinationCache.peer.IsValid() = true, want = false")
	}
	if s.sharedState.cmsgCacheMu.cmsgCache.local.IsValid() {
		t.Error("got s.sharedState.cmsgCacheMu.cmsgCache.local.IsValid() = true, want = false")
	}
	if s.sharedState.cmsgCacheMu.cmsgCache.peer.IsValid() {
		t.Error("got s.sharedState.cmsgCacheMu.cmsgCache.peer.IsValid() = true, want = false")
	}
}

func TestCloseSynchronousDatagramSocketClosesHandles(t *testing.T) {

	ns, _, wq, ep := newNetstackAndEndpoint(t, header.UDPProtocolNumber)
	s, err := makeSynchronousDatagramSocket(ep, ipv4.ProtocolNumber, header.UDPProtocolNumber, wq, ns)
	if err != nil {
		t.Fatalf("makeSynchronousDatagramSocket(_, %d, %d, _, _): %s", ipv4.ProtocolNumber, header.UDPProtocolNumber, err)
	}
	addEndpoint(t, ns, &s.endpoint, &s.endpointWithEvent.endpoint.sockOptStats)
	// Provide a cancel callback so the endpoint can be closed below.
	s.cancel = func() {}

	if _, err := s.Close(context.Background()); err != nil {
		t.Fatalf("s.Close(): %s", err)
	}

	// Verify that the handles associated with the socket have been closed.
	if s.endpointWithEvent.local.IsValid() {
		t.Error("got s.endpointWithEvent.local.IsValid() = true, want = false")
	}
	if s.endpointWithEvent.peer.IsValid() {
		t.Error("got s.endpointWithEvent.peer.IsValid() = true, want = false")
	}
}

func newNetstackAndSreamSocket(t *testing.T) (*faketime.ManualClock, *streamSocketImpl) {
	ns, clock, wq, ep := newNetstackAndEndpoint(t, header.TCPProtocolNumber)
	socketEp, err := newEndpointWithSocket(ep, wq, header.TCPProtocolNumber, ipv4.ProtocolNumber, ns, zx.SocketStream)
	if err != nil {
		t.Fatalf("newEndpointWithSocket(_, _, %d, %d, _, _): %s", header.TCPProtocolNumber, ipv4.ProtocolNumber, err)
	}
	s := makeStreamSocketImpl(socketEp)
	s.endpoint.incRef()

	// Provide a cancel callback so the endpoint can be closed below.
	s.cancel = func() {}

	return clock, &s
}

func TestCloseStreamSocketClosesHandles(t *testing.T) {

	_, s := newNetstackAndSreamSocket(t)

	if _, err := s.Close(context.Background()); err != nil {
		t.Fatalf("s.Close(): %s", err)
	}

	// Verify that the handles associated with the socket have been closed.
	verifyZirconSocketClosed(t, s.endpointWithSocket)
}

func TestCloseUnblockLoopWrite(t *testing.T) {

	tests := []struct {
		name           string
		lingerOpt      tcpip.LingerOption
		tcpLingerOpt   tcpip.TCPLingerTimeoutOption
		dataInZXSocket bool
		closeAfter     time.Duration
	}{
		{
			name:         "SO_LINGER enabled",
			lingerOpt:    tcpip.LingerOption{Enabled: false, Timeout: time.Second},
			tcpLingerOpt: tcpip.TCPLingerTimeoutOption(time.Hour),
			closeAfter:   time.Second,
		},
		{
			name:           "SO_LINGER disabled with empty zx.Socket",
			lingerOpt:      tcpip.LingerOption{Enabled: false, Timeout: time.Hour},
			tcpLingerOpt:   tcpip.TCPLingerTimeoutOption(time.Hour),
			dataInZXSocket: false,
			closeAfter:     0,
		},
		{
			name:           "SO_LINGER disabled with non-empty zx.Socket",
			lingerOpt:      tcpip.LingerOption{Enabled: false, Timeout: time.Hour},
			tcpLingerOpt:   tcpip.TCPLingerTimeoutOption(time.Second),
			dataInZXSocket: true,
			closeAfter:     time.Second,
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			clock, s := newNetstackAndSreamSocket(t)

			s.endpoint.ep.SocketOptions().SetLinger(test.lingerOpt)
			if err := s.endpoint.ep.SetSockOpt(&test.tcpLingerOpt); err != nil {
				t.Fatalf("s.endpoint.ep.SetSockOpt(&%T): %s", test.tcpLingerOpt, err)
			}

			if test.dataInZXSocket {
				var data [1]byte
				if n, err := s.endpointWithSocket.local.Write(data[:], 0 /* flags */); err != nil {
					t.Fatalf("s.endpointWithSocket.local.Write(_, 0): %s", err)
				} else if n != len(data) {
					t.Fatalf("got s.endpointWithSocket.local.Write(_, 0) = %d, want = %d", n, len(data))
				}
			}

			if _, err := s.Close(context.Background()); err != nil {
				t.Fatalf("s.Close(): %s", err)
			}

			clock.Advance(test.closeAfter)

			select {
			case <-s.unblockLoopWrite:
			default:
				t.Error("expected s.unblockLoopWrite to be readable")
			}
		})
	}
}
