// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netstack

import (
	// #include "zircon/process.h"
	"C"

	"syscall/zx"
	"syscall/zx/fidl"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/fidlconv"
	"go.fuchsia.dev/fuchsia/src/lib/component"
	syslog "go.fuchsia.dev/fuchsia/src/lib/syslog/go"

	fuchsianet "fidl/fuchsia/net"
	interfacesadmin "fidl/fuchsia/net/interfaces/admin"
	"fidl/fuchsia/net/root"
	routesadmin "fidl/fuchsia/net/routes/admin"

	"gvisor.dev/gvisor/pkg/tcpip"
)

var _ root.InterfacesWithCtx = (*rootInterfacesImpl)(nil)

type rootInterfacesImpl struct {
	ns *Netstack
}

func (ci *rootInterfacesImpl) GetAdmin(_ fidl.Context, nicid uint64, request interfacesadmin.ControlWithCtxInterfaceRequest) error {
	{
		nicid := tcpip.NICID(nicid)
		nicInfo, ok := ci.ns.stack.NICInfo()[nicid]
		if !ok {
			if err := component.CloseWithEpitaph(request.Channel, zx.ErrNotFound); err != nil {
				_ = syslog.WarnTf(root.InterfacesName, "GetAdmin(%d) close error: %s", nicid, err)
			}
			return nil
		}

		ifs := nicInfo.Context.(*ifState)
		ifs.addAdminConnection(request, false /* strong */)
		return nil
	}
}

func (ci *rootInterfacesImpl) GetMac(_ fidl.Context, nicid uint64) (root.InterfacesGetMacResult, error) {
	if nicInfo, ok := ci.ns.stack.NICInfo()[tcpip.NICID(nicid)]; ok {
		var response root.InterfacesGetMacResponse
		if linkAddress := nicInfo.LinkAddress; len(linkAddress) != 0 {
			mac := fidlconv.ToNetMacAddress(linkAddress)
			response.Mac = &mac
		}
		return root.InterfacesGetMacResultWithResponse(response), nil
	}
	return root.InterfacesGetMacResultWithErr(root.InterfacesGetMacErrorNotFound), nil
}

var _ root.RoutesV4WithCtx = (*rootRoutesV4Impl)(nil)

type rootRoutesV4Impl struct {
	ns *Netstack
}

func (r *rootRoutesV4Impl) GlobalRouteSet(ctx_ fidl.Context, request routesadmin.RouteSetV4WithCtxInterfaceRequest) error {
	return bindV4RouteSet(request.Channel, makeGlobalRouteSet[fuchsianet.Ipv4Address](r.ns))
}

var _ root.RoutesV6WithCtx = (*rootRoutesV6Impl)(nil)

type rootRoutesV6Impl struct {
	ns *Netstack
}

func (r *rootRoutesV6Impl) GlobalRouteSet(ctx_ fidl.Context, request routesadmin.RouteSetV6WithCtxInterfaceRequest) error {
	return bindV6RouteSet(request.Channel, makeGlobalRouteSet[fuchsianet.Ipv6Address](r.ns))
}
