// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package routes

import (
	"errors"
	"fmt"
	"net"
	"sort"
	"strings"

	syslog "go.fuchsia.dev/fuchsia/src/lib/syslog/go"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/fidlconv"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routetypes"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/sync"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/util"

	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/header"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
)

const tag = "routes"

var (
	ErrNoSuchRoute = errors.New("no such route")
	ErrNoSuchNIC   = errors.New("no such NIC")
)

type ExtendedRouteTable []routetypes.ExtendedRoute

func (rt ExtendedRouteTable) String() string {
	var out strings.Builder
	for _, r := range rt {
		fmt.Fprintf(&out, "%s\n", &r)
	}
	return out.String()
}

type RoutingTableChange = routetypes.RoutingTableChange
type SendRoutingTableChangeCb func(RoutingTableChange)

// RoutingChangeSender queues up pending changes to the RouteTable, and
// sends these changes to clients of the `fuchsia.net.routes.Watcher` protocols
// once the events changes are committed into gVisor.
type routingChangeSender struct {
	// A buffer containing accumulated changes to the routing table that have
	// not yet been sent into `RoutingChangesChan`.
	pendingChanges []RoutingTableChange
	// A channel to forward changes in routing state to the clients of
	// fuchsia.net.routes Watcher protocols.
	onChangeCb SendRoutingTableChangeCb
}

// queueChange queues a RoutingTableChange in the RoutingChangeSender.
func (s *routingChangeSender) queueChange(c RoutingTableChange) {
	if s.onChangeCb != nil {
		s.pendingChanges = append(s.pendingChanges, c)
	}
}

// flushChanges drains the pendingChanges by sending each RoutingTableChange
// into the RoutingChangesChan.
func (s *routingChangeSender) flushChanges() {
	if s.onChangeCb == nil {
		return
	}
	for _, c := range s.pendingChanges {
		s.onChangeCb(c)
	}
	s.pendingChanges = nil
}

// RouteTable implements a sorted list of extended routes that is used to build
// the Netstack lib route table.
type RouteTable struct {
	sync.Mutex
	routes ExtendedRouteTable
	sender routingChangeSender
}

func NewRouteTableWithOnChangeCallback(cb SendRoutingTableChangeCb) RouteTable {
	return RouteTable{
		sender: routingChangeSender{
			onChangeCb: cb,
		},
	}
}

// For debugging.
func (rt *RouteTable) dumpLocked() {
	if rt == nil {
		syslog.VLogTf(syslog.TraceVerbosity, tag, "Current Route Table:<nil>")
	} else {
		syslog.VLogTf(syslog.TraceVerbosity, tag, "Current Route Table:\n%s", rt.routes)
	}
}

// HasDefaultRoutes returns whether an interface has default IPv4/IPv6 routes.
func (rt *RouteTable) HasDefaultRouteLocked(nicid tcpip.NICID) (bool, bool) {
	var v4, v6 bool
	for _, er := range rt.routes {
		if er.Route.NIC == nicid && er.Enabled {
			if er.Route.Destination.Equal(header.IPv4EmptySubnet) {
				v4 = true
			} else if er.Route.Destination.Equal(header.IPv6EmptySubnet) {
				v6 = true
			}
		}
	}
	return v4, v6
}

// For testing.
func (rt *RouteTable) Set(r []routetypes.ExtendedRoute) {
	rt.Lock()
	defer rt.Unlock()
	rt.routes = append([]routetypes.ExtendedRoute(nil), r...)
}

type AddResult struct {
	NewlyAddedToTable bool
	NewlyAddedToSet   bool
}

func (rt *RouteTable) AddRouteLocked(route tcpip.Route, prf routetypes.Preference, metric routetypes.Metric, tracksInterface, dynamic, enabled, replaceMatchingGvisorRoutes bool, addingSet *routetypes.RouteSetId) AddResult {
	syslog.VLogTf(syslog.DebugVerbosity, tag, "RouteTable:Adding route %s with prf=%d metric=%d, tracksInterface=%t, dynamic=%t, enabled=%t, replaceMatchingGvisorRoutes=%t", route, prf, metric, tracksInterface, dynamic, enabled, replaceMatchingGvisorRoutes)

	type foldResult int
	const (
		neitherPresentInTableNorInSet foldResult = iota
		presentInTableButNotInSet
		presentInTableAndInSet
	)

	newEr := routetypes.ExtendedRoute{
		Route:                 route,
		Prf:                   prf,
		Metric:                metric,
		MetricTracksInterface: tracksInterface,
		Dynamic:               dynamic,
		Enabled:               enabled,
		OwningSets: map[*routetypes.RouteSetId]struct{}{
			addingSet: {},
		},
	}

	incomingKey, err := fidlconv.ToRouteComparisonKey(newEr)
	if err != nil {
		_ = syslog.ErrorTf(tag, "RouteTable:conversion of route-to-be-added %#v to comparison key failed: %s", newEr, err)
		return AddResult{
			NewlyAddedToTable: false,
			NewlyAddedToSet:   false,
		}
	}

	result := foldMapRoutesLocked[foldResult](
		rt,
		func(er *routetypes.ExtendedRoute) (bool, foldResult) {
			existingKey, err := fidlconv.ToRouteComparisonKey(*er)
			if err != nil {
				panic(fmt.Sprintf("existing route in table %#v is invalid and thus cannot be converted to a comparison key: %s", *er, err))
			}
			if er.Route == route && replaceMatchingGvisorRoutes {
				// Remove the route from the table, because we're going to re-add it.
				return false, neitherPresentInTableNorInSet
			} else if existingKey == incomingKey {
				// Routes match. Make sure that the route is only treated as dynamic
				// if all instances of it are dynamic.
				er.Dynamic = er.Dynamic && newEr.Dynamic
				if !addingSet.IsGlobal() {
					if er.IsMemberOfRouteSet(addingSet) {
						return true, presentInTableAndInSet
					} else {
						er.OwningSets[addingSet] = struct{}{}
						return true, presentInTableButNotInSet
					}
				} else {
					// If the route set is global, then being present in the table is
					// equivalent to being present in the set.
					return true, presentInTableAndInSet
				}
			} else {
				// This route is unrelated, so we keep it.
				return true, neitherPresentInTableNorInSet
			}
		},
		func(a, b foldResult) foldResult {
			// If either foldResult indicates the route already exists in the table,
			// keep that one.
			if a != neitherPresentInTableNorInSet && b != neitherPresentInTableNorInSet {
				panic(fmt.Sprintf("duplicate routing table entries in %s", rt.routes))
			}
			if a != neitherPresentInTableNorInSet {
				return a
			}
			return b
		},
		neitherPresentInTableNorInSet,
	)

	switch result {
	case presentInTableButNotInSet:
		return AddResult{
			NewlyAddedToTable: false,
			NewlyAddedToSet:   true,
		}
	case presentInTableAndInSet:
		return AddResult{
			NewlyAddedToTable: false,
			NewlyAddedToSet:   false,
		}
	case neitherPresentInTableNorInSet:
	default:
		panic(fmt.Sprintf("unknown foldResult value: %d", result))
	}

	// Find the target position for the new route in the table so it remains
	// sorted.
	targetIdx := sort.Search(len(rt.routes), func(i int) bool {
		return Less(&newEr, &rt.routes[i])
	})
	// Extend the table by adding the new route at the end, then move it into its
	// proper place.
	rt.routes = append(rt.routes, newEr)
	if targetIdx < len(rt.routes)-1 {
		copy(rt.routes[targetIdx+1:], rt.routes[targetIdx:])
		rt.routes[targetIdx] = newEr
	}

	rt.dumpLocked()

	rt.sender.queueChange(RoutingTableChange{
		Change: routetypes.RouteAdded,
		Route:  newEr,
	})

	return AddResult{
		NewlyAddedToTable: true,
		NewlyAddedToSet:   true,
	}
}

// AddRoute inserts the given route to the table in a sorted fashion. If the
// route already exists, it simply updates that route's preference, metric,
// dynamic, and enabled fields.
func (rt *RouteTable) AddRoute(route tcpip.Route, prf routetypes.Preference, metric routetypes.Metric, tracksInterface, dynamic, enabled bool, addingSet *routetypes.RouteSetId) AddResult {
	rt.Lock()
	defer rt.Unlock()

	return rt.AddRouteLocked(route, prf, metric, tracksInterface, dynamic, enabled, true /* replaceMatchingGvisorRoutes */, addingSet)
}

func (rt *RouteTable) DelRouteLocked(route tcpip.Route, deletingSet *routetypes.RouteSetId) []routetypes.ExtendedRoute {
	return rt.delRouteLocked(route, deletingSet)
}

func (rt *RouteTable) delRouteLocked(route tcpip.Route, deletingSet *routetypes.RouteSetId) []routetypes.ExtendedRoute {
	syslog.VLogTf(syslog.DebugVerbosity, tag, "RouteTable:Deleting route %s", route)

	routesDeleted := foldMapRoutesLocked[[]routetypes.ExtendedRoute](
		rt,
		func(er *routetypes.ExtendedRoute) (bool, []routetypes.ExtendedRoute) {
			if er.Route.Destination == route.Destination && er.Route.NIC == route.NIC {
				deletingSetMatches := !deletingSet.IsGlobal() && er.IsMemberOfRouteSet(deletingSet)

				// Match any route if Gateway is empty.
				if route.Gateway.Len() == 0 ||
					er.Route.Gateway == route.Gateway {
					if deletingSetMatches {
						delete(er.OwningSets, deletingSet)
					}
					if deletingSet.IsGlobal() || deletingSetMatches && len(er.OwningSets) == 0 {
						return false, []routetypes.ExtendedRoute{*er}
					}
				}
			}
			return true, nil
		},
		func(a, b []routetypes.ExtendedRoute) []routetypes.ExtendedRoute {
			return append(a, b...)
		},
		nil,
	)

	return routesDeleted
}

// foldMapRoutesLocked runs f on every route in the RouteTable. If f returns
// false for a given route, the route is removed; otherwise, the route is
// preserved (including modifications to the pointed-to route).
//
// The second return value of f is aggregated for each route using agg, and
// foldMapRoutesLocked returns the aggregate value.
func foldMapRoutesLocked[T any](rt *RouteTable, f func(er *routetypes.ExtendedRoute) (bool, T), agg func(a T, b T) T, init T) T {
	var routesDeleted []routetypes.ExtendedRoute
	oldTable := rt.routes
	rt.routes = oldTable[:0]
	if cap(oldTable) > 2*len(oldTable) {
		// Remove excess route table capacity instead of reusing old capacity.
		rt.routes = make([]routetypes.ExtendedRoute, 0, len(oldTable))
	}

	for _, er := range oldTable {
		keepEr, meta := f(&er)
		init = agg(meta, init)
		if keepEr {
			rt.routes = append(rt.routes, er)
		} else {
			routesDeleted = append(routesDeleted, er)
		}
	}

	// Zero out unused entries in the routes slice so that they can be garbage collected.
	deadRoutes := rt.routes[len(rt.routes):cap(rt.routes)]
	for i := range deadRoutes {
		deadRoutes[i] = routetypes.ExtendedRoute{}
	}

	rt.dumpLocked()

	for _, er := range routesDeleted {
		rt.sender.queueChange(RoutingTableChange{
			Change: routetypes.RouteRemoved,
			Route:  er,
		})
	}

	return init
}

type DelResult struct {
	NewlyRemovedFromTable bool
	NewlyRemovedFromSet   bool
}

// DelRouteExactMatchLocked deletes a route from the routing table if there is
// an extended route with the same tcpip.Route, preference, and metric.
func (rt *RouteTable) DelRouteExactMatchLocked(route tcpip.Route, prf routetypes.Preference, metric routetypes.Metric, tracksInterface bool, set *routetypes.RouteSetId) DelResult {
	syslog.DebugTf(tag, "RouteTable:Deleting route %s requiring exact match for prf=%d metric=%d tracksInterface=%t", route, prf, metric, tracksInterface)

	delEr := routetypes.ExtendedRoute{
		Route:                 route,
		Prf:                   prf,
		Metric:                metric,
		MetricTracksInterface: tracksInterface,
	}

	incomingKey, err := fidlconv.ToRouteComparisonKey(delEr)
	if err != nil {
		_ = syslog.ErrorTf(tag, "RouteTable:conversion of route-to-be-deleted %#v to comparison key failed: %s", delEr, err)
		return DelResult{
			NewlyRemovedFromTable: false,
			NewlyRemovedFromSet:   false,
		}
	}

	delResult := foldMapRoutesLocked[DelResult](
		rt,
		func(er *routetypes.ExtendedRoute) (bool, DelResult) {
			existingKey, err := fidlconv.ToRouteComparisonKey(*er)
			if err != nil {
				panic(fmt.Sprintf("existing route in table %#v is invalid and thus cannot be converted to a comparison key: %s", *er, err))
			}

			keepInTable := true
			removedFromSet := false
			if existingKey == incomingKey {
				if er.IsMemberOfRouteSet(set) || set.IsGlobal() {
					// The global route set is intentionally never in the owning
					// sets for a route.
					if !set.IsGlobal() {
						delete(er.OwningSets, set)
					}
					removedFromSet = true

					if len(er.OwningSets) == 0 || set.IsGlobal() {
						keepInTable = false
					}
				}
			}

			return keepInTable, DelResult{
				NewlyRemovedFromTable: !keepInTable,
				NewlyRemovedFromSet:   removedFromSet,
			}
		},
		func(a, b DelResult) DelResult {
			return DelResult{
				NewlyRemovedFromTable: a.NewlyRemovedFromTable || b.NewlyRemovedFromTable,
				NewlyRemovedFromSet:   a.NewlyRemovedFromSet || b.NewlyRemovedFromSet,
			}
		},
		DelResult{},
	)

	if !delResult.NewlyRemovedFromSet {
		_ = syslog.DebugTf(tag, "did not find exact match for route=%#v prf=%#v metric=%#v tracksInterface=%#v set=%#v; routes: %#v", route, prf, metric, tracksInterface, set, rt)
	}
	return delResult
}

// DelRoute removes matching routes from the route table, returning them.
func (rt *RouteTable) DelRoute(route tcpip.Route, deletingSet *routetypes.RouteSetId) []routetypes.ExtendedRoute {
	rt.Lock()
	defer rt.Unlock()

	return rt.DelRouteLocked(route, deletingSet)
}

// DelRouteSetLocked closes a user routeSet and returns any routes deleted from the route table as a
// result.  If the routeSet is global, no changes are made.
func (rt *RouteTable) DelRouteSetLocked(routeSet *routetypes.RouteSetId) []routetypes.ExtendedRoute {
	if routeSet.IsGlobal() {
		return nil
	}

	routesDeleted := foldMapRoutesLocked[[]routetypes.ExtendedRoute](
		rt,
		func(er *routetypes.ExtendedRoute) (bool, []routetypes.ExtendedRoute) {
			if er.IsMemberOfRouteSet(routeSet) {
				delete(er.OwningSets, routeSet)
				if len(er.OwningSets) == 0 {
					return false, []routetypes.ExtendedRoute{*er}
				}
			}
			_ = syslog.DebugTf(tag, "DelRouteSetLocked(%#v) did not delete %s with OwningSets %#v", routeSet, er, er.OwningSets)
			return true, nil
		},
		func(a, b []routetypes.ExtendedRoute) []routetypes.ExtendedRoute {
			return append(a, b...)
		},
		nil,
	)

	_ = syslog.DebugTf(tag, "DelRouteSetLocked(%#v) deleted %d routes", routeSet, len(routesDeleted))

	return routesDeleted
}

// GetExtendedRouteTable returns a copy of the current extended route table.
func (rt *RouteTable) GetExtendedRouteTable() ExtendedRouteTable {
	rt.Lock()
	defer rt.Unlock()

	rt.dumpLocked()

	return append([]routetypes.ExtendedRoute(nil), rt.routes...)
}

// UpdateStack updates stack with the current route table.
func (rt *RouteTable) UpdateStackLocked(stack *stack.Stack, onUpdateSucceeded func()) {
	t := make([]tcpip.Route, 0, len(rt.routes))
	for _, er := range rt.routes {
		if er.Enabled {
			t = append(t, er.Route)
		}
	}
	stack.SetRouteTable(t)

	_ = syslog.VLogTf(syslog.DebugVerbosity, tag, "UpdateStack route table: %+v", t)
	onUpdateSucceeded()

	// Notify clients of `fuchsia.net.routes/Watcher` of the changes.
	rt.sender.flushChanges()
}

// UpdateStack updates stack with the current route table.
func (rt *RouteTable) UpdateStack(stack *stack.Stack, onUpdateSucceeded func()) {
	rt.Lock()
	defer rt.Unlock()

	rt.UpdateStackLocked(stack, onUpdateSucceeded)
}

func (rt *RouteTable) UpdateRoutesByInterfaceLocked(nicid tcpip.NICID, action routetypes.Action) {
	syslog.VLogTf(syslog.DebugVerbosity, tag, "RouteTable:Update route table for routes to nic-%d with action:%d", nicid, action)

	oldTable := rt.routes
	rt.routes = oldTable[:0]
	if cap(oldTable) > 2*len(oldTable) {
		// Remove excess route table capacity instead of reusing old capacity.
		rt.routes = make([]routetypes.ExtendedRoute, 0, len(oldTable))
	}
	for _, er := range oldTable {
		if er.Route.NIC == nicid {
			switch action {
			case routetypes.ActionDeleteAll:
				rt.sender.queueChange(RoutingTableChange{
					Change: routetypes.RouteRemoved,
					Route:  er,
				})
				continue // delete
			case routetypes.ActionDeleteDynamic:
				if er.Dynamic {
					rt.sender.queueChange(RoutingTableChange{
						Change: routetypes.RouteRemoved,
						Route:  er,
					})
					continue // delete
				}
			case routetypes.ActionDisableStatic:
				if !er.Dynamic {
					er.Enabled = false
				}
			case routetypes.ActionEnableStatic:
				if !er.Dynamic {
					er.Enabled = true
				}
			}
		}
		// Keep.
		rt.routes = append(rt.routes, er)
	}

	// Zero out unused entries in the routes slice so that they can be garbage collected.
	deadRoutes := rt.routes[len(rt.routes):cap(rt.routes)]
	for i := range deadRoutes {
		deadRoutes[i] = routetypes.ExtendedRoute{}
	}

	rt.sortRouteTableLocked()

	rt.dumpLocked()
}

// UpdateRoutesByInterface applies an action to the routes pointing to an interface.
func (rt *RouteTable) UpdateRoutesByInterface(nicid tcpip.NICID, action routetypes.Action) {
	rt.Lock()
	defer rt.Unlock()

	rt.UpdateRoutesByInterfaceLocked(nicid, action)
}

// FindNIC returns the NIC-ID that the given address is routed on. This requires
// an exact route match, i.e. no default route.
func (rt *RouteTable) FindNIC(addr tcpip.Address) (tcpip.NICID, error) {
	rt.Lock()
	defer rt.Unlock()

	for _, er := range rt.routes {
		// Ignore default routes.
		if util.IsAny(er.Route.Destination.ID()) {
			continue
		}
		if er.Match(addr) && er.Route.NIC > 0 {
			return er.Route.NIC, nil
		}
	}
	return 0, ErrNoSuchNIC
}

// GetNICsWithDefaultRoutesLocked returns the set of NICs that have default
// IPv4 routes.
func (rt *RouteTable) GetNICsWithDefaultV4RoutesLocked() map[tcpip.NICID]struct{} {
	set := make(map[tcpip.NICID]struct{})
	for _, er := range rt.routes {
		if er.Route.Destination.Equal(header.IPv4EmptySubnet) {
			set[er.Route.NIC] = struct{}{}
		}
	}
	return set
}

// GetNICsWithDefaultRoutesLocked returns the set of NICs that have default
// IPv6 routes.
func (rt *RouteTable) GetNICsWithDefaultV6RoutesLocked() map[tcpip.NICID]struct{} {
	set := make(map[tcpip.NICID]struct{})
	for _, er := range rt.routes {
		if er.Route.Destination.Equal(header.IPv6EmptySubnet) {
			set[er.Route.NIC] = struct{}{}
		}
	}
	return set
}

func (rt *RouteTable) sortRouteTableLocked() {
	sort.SliceStable(rt.routes, func(i, j int) bool {
		return Less(&rt.routes[i], &rt.routes[j])
	})
}

// Less compares two routes and returns which one should appear earlier in the
// route table.
func Less(ei, ej *routetypes.ExtendedRoute) bool {
	ri, rj := ei.Route, ej.Route
	riDest, rjDest := ri.Destination.ID(), rj.Destination.ID()

	// Loopback routes before non-loopback ones.
	// (as a workaround for github.com/google/gvisor/issues/1169).
	if riIsLoop, rjIsLoop := net.IP(riDest.AsSlice()).IsLoopback(), net.IP(rjDest.AsSlice()).IsLoopback(); riIsLoop != rjIsLoop {
		return riIsLoop
	}

	// Non-default before default one.
	if riAny, rjAny := util.IsAny(riDest), util.IsAny(rjDest); riAny != rjAny {
		return !riAny
	}

	// IPv4 before IPv6 (arbitrary choice).
	if riLen, rjLen := riDest.Len(), rjDest.Len(); riLen != rjLen {
		return riLen == header.IPv4AddressSize
	}

	// Longer prefix wins.
	if riPrefix, rjPrefix := ri.Destination.Prefix(), rj.Destination.Prefix(); riPrefix != rjPrefix {
		return riPrefix > rjPrefix
	}

	// On-link wins.
	if riOnLink, rjOnLink := ri.Gateway.Len() == 0, rj.Gateway.Len() == 0; riOnLink != rjOnLink {
		return riOnLink
	}

	// Higher preference wins.
	if ei.Prf != ej.Prf {
		return ei.Prf > ej.Prf
	}

	// Lower metrics wins.
	if ei.Metric != ej.Metric {
		return ei.Metric < ej.Metric
	}

	// Everything that matters is the same. At this point we still need a
	// deterministic way to tie-break. First go by destination IPs (lower wins),
	// finally use the NIC.
	riDestAsSlice := riDest.AsSlice()
	rjDestAsSlice := rjDest.AsSlice()
	for i := 0; i < riDest.Len(); i++ {
		if riDestAsSlice[i] != rjDestAsSlice[i] {
			return riDestAsSlice[i] < rjDestAsSlice[i]
		}
	}

	// Same prefix and destination IPs (e.g. loopback IPs), use NIC as final
	// tie-breaker.
	return ri.NIC < rj.NIC
}
