// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package routetypes

import (
	"fmt"
	"strings"

	"gvisor.dev/gvisor/pkg/tcpip"
)

type IpProtoTag uint32

const (
	_ IpProtoTag = iota
	IPv4
	IPv6
)

type Action uint32

const (
	ActionDeleteAll Action = iota
	ActionDeleteDynamic
	ActionDisableStatic
	ActionEnableStatic
)

// Metric is the metric used for sorting the route table. It acts as a
// priority with a lower value being better.
type Metric uint32

// Preference is the preference for a route.
type Preference int

const (
	// LowPreference indicates that a route has a low preference.
	LowPreference Preference = iota

	// MediumPreference indicates that a route has a medium (default)
	// preference.
	MediumPreference

	// HighPreference indicates that a route has a high preference.
	HighPreference
)

// ExtendedRoute is a single route that contains the standard tcpip.Route plus
// additional attributes.
type ExtendedRoute struct {
	// Route used to build the route table to be fed into the
	// gvisor.dev/gvisor/pkg lib.
	Route tcpip.Route

	// Prf is the preference of the route when comparing routes to the same
	// destination.
	Prf Preference

	// Metric acts as a tie-breaker when comparing otherwise identical routes.
	Metric Metric

	// MetricTracksInterface is true when the metric tracks the metric of the
	// interface for this route. This means when the interface metric changes, so
	// will this route's metric. If false, the metric is static and only changed
	// explicitly by API.
	MetricTracksInterface bool

	// Dynamic marks a route as being obtained via DHCP. Such routes are removed
	// from the table when the interface goes down, vs. just being disabled.
	Dynamic bool

	// Enabled marks a route as inactive, i.e., its interface is down and packets
	// must not use this route.
	// Disabled routes are omitted when building the route table for the
	// Netstack lib.
	// This flag is used with non-dynamic routes (i.e., statically added routes)
	// to keep them in the table while their interface is down.
	Enabled bool

	// OwningSets contains references to the route sets that "own" / reference
	// this route.
	OwningSets map[*RouteSetId]struct{}
}

type RoutingChangeTag uint32

const (
	_ RoutingChangeTag = iota
	RouteAdded
	RouteRemoved
)

// A union type abstracting over the possible changes to the routing table.
type RoutingTableChange struct {
	Change RoutingChangeTag
	Route  ExtendedRoute
}

// Match matches the given address against this route.
func (er *ExtendedRoute) Match(addr tcpip.Address) bool {
	return er.Route.Destination.Contains(addr)
}

func (er *ExtendedRoute) String() string {
	var out strings.Builder
	fmt.Fprintf(&out, "%s", er.Route)
	if er.MetricTracksInterface {
		fmt.Fprintf(&out, " metric[if] %d", er.Metric)
	} else {
		fmt.Fprintf(&out, " metric[static] %d", er.Metric)
	}
	if er.Dynamic {
		fmt.Fprintf(&out, " (dynamic)")
	} else {
		fmt.Fprintf(&out, " (static)")
	}
	if !er.Enabled {
		fmt.Fprintf(&out, " (disabled)")
	}
	return out.String()
}

func (er *ExtendedRoute) IsMemberOfRouteSet(id *RouteSetId) bool {
	if id.IsGlobal() {
		return true
	}

	_, ok := er.OwningSets[id]
	return ok
}

type SendRoutingTableChangeCb func(RoutingTableChange)

// RouteSetId as a value means nothing, but pointers to RouteSetId are used as
// unique identifiers of route sets.
//
// The byte is there to make RouteSetId non-zero sized. As per the Go spec,
// pointers to empty structs are not guaranteed to be unique.  See:
// https://go.dev/ref/spec#Size_and_alignment_guarantees.
type RouteSetId struct{ byte }

func (id *RouteSetId) IsGlobal() bool {
	return id == nil
}

func GlobalRouteSet() *RouteSetId {
	return nil
}
