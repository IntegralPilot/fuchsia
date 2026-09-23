// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netstack

import (
	"context"
	"errors"
	"fmt"
	"net/netip"
	"strings"
	"syscall/zx/fidl"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/fidlconv"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/sync"
	"go.fuchsia.dev/fuchsia/src/lib/component"
	syslog "go.fuchsia.dev/fuchsia/src/lib/syslog/go"

	"fidl/fuchsia/net"
	"fidl/fuchsia/net/neighbor"

	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/header"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
)

const (
	nudTag = "NUD"
)

type nudDispatcher struct {
	ns     *Netstack
	events chan neighborEvent
}

var _ stack.NUDDispatcher = (*nudDispatcher)(nil)

func (d *nudDispatcher) log(verb string, nicID tcpip.NICID, entry stack.NeighborEntry) {
	family := func() string {
		switch l := entry.Addr.Len(); l {
		case header.IPv4AddressSize:
			return "v4"
		case header.IPv6AddressSize:
			return "v6"
		default:
			return fmt.Sprintf("l=%d", l)
		}
	}()
	flags := func() string {
		var defaultGateway, onLink bool
		for _, route := range d.ns.stack.GetRouteTable() {
			if route.NIC != nicID {
				continue
			}
			if id := route.Destination.ID(); id.Len() != entry.Addr.Len() {
				continue
			}
			if route.Destination.Prefix() == 0 {
				if route.Gateway == entry.Addr {
					defaultGateway = true
				}
			} else if route.Destination.Contains(entry.Addr) {
				if route.Gateway.Len() == 0 {
					onLink = true
				}
			}
		}
		var b strings.Builder
		if defaultGateway {
			b.WriteByte('G')
		}
		if onLink {
			b.WriteByte('L')
		}
		if b.Len() != 0 {
			return b.String()
		}
		return "U"
	}()

	// Only log the more significant states at the INFO level to reduce log noise.
	level := func() syslog.LogLevel {
		switch entry.State {
		case stack.Stale, stack.Delay, stack.Probe:
			return syslog.DebugLevel
		case stack.Unknown, stack.Incomplete, stack.Reachable, stack.Unreachable, stack.Static:
			return syslog.InfoLevel
		default:
			panic(fmt.Sprintf("invalid NeighborState = %d: %#v", entry.State, entry))
		}
	}()
	_ = syslog.Logf(syslog.DefaultCallDepth, level, nudTag, "%s %s (%s|%s) NIC=%d LinkAddress=%s %s", verb, entry.Addr, family, flags, nicID, entry.LinkAddr, entry.State)
}

// OnNeighborAdded implements stack.NUDDispatcher.
func (d *nudDispatcher) OnNeighborAdded(nicID tcpip.NICID, entry stack.NeighborEntry) {
	d.events <- neighborEvent{
		kind:  neighborAdded,
		entry: entry,
		nicID: nicID,
	}
	d.log("ADD", nicID, entry)
}

// OnNeighborChanged implements stack.NUDDispatcher.
func (d *nudDispatcher) OnNeighborChanged(nicID tcpip.NICID, entry stack.NeighborEntry) {
	d.events <- neighborEvent{
		kind:  neighborChanged,
		entry: entry,
		nicID: nicID,
	}
	d.log("MOD", nicID, entry)
}

// OnNeighborRemoved implements stack.NUDDispatcher.
func (d *nudDispatcher) OnNeighborRemoved(nicID tcpip.NICID, entry stack.NeighborEntry) {
	d.events <- neighborEvent{
		kind:  neighborRemoved,
		entry: entry,
		nicID: nicID,
	}
	d.log("DEL", nicID, entry)
}

type neighborEventKind int64

const (
	_ neighborEventKind = iota
	neighborAdded
	neighborChanged
	neighborRemoved
)

// maxEntryIteratorItemsQueueLen returns the maximum number of events that the
// fuchsia.net.neighbor implementation will queue on the behalf of clients.
//
// Ensure that it's larger than neighbor.MaxItemBatchSize so that we don't
// artificially truncate event batches. Also ensure that it's larger than
// stack.neighborCacheSize, which is the maximum number of events per interface.
// Here we use 4 times the larger of the two, to ensure that we have some wiggle
// room (e.g. supporting multiple interfaces with the maximum number of
// neighbors).
func maxEntryIteratorItemsQueueLen() uint64 {
	const neighborEntryMultiplier = 4
	// TODO(https://fxbug.dev/42075370): export the stack.neighborCacheSize
	// constant in gVisor so we can depend on it here.
	const gVisorNeighborCacheSize = 512
	if neighbor.MaxItemBatchSize > gVisorNeighborCacheSize {
		return neighbor.MaxItemBatchSize * neighborEntryMultiplier
	} else {
		return gVisorNeighborCacheSize * neighborEntryMultiplier
	}
}

type neighborEvent struct {
	kind  neighborEventKind
	entry stack.NeighborEntry
	nicID tcpip.NICID
}

type neighborEntryKey struct {
	nicID   tcpip.NICID
	address tcpip.Address
}

func (k *neighborEntryKey) String() string {
	return fmt.Sprintf("%s@%d", k.address, k.nicID)
}

type neighborImpl struct {
	stack *stack.Stack

	mu struct {
		sync.Mutex
		state     map[neighborEntryKey]*neighbor.Entry
		iterators map[*neighborEntryIterator]struct{}
		running   bool
	}
}

var _ neighbor.ViewWithCtx = (*neighborImpl)(nil)

// observeEvents starts observing neighbor events from neighborEvent. Can only
// be called once.
func (n *neighborImpl) observeEvents(events <-chan neighborEvent) {
	n.mu.Lock()
	running := n.mu.running
	n.mu.running = true
	n.mu.Unlock()
	if running {
		panic("called ObserveEvents twice on the same implementation")
	}

	for e := range events {
		n.processEvent(e)
	}

	n.mu.Lock()
	n.mu.running = false
	n.mu.Unlock()
}

func (n *neighborImpl) processEvent(event neighborEvent) {
	n.mu.Lock()
	defer n.mu.Unlock()
	key := neighborEntryKey{
		nicID:   event.nicID,
		address: event.entry.Addr,
	}
	propagateEvent, valid := func() (neighbor.EntryIteratorItem, bool) {
		switch event.kind {
		case neighborAdded:
			if old, ok := n.mu.state[key]; ok {
				panic(fmt.Sprintf("observed duplicate add for neighbor key %s. old=%v, event.entry=%v", &key, old, event.entry))
			}
			var newEntry neighbor.Entry
			newEntry.SetInterface(uint64(event.nicID))
			newEntry.SetNeighbor(fidlconv.ToNetIpAddress(event.entry.Addr))
			updateEntry(&newEntry, event.entry)
			n.mu.state[key] = &newEntry
			if newEntry.HasState() {
				return neighbor.EntryIteratorItemWithAdded(newEntry), true
			}
		case neighborRemoved:
			entry, ok := n.mu.state[key]
			if !ok {
				panic(fmt.Sprintf("attempted to remove non existing neighbor %s", &key))
			}
			delete(n.mu.state, key)
			if entry.HasState() {
				return neighbor.EntryIteratorItemWithRemoved(*entry), true
			}
		case neighborChanged:
			entry, ok := n.mu.state[key]
			if !ok {
				panic(fmt.Sprintf("attempted to update non existing neighbor %s. event.entry=%v", &key, event.entry))
			}
			hadState := entry.HasState()
			updateEntry(entry, event.entry)
			if entry.HasState() {
				if hadState {
					return neighbor.EntryIteratorItemWithChanged(*entry), true
				}
				return neighbor.EntryIteratorItemWithAdded(*entry), true
			}
		default:
			panic(fmt.Sprintf("unrecognized neighbor event %d", event.kind))
		}
		return neighbor.EntryIteratorItem{}, false
	}()
	if !valid {
		return
	}
	maxSize := maxEntryIteratorItemsQueueLen()
	for it := range n.mu.iterators {
		it.mu.Lock()
		if uint64(len(it.mu.items)) < maxSize {
			it.mu.items = append(it.mu.items, propagateEvent)
		} else {
			// Stop serving if client is not fetching events fast enough.
			_ = syslog.WarnTf(neighbor.ViewName, "Exceeded maximum queue size of %d, disconnecting.", maxSize)
			it.cancelServe()
		}
		hanging := it.mu.isHanging
		it.mu.Unlock()
		if hanging {
			select {
			case it.ready <- struct{}{}:
			default:
			}
		}
	}
}

func (n *neighborImpl) OpenEntryIterator(_ fidl.Context, req neighbor.EntryIteratorWithCtxInterfaceRequest, _ neighbor.EntryIteratorOptions) error {
	n.mu.Lock()
	defer n.mu.Unlock()

	items := make([]neighbor.EntryIteratorItem, 0, len(n.mu.state)+1)

	for _, e := range n.mu.state {
		if e.HasState() {
			items = append(items, neighbor.EntryIteratorItemWithExisting(*e))
		}
	}

	items = append(items, neighbor.EntryIteratorItemWithIdle(neighbor.IdleEvent{}))

	ctx, cancel := context.WithCancel(context.Background())
	it := &neighborEntryIterator{
		cancelServe: cancel,
		ready:       make(chan struct{}, 1),
	}
	it.mu.Lock()
	it.mu.items = items
	it.mu.Unlock()
	go func() {
		defer cancel()
		component.Serve(ctx, &neighbor.EntryIteratorWithCtxStub{Impl: it}, req.Channel, component.ServeOptions{
			Concurrent: true,
			OnError: func(err error) {
				_ = syslog.WarnTf(neighbor.ViewName, "EntryIterator: %s", err)
			},
		})

		n.mu.Lock()
		delete(n.mu.iterators, it)
		n.mu.Unlock()
	}()

	n.mu.iterators[it] = struct{}{}

	return nil
}

func isValidNeighborAddr(addr netip.Addr) bool {
	limitedBroadcastAddr := netip.AddrFrom4([4]byte{255, 255, 255, 255})
	return !(addr.IsLoopback() || addr.IsMulticast() || addr.IsUnspecified() || addr == limitedBroadcastAddr || addr.Is4In6())
}

var _ neighbor.ControllerWithCtx = (*neighborImpl)(nil)

func (n *neighborImpl) AddEntry(_ fidl.Context, interfaceID uint64, neighborIP net.IpAddress, mac net.MacAddress) (neighbor.ControllerAddEntryResult, error) {
	address, network := fidlconv.ToTCPIPAddressAndProtocolNumber(neighborIP)
	linkAddr := fidlconv.ToTCPIPLinkAddress(mac)
	if !isValidNeighborAddr(fidlconv.ToStdAddr(neighborIP)) {
		return neighbor.ControllerAddEntryResultWithErr(neighbor.ControllerErrorInvalidIpAddress), nil
	}
	if !header.IsValidUnicastEthernetAddress(linkAddr) {
		return neighbor.ControllerAddEntryResultWithErr(neighbor.ControllerErrorMacAddressNotUnicast), nil
	}
	if err := n.stack.AddStaticNeighbor(tcpip.NICID(interfaceID), network, address, linkAddr); err != nil {
		var cErr neighbor.ControllerError
		switch err.(type) {
		case *tcpip.ErrUnknownNICID:
			cErr = neighbor.ControllerErrorInterfaceNotFound
		case *tcpip.ErrNotSupported:
			cErr = neighbor.ControllerErrorInterfaceNotSupported
		}
		return neighbor.ControllerAddEntryResultWithErr(cErr), nil
	}
	return neighbor.ControllerAddEntryResultWithResponse(neighbor.ControllerAddEntryResponse{}), nil
}

func (n *neighborImpl) ProbeEntry(_ fidl.Context, interfaceID uint64, neighborIP net.IpAddress) (neighbor.ControllerProbeEntryResult, error) {
	_ = syslog.WarnTf(neighbor.ControllerName, "ignoring unsupported method ProbeEntry(interface=%d, neighbor=%+v)", interfaceID, neighborIP)
	return neighbor.ControllerProbeEntryResultWithResponse(neighbor.ControllerProbeEntryResponse{}), nil
}

func (n *neighborImpl) RemoveEntry(_ fidl.Context, interfaceID uint64, neighborIP net.IpAddress) (neighbor.ControllerRemoveEntryResult, error) {
	address, network := fidlconv.ToTCPIPAddressAndProtocolNumber(neighborIP)
	if !isValidNeighborAddr(fidlconv.ToStdAddr(neighborIP)) {
		return neighbor.ControllerRemoveEntryResultWithErr(neighbor.ControllerErrorInvalidIpAddress), nil
	}
	if err := n.stack.RemoveNeighbor(tcpip.NICID(interfaceID), network, address); err != nil {
		var cErr neighbor.ControllerError
		switch err.(type) {
		case *tcpip.ErrBadAddress:
			cErr = neighbor.ControllerErrorNeighborNotFound
		case *tcpip.ErrUnknownNICID:
			cErr = neighbor.ControllerErrorInterfaceNotFound
		case *tcpip.ErrNotSupported:
			cErr = neighbor.ControllerErrorInterfaceNotSupported
		}
		return neighbor.ControllerRemoveEntryResultWithErr(cErr), nil
	}
	return neighbor.ControllerRemoveEntryResultWithResponse(neighbor.ControllerRemoveEntryResponse{}), nil
}

func (n *neighborImpl) ClearEntries(_ fidl.Context, interfaceID uint64, ipVersion net.IpVersion) (neighbor.ControllerClearEntriesResult, error) {
	netProto := fidlconv.ToTCPIPNetProto(ipVersion)
	if err := n.stack.ClearNeighbors(tcpip.NICID(interfaceID), netProto); err != nil {
		var cErr neighbor.ControllerError
		switch err.(type) {
		case *tcpip.ErrUnknownNICID:
			cErr = neighbor.ControllerErrorInterfaceNotFound
		case *tcpip.ErrNotSupported:
			cErr = neighbor.ControllerErrorInterfaceNotSupported
		}
		return neighbor.ControllerClearEntriesResultWithErr(cErr), nil
	}
	return neighbor.ControllerClearEntriesResultWithResponse(neighbor.ControllerClearEntriesResponse{}), nil
}

// neighborEntryIterator queues events received from the neighbor table for
// consumption by a FIDL client.
type neighborEntryIterator struct {
	cancelServe context.CancelFunc
	ready       chan struct{}
	mu          struct {
		sync.Mutex
		items     []neighbor.EntryIteratorItem
		isHanging bool
	}
}

var _ neighbor.EntryIteratorWithCtx = (*neighborEntryIterator)(nil)

// GetNext implements neighbor.EntryIteratorWithCtx.GetNext.
func (it *neighborEntryIterator) GetNext(ctx fidl.Context) ([]neighbor.EntryIteratorItem, error) {
	it.mu.Lock()
	defer it.mu.Unlock()

	if it.mu.isHanging {
		it.cancelServe()
		return nil, errors.New("not allowed to call EntryIterator.GetNext when a call is already pending")
	}

	for {
		items := it.mu.items
		avail := len(items)
		if avail != 0 {
			if avail > int(neighbor.MaxItemBatchSize) {
				items = items[:neighbor.MaxItemBatchSize]
				it.mu.items = it.mu.items[neighbor.MaxItemBatchSize:]
			} else {
				it.mu.items = nil
			}
			return items, nil
		}

		it.mu.isHanging = true
		it.mu.Unlock()

		var err error
		select {
		case <-it.ready:
		case <-ctx.Done():
			err = fmt.Errorf("cancelled: %w", ctx.Err())
		}

		it.mu.Lock()
		it.mu.isHanging = false
		if err != nil {
			return nil, err
		}
	}
}

func updateEntry(e *neighbor.Entry, n stack.NeighborEntry) {
	e.SetUpdatedAt(int64(fidlconv.ToZxTime(n.UpdatedAt)))
	if len(n.LinkAddr) != 0 {
		e.SetMac(fidlconv.ToNetMacAddress(n.LinkAddr))
	}

	if entryState, valid := func() (neighbor.EntryState, bool) {
		switch n.State {
		case stack.Unknown:
			// Unknown is an internal state used by the netstack to represent a newly
			// created or deleted entry. Clients do not need to be concerned with this
			// in-between state; all transitions into and out of the Unknown state
			// trigger an event.
			return 0, false
		case stack.Incomplete:
			return neighbor.EntryStateIncomplete, true
		case stack.Reachable:
			return neighbor.EntryStateReachable, true
		case stack.Stale:
			return neighbor.EntryStateStale, true
		case stack.Delay:
			return neighbor.EntryStateDelay, true
		case stack.Probe:
			return neighbor.EntryStateProbe, true
		case stack.Static:
			return neighbor.EntryStateStatic, true
		case stack.Unreachable:
			return neighbor.EntryStateUnreachable, true
		default:
			panic(fmt.Sprintf("invalid NeighborState = %d: %#v", n.State, n))
		}
	}(); valid {
		e.SetState(entryState)
	}
}

func newNudDispatcher() *nudDispatcher {
	return &nudDispatcher{
		// Create channel with enough of a backlog to not block nudDispatcher.
		events: make(chan neighborEvent, 128),
	}
}

func newNeighborImpl(stack *stack.Stack) *neighborImpl {
	impl := &neighborImpl{stack: stack}
	impl.mu.Lock()
	impl.mu.state = make(map[neighborEntryKey]*neighbor.Entry)
	impl.mu.iterators = make(map[*neighborEntryIterator]struct{})
	impl.mu.Unlock()
	return impl
}
