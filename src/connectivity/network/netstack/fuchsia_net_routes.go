// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netstack

import (
	"context"
	"fmt"
	"strings"
	"sync/atomic"
	"syscall/zx"
	"syscall/zx/fidl"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/fidlconv"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routes"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routetypes"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/sync"
	"go.fuchsia.dev/fuchsia/src/lib/component"
	syslog "go.fuchsia.dev/fuchsia/src/lib/syslog/go"

	"fidl/fuchsia/net"
	fnetRoutes "fidl/fuchsia/net/routes"

	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
)

const routesFidlName = "fuchsia.net.routes"
const watcherV4ProtocolName = "fuchsia.net.routes.WatcherV4"
const watcherV6ProtocolName = "fuchsia.net.routes.WatcherV6"

var _ fnetRoutes.StateWithCtx = (*resolveImpl)(nil)

// resolveImpl provides an implementation for fuchsia.net.routes/State.
type resolveImpl struct {
	stack *stack.Stack
}

var _ fnetRoutes.StateV4WithCtx = (*getWatcherImpl)(nil)
var _ fnetRoutes.StateV6WithCtx = (*getWatcherImpl)(nil)

// getWatcherImpl provides implementations for fuchsia.net.routes/StateV4 and
// fuchsia.net.routes/State.V6.
type getWatcherImpl struct {
	interruptChan chan<- routeInterrupt
}

func (r *resolveImpl) resolveInner(ctx fidl.Context, destination net.IpAddress, options fnetRoutes.ResolveOptions) (fnetRoutes.ResolveResult, tcpip.Error) {
	if options.HasMarks() {
		syslog.WarnTf(routesFidlName, "Resolve2 received non-empty options, will ignore")
	}
	const unspecifiedNIC = tcpip.NICID(0)
	var unspecifiedLocalAddress tcpip.Address

	remote, proto := fidlconv.ToTCPIPAddressAndProtocolNumber(destination)
	netProtoName := strings.TrimPrefix(networkProtocolToString(proto), "IP")
	var flags string
	syslogFn := syslog.DebugTf
	if remote.Unspecified() {
		flags = "U"
		syslogFn = syslog.InfoTf
	}
	logFn := func(suffix string, err tcpip.Error) {
		_ = syslogFn(
			routesFidlName, "stack.FindRoute(%s (%s|%s))%s = (_, %s)",
			remote,
			netProtoName,
			flags,
			suffix,
			err,
		)
	}

	route, err := r.stack.FindRoute(unspecifiedNIC, unspecifiedLocalAddress, remote, proto, false /* multicastLoop */)
	if err != nil {
		logFn("", err)
		return fnetRoutes.ResolveResult{}, err
	}
	defer route.Release()

	return func() (fnetRoutes.ResolveResult, tcpip.Error) {
		ch := make(chan stack.ResolvedFieldsResult, 1)
		err := route.ResolvedFields(func(result stack.ResolvedFieldsResult) {
			ch <- result
		})
		switch err.(type) {
		case nil, *tcpip.ErrWouldBlock:
			select {
			case result := <-ch:
				if result.Err == nil {
					// Build our response with the resolved route.
					nicID := route.NICID()
					route := result.RouteInfo

					var node fnetRoutes.Destination
					node.SetSourceAddress(fidlconv.ToNetIpAddress(route.LocalAddress))
					// If the remote link address is unspecified, then the outgoing link
					// does not support MAC addressing.
					if linkAddr := route.RemoteLinkAddress; len(linkAddr) != 0 {
						node.SetMac(fidlconv.ToNetMacAddress(linkAddr))
					}
					node.SetInterfaceId(uint64(nicID))

					var result fnetRoutes.ResolveResult
					if route.NextHop.Len() != 0 {
						node.SetAddress(fidlconv.ToNetIpAddress(route.NextHop))
						result.SetGateway(node)
					} else {
						node.SetAddress(fidlconv.ToNetIpAddress(route.RemoteAddress))
						result.SetDirect(node)
					}
					return result, nil
				}
				err = result.Err
			case <-ctx.Done():
				switch ctx.Err() {
				case context.Canceled:
					return fnetRoutes.ResolveResult{}, &tcpip.ErrAborted{}
				case context.DeadlineExceeded:
					return fnetRoutes.ResolveResult{}, &tcpip.ErrTimeout{}
				}
			}
		}
		logFn(".ResolvedFields(...)", err)
		return fnetRoutes.ResolveResult{}, &tcpip.ErrNetworkUnreachable{}
	}()
}

func (r *resolveImpl) Resolve2(ctx fidl.Context, destination net.IpAddress, options fnetRoutes.ResolveOptions) (fnetRoutes.StateResolve2Result, error) {
	result, err := r.resolveInner(ctx, destination, options)
	if err != nil {
		return fnetRoutes.StateResolve2ResultWithErr(fnetRoutes.ResolveErrorAddressUnreachable), nil
	}
	return fnetRoutes.StateResolve2ResultWithResponse(fnetRoutes.StateResolve2Response{
		Result: result,
	}), nil
}

func (r *resolveImpl) Resolve(ctx fidl.Context, destination net.IpAddress) (fnetRoutes.StateResolveResult, error) {
	result, err := r.resolveInner(ctx, destination, fnetRoutes.ResolveOptions{})
	if err != nil {
		switch err.(type) {
		case *tcpip.ErrAborted:
			return fnetRoutes.StateResolveResultWithErr(int32(zx.ErrCanceled)), nil
		default:
			return fnetRoutes.StateResolveResultWithErr(int32(WrapTcpIpError(err).ToZxStatus())), nil
		}
	}
	var response fnetRoutes.StateResolveResponse
	switch tag := result.Which(); tag {
	case fnetRoutes.ResolveResultGateway:
		response.Result.SetGateway(result.Gateway)
	case fnetRoutes.ResolveResultDirect:
		response.Result.SetDirect((result.Direct))
	default:
		panic(fmt.Sprintf("Invalid tag for the result: %d", tag))
	}
	return fnetRoutes.StateResolveResultWithResponse(response), nil
}

// GetRouteTableName implements fuchsia.net.routes/State.GetRouteTableName.
func (r *resolveImpl) GetRouteTableName(ctx_ fidl.Context, tableId uint32) (fnetRoutes.StateGetRouteTableNameResult, error) {
	panic("TODO(https://fxbug.dev/336205291): Implement for main table")
}

// GetWatcherV4 implements fuchsia.net.routes/StateV4.GetWatcherV4.
func (r *getWatcherImpl) GetWatcherV4(
	_ fidl.Context,
	watcher fnetRoutes.WatcherV4WithCtxInterfaceRequest,
	options fnetRoutes.WatcherOptionsV4,
) error {
	r.interruptChan <- &getWatcherV4Request{
		req:     watcher,
		options: options,
	}
	return nil
}

// GetRuleWatcherV4 implements fuchsia.net.routes/StateV4.GetRuleWatcherV4.
func (r *getWatcherImpl) GetRuleWatcherV4(
	_ fidl.Context,
	watcher fnetRoutes.RuleWatcherV4WithCtxInterfaceRequest,
	options fnetRoutes.RuleWatcherOptionsV4,
) error {
	panic("TODO(https://fxbug.dev/336204757): Implement rules watcher")
}

// GetWatcherV6 implements fuchsia.net.routes/StateV6.GetWatcherV6.
func (r *getWatcherImpl) GetWatcherV6(
	_ fidl.Context,
	watcher fnetRoutes.WatcherV6WithCtxInterfaceRequest,
	options fnetRoutes.WatcherOptionsV6,
) error {
	r.interruptChan <- &getWatcherV6Request{
		req:     watcher,
		options: options,
	}
	return nil
}

// GetRuleWatcherV6 implements fuchsia.net.routes/StateV6.GetRuleWatcherV6.
func (r *getWatcherImpl) GetRuleWatcherV6(
	_ fidl.Context,
	watcher fnetRoutes.RuleWatcherV6WithCtxInterfaceRequest,
	options fnetRoutes.RuleWatcherOptionsV6,
) error {
	panic("TODO(https://fxbug.dev/336204757): Implement rules watcher")
}

// routesGetWatcherRequest is an interface for GetWatcher requests, abstracting
// over the V4 & V6 variants.
type routesGetWatcherRequest interface {
	// serve serves the GetWatcher request by instantiating a Watcher client.
	serve(
		ctx context.Context,
		cancel context.CancelFunc,
		existingRoutes map[installedRouteComparisonKey]struct{},
		eventsChan chan eventUnion,
		onClose chan<- routeInterrupt,
		metrics *fidlRoutesWatcherMetrics,
	) *routesWatcherImplInner
}

var _ routesGetWatcherRequest = (*getWatcherV4Request)(nil)
var _ routesGetWatcherRequest = (*getWatcherV6Request)(nil)

// getWatcherV4Request is an implementation of the routesGetWatcherRequest
// interface for IPv4.
type getWatcherV4Request struct {
	req     fnetRoutes.WatcherV4WithCtxInterfaceRequest
	options fnetRoutes.WatcherOptionsV4
}

// getWatcherV6Request is an implementation of the routesGetWatcherRequest
// interface for IPv6.
type getWatcherV6Request struct {
	req     fnetRoutes.WatcherV6WithCtxInterfaceRequest
	options fnetRoutes.WatcherOptionsV6
}

// serve implements routesGetWatcherRequest.
func (r *getWatcherV4Request) serve(
	ctx context.Context,
	cancel context.CancelFunc,
	existingRoutes map[installedRouteComparisonKey]struct{},
	eventsChan chan eventUnion,
	onClose chan<- routeInterrupt,
	metrics *fidlRoutesWatcherMetrics,
) *routesWatcherImplInner {
	impl := routesWatcherV4Impl{
		inner: makeRoutesWatcherImplInner(
			cancel,
			eventsChan,
			// Filter out all Non-IPv4 events.
			func(e *eventUnion) bool {
				return e.version == routetypes.IPv4
			},
			existingRoutes,
			eventUnion{
				version:    routetypes.IPv4,
				ipv4_event: fnetRoutes.EventV4WithIdle(fnetRoutes.Empty{}),
			},
		),
	}

	go func() {
		defer func() {
			metrics.count_v4.Add(-1)
			cancel()
			onClose <- &(impl.inner)
		}()
		metrics.count_v4.Add(1)
		component.Serve(
			ctx,
			&fnetRoutes.WatcherV4WithCtxStub{Impl: &impl},
			r.req.Channel,
			component.ServeOptions{
				Concurrent: true,
				OnError: func(err error) {
					_ = syslog.WarnTf(watcherV4ProtocolName, "%s", err)
				},
			})
	}()

	return &(impl.inner)
}

// serve implements routesGetWatcherRequest.
func (r *getWatcherV6Request) serve(
	ctx context.Context,
	cancel context.CancelFunc,
	existingRoutes map[installedRouteComparisonKey]struct{},
	eventsChan chan eventUnion,
	onClose chan<- routeInterrupt,
	metrics *fidlRoutesWatcherMetrics,
) *routesWatcherImplInner {
	impl := routesWatcherV6Impl{
		inner: makeRoutesWatcherImplInner(
			cancel,
			eventsChan,
			// Filter out all Non-IPv6 events.
			func(e *eventUnion) bool {
				return e.version == routetypes.IPv6
			},
			existingRoutes,
			eventUnion{
				version:    routetypes.IPv6,
				ipv6_event: fnetRoutes.EventV6WithIdle(fnetRoutes.Empty{}),
			},
		),
	}

	go func() {
		defer func() {
			metrics.count_v6.Add(-1)
			cancel()
			onClose <- &(impl.inner)
		}()
		metrics.count_v6.Add(1)
		component.Serve(
			ctx,
			&fnetRoutes.WatcherV6WithCtxStub{Impl: &impl},
			r.req.Channel,
			component.ServeOptions{
				Concurrent: true,
				OnError: func(err error) {
					_ = syslog.WarnTf(watcherV6ProtocolName, "%s", err)
				},
			})
	}()

	return &(impl.inner)
}

// isRouteInterrupt implements routeInterrupt.
func (*getWatcherV4Request) isRouteInterrupt() {}

// isRouteInterrupt implements routeInterrupt.
func (*getWatcherV6Request) isRouteInterrupt() {}

// routesWatcherImplInner is the implementation of a routes Watcher protocol.
type routesWatcherImplInner struct {
	// cancel is the function to call to cancel the watcher.
	cancel context.CancelFunc
	// eventsChan is the channel of events that should be sent to this watcher's
	// client.
	eventsChan chan eventUnion
	// existingEvents are the routes that existed at the time this watcher
	// client was instantiated, followed by the idle sentinel. Note that
	// existing routes are stored out of band from eventsChan, because the
	// number of existing routes may exceed the size of the eventsChan.
	existingEvents []eventUnion
	// filter is the predicate applied to events before they are pushed into
	// eventsChan.
	filter func(*eventUnion) bool
	mu     struct {
		sync.Mutex
		// isHanging is True while a Watch request is in progress. E.g.
		// "hanging" in the Hanging-Get design pattern.
		isHanging bool
	}
}

// makeRoutesWatcherImplInner constructs a new routesWatcherImplInner.
func makeRoutesWatcherImplInner(
	cancel context.CancelFunc,
	eventsChan chan eventUnion,
	filter func(*eventUnion) bool,
	existingRoutes map[installedRouteComparisonKey]struct{},
	idleEvent eventUnion,
) routesWatcherImplInner {
	// Create the existing events.
	var existingEvents []eventUnion
	for key := range existingRoutes {
		installedRoute, err := fromInstalledRouteComparisonKey(key)
		if err != nil {
			panic(fmt.Sprintf("observed error while attempting to construct route from comparison key %#v: %s", key, err))
		}
		event := toEvent(existingEvent, installedRoute)
		if filter(&event) {
			existingEvents = append(existingEvents, event)
		}
	}
	existingEvents = append(existingEvents, idleEvent)
	return routesWatcherImplInner{
		cancel:         cancel,
		eventsChan:     eventsChan,
		filter:         filter,
		existingEvents: existingEvents,
	}
}

// pushEvent pushes the given event into the watcher's event channel, if it
// passes the watcher's filter. If the push would block (indicating the client
// isn't keeping up with the flow of events), the client is canceled.
func (w *routesWatcherImplInner) pushEvent(e eventUnion) {
	if !w.filter(&e) {
		return
	}
	select {
	case w.eventsChan <- e:
		// The event was successfully dispatched without blocking.
	default:
		_ = syslog.WarnTf(
			routesFidlName,
			"too many unconsumed events "+
				"(the client may not be calling Watch frequently enough): %d",
			maxPendingEventsPerClient,
		)
		w.cancel()
	}
}

// pullEvents pulls events from the underlying eventsChan. This call blocks if
// there are no available events. Otherwise, this call pulls all available
// events from the underlying channel (up to fnetRoutes.MaxEvents) and returns
// immediately.
func (w *routesWatcherImplInner) pullEvents(fidlCtx fidl.Context) ([]eventUnion, error) {
	var events []eventUnion
	for len(events) < int(fnetRoutes.MaxEvents) {
		if len(w.existingEvents) > 0 {
			// First drain the existing routes, before pulling from the channel.
			var existing eventUnion
			existing, w.existingEvents = w.existingEvents[0], w.existingEvents[1:]
			events = append(events, existing)
		} else if len(events) != 0 {
			// Poll the channel in a non-blocking fashion if we've already
			// accumulated some events.
			select {
			case <-fidlCtx.Done():
				return nil, fmt.Errorf("cancelled: %w", fidlCtx.Err())
			case event := <-w.eventsChan:
				events = append(events, event)
			default:
				return events, nil
			}
		} else {
			// Poll the channel and block until an event is available.
			select {
			case <-fidlCtx.Done():
				return nil, fmt.Errorf("cancelled: %w", fidlCtx.Err())
			case event := <-w.eventsChan:
				events = append(events, event)
			}
		}
	}
	return events, nil
}

// watch services a single call to Watch. This function returns an error if
// there was already a pending call to Watch or if it was canceled while waiting
// for available events.
func (w *routesWatcherImplInner) watch(fidlCtx fidl.Context) ([]eventUnion, error) {
	{
		w.mu.Lock()
		if w.mu.isHanging {
			w.cancel()
			w.mu.Unlock()
			return nil, fmt.Errorf("Watch called while a prior call is still pending")
		}
		w.mu.isHanging = true
		w.mu.Unlock()
	}

	// Fetch Events, blocking if none are available.
	events, err := w.pullEvents(fidlCtx)

	if err != nil {
		w.cancel()
	}

	{
		w.mu.Lock()
		w.mu.isHanging = false
		w.mu.Unlock()
	}

	return events, err
}

// isRouteInterrupt implements routeInterrupt.
func (*routesWatcherImplInner) isRouteInterrupt() {}

// routesWatcherV4Impl and routesWatcherV6Impl wrap routesWatcherImplInner to
// implement WatcherV4 and WatcherV6, respectively. Note that this layer of
// indirection is necessary because both WatcherV4 and WatcherV6 expose a Watch
// method with an identical signature. Trying to to implement the protocols
// directly leads to conflicting implementations (e.g. Watch func defined
// multiple times).

var _ fnetRoutes.WatcherV4WithCtx = (*routesWatcherV4Impl)(nil)

type routesWatcherV4Impl struct {
	inner routesWatcherImplInner
}

var _ fnetRoutes.WatcherV6WithCtx = (*routesWatcherV6Impl)(nil)

type routesWatcherV6Impl struct {
	inner routesWatcherImplInner
}

// Watch implements fuchsia.net.routes/WatcherV4.Watch.
func (w4 *routesWatcherV4Impl) Watch(ctx fidl.Context) ([]fnetRoutes.EventV4, error) {
	events, err := w4.inner.watch(ctx)
	if err != nil {
		return nil, err
	}

	eventsV4 := make([]fnetRoutes.EventV4, 0, len(events))
	for _, event := range events {
		switch event.version {
		case routetypes.IPv4:
			eventsV4 = append(eventsV4, event.ipv4_event)
		case routetypes.IPv6:
			// Unreachable because of the filter installed by GetWatcherV4.
			panic(fmt.Sprintf(
				"Internal Error. %s received an IPv6 event: %s",
				watcherV4ProtocolName,
				intoLogString(event),
			))
		default:
			panic(fmt.Sprintf("Event with invalid IP protocol :%d", event.version))
		}
	}
	return eventsV4, nil
}

// Watch implements fuchsia.net.routes/WatcherV6.Watch.
func (w6 *routesWatcherV6Impl) Watch(ctx fidl.Context) ([]fnetRoutes.EventV6, error) {
	events, err := w6.inner.watch(ctx)
	if err != nil {
		return nil, err
	}

	eventsV6 := make([]fnetRoutes.EventV6, 0, len(events))
	for _, event := range events {
		switch event.version {
		case routetypes.IPv4:
			// Unreachable because of the filter installed by GetWatcherV4.
			panic(fmt.Sprintf(
				"Internal Error. %s received an IPv4 event: %s",
				watcherV6ProtocolName,
				intoLogString(event),
			))
		case routetypes.IPv6:
			eventsV6 = append(eventsV6, event.ipv6_event)
		default:
			panic(fmt.Sprintf("Event with invalid IP protocol :%d", event.version))
		}
	}
	return eventsV6, nil
}

// fidlRoutesWatcherMetrics defines metrics for the
// fuchsia.net.routes Watcher protocols.
type fidlRoutesWatcherMetrics struct {
	count_v4 atomic.Int64
	count_v6 atomic.Int64
}

// eventUnion is a union type abstracting over IPv4 and IPv6 events.
// ipv4_event will be set iif version == IPv4 (and vice-versa for ipv6_event).
type eventUnion struct {
	version    routetypes.IpProtoTag
	ipv4_event fnetRoutes.EventV4
	ipv6_event fnetRoutes.EventV6
}

// maxPendingEventsPerClient is the maximum number of events queued for a single
// Watcher client. The value should be large enough to give clients some leeway
// when processing entries, but small enough to not waste memory. Clients are
// responsible for pulling events from the queue, thus it may back up.
const maxPendingEventsPerClient = 5 * fnetRoutes.MaxEvents

// maxPendingInterrupts is the maximum number of interrupts that have not yet
// been handled by the routesWatcherEventLoop. The value is somewhat arbitrary,
// because the changes are "drained" by the routesWatcherEventLoop immediately;
// however, larger values may reduce contention between goroutines.
const maxPendingInterrupts = 100

// routeInterrupt is a marker interface used to improve type safety in
// routesWatcherEventLoop.
type routeInterrupt interface {
	isRouteInterrupt()
}

// routingTableChanged wraps routes.RoutingTableChanged so that it can implement
// routeEvent.
type routingTableChange struct {
	routes.RoutingTableChange
}

// isRouteInterrupt implements routeInterrupt.
func (*routingTableChange) isRouteInterrupt() {}

// routesWatcherEventLoop is the main event loop servicing clients of the
// fuchsia.net.routes Watcher protocols.
func routesWatcherEventLoop(
	ctx context.Context,
	interruptChan chan routeInterrupt,
	metrics *fidlRoutesWatcherMetrics,
) {
	// Keep track of the current routing table state as changes are received.
	// This allows us to push existing events to new watcher clients without
	// having to call into the system Routing table, which may introduce race
	// conditions.
	currentRoutes := make(map[installedRouteComparisonKey]struct{})

	// Keep track of the active Watcher implementations
	currentWatchers := make(map[*routesWatcherImplInner]struct{})

	for {
		select {
		case <-ctx.Done():
			_ = syslog.WarnTf(routesFidlName, "stopping routes watcher event loop")
			// Wait for all watchers to close.
			for len(currentWatchers) > 0 {
				switch interrupt := (<-interruptChan).(type) {
				case *routesWatcherImplInner:
					delete(currentWatchers, interrupt)
				case *routingTableChange, *getWatcherV4Request, *getWatcherV6Request:
					// Ignore all other interrupts when canceled.
				default:
					panic(fmt.Sprintf("unknown interrupt: %T", interrupt))
				}
			}
			return
		case interrupt := <-interruptChan:
			switch interrupt := interrupt.(type) {
			case *routingTableChange:
				route := fidlconv.ToInstalledRoute(interrupt.Route)
				key, err := toInstalledRouteComparisonKey(route)
				if err != nil {
					panic(fmt.Sprintf("observed error while attempting to compute comparison key for %#v: %s", route, err))
				}
				var eventType eventTag
				// Updates routes based on the received change.
				switch interrupt.Change {
				case routetypes.RouteAdded:
					if _, present := currentRoutes[key]; present {
						panic(fmt.Sprintf(
							"received duplicate add event for route: %+v",
							interrupt.Route,
						))
					}
					currentRoutes[key] = struct{}{}
					eventType = addedEvent
				case routetypes.RouteRemoved:
					if _, present := currentRoutes[key]; !present {
						panic(fmt.Sprintf(
							"received remove event for non-existent route: %+v",
							interrupt.Route,
						))
					}
					delete(currentRoutes, key)
					eventType = removedEvent
				default:
					panic(fmt.Sprintf(
						"observed unexpected routing table change :%+v",
						interrupt.Route,
					))
				}
				// Notify the watchers of change
				event := toEvent(eventType, route)
				for watcherImpl := range currentWatchers {
					watcherImpl.pushEvent(event)
				}
			case *getWatcherV4Request, *getWatcherV6Request:
				// NB: because we're using a case-list, Golang will keep
				// interrupt as an routeInterrupt, instead of rebinding it to a
				// more specific type.
				var watcher routesGetWatcherRequest
				var ok bool
				if watcher, ok = interrupt.(routesGetWatcherRequest); !ok {
					panic(fmt.Sprintf(
						"interrupt was impossibly not a routesGetWatcherRequest: %T",
						interrupt,
					))
				}
				// Serve the new watcher client.
				eventsChan := make(chan eventUnion, maxPendingEventsPerClient)
				watcherCtx, cancel := context.WithCancel(ctx)
				impl := watcher.serve(
					watcherCtx, cancel, currentRoutes, eventsChan, interruptChan, metrics,
				)
				currentWatchers[impl] = struct{}{}
			case *routesWatcherImplInner:
				delete(currentWatchers, interrupt)
			default:
				panic(fmt.Sprintf("unknown interrupt: %T", interrupt))
			}
		}
	}
}

type eventTag uint32

const (
	_ eventTag = iota
	existingEvent
	addedEvent
	removedEvent
)

// toEvent converts the given route into an event based on the provided
// eventTag.
func toEvent(eventType eventTag, route fidlconv.InstalledRoute) eventUnion {
	switch route.Version {
	case routetypes.IPv4:
		var event fnetRoutes.EventV4
		switch eventType {
		case existingEvent:
			event.SetExisting(route.V4)
		case addedEvent:
			event.SetAdded(route.V4)
		case removedEvent:
			event.SetRemoved(route.V4)
		default:
			panic(fmt.Sprintf("invalid eventTag: %d", eventType))
		}
		return eventUnion{
			version:    routetypes.IPv4,
			ipv4_event: event,
		}
	case routetypes.IPv6:
		var event fnetRoutes.EventV6
		switch eventType {
		case existingEvent:
			event.SetExisting(route.V6)
		case addedEvent:
			event.SetAdded(route.V6)
		case removedEvent:
			event.SetRemoved(route.V6)
		default:
			panic(fmt.Sprintf("invalid eventTag: %d", eventType))
		}
		return eventUnion{
			version:    routetypes.IPv6,
			ipv6_event: event,
		}
	default:
		panic(fmt.Sprintf("Route with invalid IP protocol :%d", route.Version))
	}
}

// intoLogString converts the given eventUnion into a string suitable for logs.
// It's important not to log the FIDL type directly, as it's address format
// prevents PII redaction.
func intoLogString(e eventUnion) string {
	fmtInstalledRouteV4 := func(r fnetRoutes.InstalledRouteV4) string {
		dst := fidlconv.ToTCPIPSubnet(net.Subnet{
			Addr:      net.IpAddressWithIpv4(r.Route.Destination.Addr),
			PrefixLen: r.Route.Destination.PrefixLen,
		})
		return fmt.Sprintf(
			"InstalledRoute: %T{ EffectiveProperties: %#v, Route: %T{ Destination: %s, %s, Properties: %#v } }",
			r,
			r.EffectiveProperties,
			r.Route,
			dst,
			func() string {
				return fmt.Sprintf(
					"Action: %T{ I_routeActionV4Tag: %d, Forward: %s }",
					r.Route.Action,
					r.Route.Action.I_routeActionV4Tag,
					func() string {
						switch r.Route.Action.I_routeActionV4Tag {
						case fnetRoutes.RouteActionV4Forward:
							return fmt.Sprintf(
								"%T{ OutboundInterface: %d, NextHop: %s } }",
								r.Route.Action,
								r.Route.Action.I_routeActionV4Tag,
								func() string {
									if r.Route.Action.Forward.NextHop == nil {
										return "nil"
									} else {
										return tcpip.AddrFrom4Slice(
											r.Route.Action.Forward.NextHop.Addr[:]).String()
									}
								}())
						default:
							return fmt.Sprintf("%#v", r.Route.Action.Forward)
						}
					}())
			}(),
			r.Route.Properties)
	}

	fmtInstalledRouteV6 := func(r fnetRoutes.InstalledRouteV6) string {
		dst := fidlconv.ToTCPIPSubnet(net.Subnet{
			Addr:      net.IpAddressWithIpv6(r.Route.Destination.Addr),
			PrefixLen: r.Route.Destination.PrefixLen,
		})
		return fmt.Sprintf(
			"InstalledRoute: %T{ EffectiveProperties: %#v, Route: %T{ Destination: %s, %s, Properties: %#v } }",
			r,
			r.EffectiveProperties,
			r.Route,
			dst,
			func() string {
				return fmt.Sprintf(
					"Action: %T{ I_routeActionV6Tag: %d, Forward: %s }",
					r.Route.Action,
					r.Route.Action.I_routeActionV6Tag,
					func() string {
						switch r.Route.Action.I_routeActionV6Tag {
						case fnetRoutes.RouteActionV6Forward:
							return fmt.Sprintf(
								"%T{ OutboundInterface: %d, NextHop: %s } }",
								r.Route.Action,
								r.Route.Action.I_routeActionV6Tag,
								func() string {
									if r.Route.Action.Forward.NextHop == nil {
										return "nil"
									} else {
										return tcpip.AddrFrom16Slice(
											r.Route.Action.Forward.NextHop.Addr[:]).String()
									}
								}())
						default:
							return fmt.Sprintf("%#v", r.Route.Action.Forward)
						}
					}())
			}(),
			r.Route.Properties)
	}

	return fmt.Sprintf(
		"%T{ version: %d ipv4_event: %s, ipv6_event: %s}",
		e,
		e.version,
		func() string {
			switch e.version {
			case routetypes.IPv4:
				return fmt.Sprintf("%T{ I_eventV4Tag: %d, %s }",
					e.ipv4_event, e.ipv4_event.I_eventV4Tag, func() string {
						switch e.ipv4_event.I_eventV4Tag {
						case fnetRoutes.EventV4Idle:
							return "Idle: {}{}"
						case fnetRoutes.EventV4Existing:
							return fmt.Sprintf("Existing: %s",
								fmtInstalledRouteV4(e.ipv4_event.Existing))
						case fnetRoutes.EventV4Added:
							return fmt.Sprintf("Added: %s",
								fmtInstalledRouteV4(e.ipv4_event.Added))
						case fnetRoutes.EventV4Removed:
							return fmt.Sprintf("Removed: %s",
								fmtInstalledRouteV4(e.ipv4_event.Removed))
						default:
							return "Unknown"
						}
					}())

			default:
				return "<unset>"
			}
		}(),
		func() string {
			switch e.version {
			case routetypes.IPv6:
				return fmt.Sprintf("%T{ I_eventV6Tag: %d, %s }",
					e.ipv6_event, e.ipv6_event.I_eventV6Tag, func() string {
						switch e.ipv6_event.I_eventV6Tag {
						case fnetRoutes.EventV6Idle:
							return "Idle: {}{}"
						case fnetRoutes.EventV6Existing:
							return fmt.Sprintf("Existing: %s",
								fmtInstalledRouteV6(e.ipv6_event.Existing))
						case fnetRoutes.EventV6Added:
							return fmt.Sprintf("Added: %s",
								fmtInstalledRouteV6(e.ipv6_event.Added))
						case fnetRoutes.EventV6Removed:
							return fmt.Sprintf("Removed: %s",
								fmtInstalledRouteV6(e.ipv6_event.Removed))
						default:
							return "Unknown"
						}
					}())

			default:
				return "<unset>"
			}
		}(),
	)
}

// installedRouteComparisonKey is an alternative to InstalledRoute whose boxed members
// are defined out-of-band at the top level. Two installedRouteComparisonKeys can be
// checked for equality, whereas two InstalledRoutes cannot (since their boxed
// members are behind a pointer, leading to address comparisons, not value
// comparisons).
type installedRouteComparisonKey struct {
	fidlconv.RouteComparisonKey
	Version                         routetypes.IpProtoTag
	EffectiveRoutePropertiesPresent bool
	EffectiveRouteProperties        fnetRoutes.EffectiveRouteProperties
	TableId                         uint32
}

// toInstalledRouteComparisonKey converts an InstalledRoute to an installedRouteComparisonKey.
func toInstalledRouteComparisonKey(r fidlconv.InstalledRoute) (installedRouteComparisonKey, error) {
	key, err := r.ToRouteComparisonKey()
	if err != nil {
		return installedRouteComparisonKey{}, err
	}

	present, props, tableId := func() (bool, fnetRoutes.EffectiveRouteProperties, uint32) {
		switch r.Version {
		case routetypes.IPv4:
			return r.V4.EffectivePropertiesPresent, r.V4.EffectiveProperties, r.V4.TableId
		case routetypes.IPv6:
			return r.V6.EffectivePropertiesPresent, r.V6.EffectiveProperties, r.V6.TableId
		default:
			panic(fmt.Sprintf("unknown IP Version tag %d", r.Version))
		}
	}()

	return installedRouteComparisonKey{
		RouteComparisonKey:              key,
		Version:                         r.Version,
		EffectiveRoutePropertiesPresent: present,
		EffectiveRouteProperties:        props,
		TableId:                         tableId,
	}, nil
}

// fromInstalledRouteComparisonKey converts an installedRouteComparisonKey to an InstalledRoute.
func fromInstalledRouteComparisonKey(r installedRouteComparisonKey) (fidlconv.InstalledRoute, error) {
	var result fidlconv.InstalledRoute
	result.Version = r.Version

	switch r.Version {
	case routetypes.IPv4:
		route, err := fidlconv.ToFidlRouteV4(r.V4)
		if err != nil {
			return result, err
		}
		result.V4.SetRoute(route)
		result.V4.EffectivePropertiesPresent = r.EffectiveRoutePropertiesPresent
		result.V4.EffectiveProperties = r.EffectiveRouteProperties
		result.V4.SetTableId(r.TableId)
	case routetypes.IPv6:
		route, err := fidlconv.ToFidlRouteV6(r.V6)
		if err != nil {
			return result, err
		}
		result.V6.SetRoute(route)
		result.V6.EffectivePropertiesPresent = r.EffectiveRoutePropertiesPresent
		result.V6.EffectiveProperties = r.EffectiveRouteProperties
		result.V6.SetTableId(r.TableId)
	default:
		panic(fmt.Sprintf("unknown IP Version tag %d", r.Version))
	}
	return result, nil
}
