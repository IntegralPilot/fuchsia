// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netstack

import (
	"context"
	"errors"
	"fmt"
	"syscall/zx"
	"syscall/zx/fidl"
	"time"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/fidlconv"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/link/netdevice"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routetypes"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/sync"
	"go.fuchsia.dev/fuchsia/src/lib/component"
	syslog "go.fuchsia.dev/fuchsia/src/lib/syslog/go"

	"fidl/fuchsia/hardware/network"
	"fidl/fuchsia/net"
	"fidl/fuchsia/net/interfaces"
	"fidl/fuchsia/net/interfaces/admin"
	"fidl/fuchsia/net/resources"

	"gvisor.dev/gvisor/pkg/tcpip"
	"gvisor.dev/gvisor/pkg/tcpip/link/ethernet"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv4"
	"gvisor.dev/gvisor/pkg/tcpip/network/ipv6"
	"gvisor.dev/gvisor/pkg/tcpip/stack"
)

const (
	addressStateProviderName = "fuchsia.net.interfaces.admin/AddressStateProvider"
	controlName              = "fuchsia.net.interfaces.admin/Control"
	deviceControlName        = "fuchsia.net.interfaces.admin/DeviceControl"
)

var _ stack.AddressDispatcher = (*addressDispatcher)(nil)

type addressDispatcher struct {
	watcherDisp watcherAddressDispatcher
	mu          struct {
		sync.Mutex
		aspImpl *adminAddressStateProviderImpl
	}
}

func (ad *addressDispatcher) OnChanged(lifetimes stack.AddressLifetimes, state stack.AddressAssignmentState) {
	ad.watcherDisp.OnChanged(lifetimes, state)

	ad.mu.Lock()
	if ad.mu.aspImpl != nil {
		ad.mu.aspImpl.OnChanged(lifetimes, state)
	}
	ad.mu.Unlock()
}

func (ad *addressDispatcher) OnRemoved(reason stack.AddressRemovalReason) {
	ad.mu.Lock()
	if ad.mu.aspImpl != nil {
		ad.mu.aspImpl.OnRemoved(reason)
	}
	ad.mu.Unlock()

	ad.watcherDisp.OnRemoved(reason)
}

var _ admin.AddressStateProviderWithCtx = (*adminAddressStateProviderImpl)(nil)
var _ stack.AddressDispatcher = (*adminAddressStateProviderImpl)(nil)

type adminAddressStateProviderImpl struct {
	ns           *Netstack
	nicid        tcpip.NICID
	cancelServe  context.CancelFunc
	ready        chan struct{}
	protocolAddr tcpip.ProtocolAddress
	mu           struct {
		sync.Mutex
		eventProxy admin.AddressStateProviderEventProxy
		isHanging  bool
		// NB: state is the zero value while the address has been added but the
		// initial assignment state is unknown.
		state interfaces.AddressAssignmentState
		// NB: lastObserved is the zero value iff the client has yet to observe the
		// state for the first time.
		lastObserved interfaces.AddressAssignmentState
		// detached is set to true if Detach has been called on the channel, and will
		// result in the address not being removed when the client closes its end of
		// the channel.
		detached bool
		// removedReason is set to the reason why the address was removed. When this
		// is set, the address does not need to be removed when this protocol
		// terminates.
		removedReason stack.AddressRemovalReason
		// Indicates if the OnAddressAdded event was sent.
		sentAddedEvent bool
	}
	// The RouteSetId used to associate this address to any subnet route added
	// as a result of the add_subnet_route AddressParameters field.
	subnetRouteSetId routetypes.RouteSetId
	// The add_subnet_route AddressParameters field from when this address was
	// added to the stack.
	addSubnetRoute bool
}

func (pi *adminAddressStateProviderImpl) UpdateAddressProperties(_ fidl.Context, properties admin.AddressProperties) error {
	lifetimes := propertiesToLifetimes(properties)
	switch err := pi.ns.stack.SetAddressLifetimes(
		pi.nicid,
		pi.protocolAddr.AddressWithPrefix.Address,
		lifetimes,
	); err.(type) {
	case nil:
	case *tcpip.ErrUnknownNICID, *tcpip.ErrBadLocalAddress:
		// TODO(https://fxbug.dev/42176338): Upgrade to panic once we're guaranteed that we get here iff the address still exists.
		_ = syslog.WarnTf(addressStateProviderName, "SetAddressLifetimes(%d, %s, %#v) failed: %s",
			pi.nicid, pi.protocolAddr.AddressWithPrefix.Address, lifetimes, err)
	default:
		panic(fmt.Sprintf("SetAddressLifetimes(%d, %s, %#v) failed with unexpected error: %s",
			pi.nicid, pi.protocolAddr.AddressWithPrefix.Address, lifetimes, err))
	}
	return nil
}

func (pi *adminAddressStateProviderImpl) Detach(fidl.Context) error {
	pi.mu.Lock()
	defer pi.mu.Unlock()

	pi.mu.detached = true
	return nil
}

func (pi *adminAddressStateProviderImpl) WatchAddressAssignmentState(ctx fidl.Context) (interfaces.AddressAssignmentState, error) {
	pi.mu.Lock()
	defer pi.mu.Unlock()

	if pi.mu.isHanging {
		pi.cancelServe()
		return 0, errors.New("not allowed to call WatchAddressAssignmentState when a call is already in progress")
	}

	for {
		if pi.mu.lastObserved != pi.mu.state {
			state := pi.mu.state
			pi.mu.lastObserved = state
			syslog.DebugTf(addressStateProviderName, "NIC=%d address %+v observed state: %s", pi.nicid, pi.protocolAddr, state)
			return state, nil
		}

		pi.mu.isHanging = true
		pi.mu.Unlock()

		var err error
		select {
		case <-pi.ready:
		case <-ctx.Done():
			err = fmt.Errorf("cancelled: %w", ctx.Err())
		}

		pi.mu.Lock()
		pi.mu.isHanging = false
		if err != nil {
			return 0, err
		}
	}
}

var _ admin.ControlWithCtx = (*adminControlImpl)(nil)

type adminControlImpl struct {
	ns          *Netstack
	nicid       tcpip.NICID
	cancelServe context.CancelFunc
	syncRemoval bool
	doneChannel chan zx.Channel
	// TODO(https://fxbug.dev/42169142): encode owned, strong, and weak refs once
	// cloning Control is allowed.
	isStrongRef bool
}

func (ci *adminControlImpl) getNICContext() *ifState {
	nicInfo, ok := ci.ns.stack.NICInfo()[ci.nicid]
	if !ok {
		// All serving control channels must be canceled before removing NICs from
		// the stack, this is a violation of that invariant.
		panic(fmt.Sprintf("NIC %d not found", ci.nicid))
	}
	return nicInfo.Context.(*ifState)
}

func (ci *adminControlImpl) Enable(fidl.Context) (admin.ControlEnableResult, error) {
	wasEnabled, err := ci.getNICContext().setState(true /* enabled */)
	if err != nil {
		// The only known error path that causes this failure is failure from the
		// device layers, which all mean we're possible racing with shutdown.
		_ = syslog.Errorf("ifs.Up() failed (NIC %d): %s", ci.nicid, err)
		return admin.ControlEnableResult{}, err
	}

	return admin.ControlEnableResultWithResponse(
		admin.ControlEnableResponse{
			DidEnable: !wasEnabled,
		}), nil
}

func (ci *adminControlImpl) Disable(fidl.Context) (admin.ControlDisableResult, error) {
	wasEnabled, err := ci.getNICContext().setState(false /* enabled */)
	if err != nil {
		// The only known error path that causes this failure is failure from the
		// device layers, which all mean we're possible racing with shutdown.
		_ = syslog.Errorf("ifs.Down() failed (NIC %d): %s", ci.nicid, err)
		return admin.ControlDisableResult{}, err
	}

	return admin.ControlDisableResultWithResponse(
		admin.ControlDisableResponse{
			DidDisable: wasEnabled,
		}), nil
}

func (ci *adminControlImpl) Detach(fidl.Context) error {
	// Make it a weak ref but don't decrease the reference count. If this was a
	// strong ref, the interface will leak.
	//
	// TODO(https://fxbug.dev/42169142): Detach should only be allowed on OWNED refs
	// once we allow cloning Control.
	ci.isStrongRef = false
	return nil
}

func (ci *adminControlImpl) isLoopback() bool {
	nicInfo, ok := ci.ns.stack.NICInfo()[ci.nicid]
	if !ok {
		panic(fmt.Sprintf("Remove on NIC %d not present", ci.nicid))
	}
	return nicInfo.Flags.Loopback
}

func (ci *adminControlImpl) Remove(fidl.Context) (admin.ControlRemoveResult, error) {
	if ci.isLoopback() {
		return admin.ControlRemoveResultWithErr(admin.ControlRemoveErrorNotAllowed), nil
	}

	ci.syncRemoval = true
	ci.cancelServe()

	return admin.ControlRemoveResultWithResponse(admin.ControlRemoveResponse{}), nil
}

func (ci *adminControlImpl) GetAuthorizationForInterface(fidl.Context) (resources.GrantForInterfaceAuthorization, error) {
	nicInfo := ci.getNICContext()

	token, err := nicInfo.authorizationToken.Duplicate(zx.RightTransfer | zx.RightDuplicate)
	if err != nil {
		return resources.GrantForInterfaceAuthorization{}, err
	}

	return resources.GrantForInterfaceAuthorization{InterfaceId: uint64(nicInfo.nicid), Token: token}, nil
}

func propertiesToLifetimes(properties admin.AddressProperties) stack.AddressLifetimes {
	lifetimes := stack.AddressLifetimes{
		ValidUntil: tcpip.MonotonicTimeInfinite(),
	}
	if properties.HasValidLifetimeEnd() {
		lifetimes.ValidUntil = fidlconv.ToTCPIPMonotonicTime(zx.Time(properties.GetValidLifetimeEnd()))
	}
	if properties.HasPreferredLifetimeInfo() {
		switch preferred := properties.GetPreferredLifetimeInfo(); preferred.Which() {
		case interfaces.PreferredLifetimeInfoDeprecated:
			lifetimes.Deprecated = true
		case interfaces.PreferredLifetimeInfoPreferredUntil:
			lifetimes.PreferredUntil = fidlconv.ToTCPIPMonotonicTime(zx.Time(preferred.PreferredUntil))
		default:
			panic(fmt.Sprintf("unknown preferred lifetime info tag: %+v", preferred))
		}
	} else {
		lifetimes.PreferredUntil = tcpip.MonotonicTimeInfinite()
	}
	return lifetimes
}

func (pi *adminAddressStateProviderImpl) OnChanged(lifetimes stack.AddressLifetimes, state stack.AddressAssignmentState) {
	_ = syslog.DebugTf(addressStateProviderName, "NIC=%d addr=%s changed lifetimes=%#v state=%s",
		pi.nicid, pi.protocolAddr.AddressWithPrefix, lifetimes, state)

	pi.mu.Lock()
	defer pi.mu.Unlock()

	pi.mu.state = fidlconv.ToAddressAssignmentState(state)
	if pi.mu.lastObserved != pi.mu.state {
		syslog.DebugTf(addressStateProviderName, "NIC=%d address %+v state changed from %s to %s", pi.nicid, pi.protocolAddr.AddressWithPrefix, pi.mu.lastObserved, pi.mu.state)
		select {
		case pi.ready <- struct{}{}:
		default:
		}
	}
}

func (pi *adminAddressStateProviderImpl) Remove(fidl.Context) error {
	_ = syslog.DebugTf(addressStateProviderName, "NICID=%d removing address %s from NIC %d due to explicit removal request", pi.nicid, pi.protocolAddr.AddressWithPrefix, pi.nicid)

	pi.mu.Lock()
	prevRemovedReason := pi.mu.removedReason
	// If not already set, removedReason will be set when the address is removed
	// below by the synchronous callback to pi.OnRemoved.
	pi.mu.Unlock()

	if prevRemovedReason != 0 {
		return nil
	}

	nicInfo, ok := pi.ns.stack.NICInfo()[pi.nicid]
	if !ok {
		panic(fmt.Sprintf("NIC %d not found when removing %s", pi.nicid, pi.protocolAddr.AddressWithPrefix))
	}
	ifs := nicInfo.Context.(*ifState)
	switch status := ifs.removeAddress(pi.protocolAddr); status {
	case zx.ErrOk:
	case zx.ErrNotFound:
		// Normally, we'd expect that it's impossible to get NotFound while holding
		// an AddressStateProvider, as the existence of the ASP itself should
		// indicate that the address is present.
		// However, we could be racing with fuchsia.net.interfaces.admin.Control/RemoveAddress
		// (see adminControlImpl.RemoveAddress in this file).
		_ = syslog.ErrorTf(addressStateProviderName, "tried to remove address %s that was not found on NIC %d", pi.protocolAddr.AddressWithPrefix, pi.nicid)
	case zx.ErrBadState:
		_ = syslog.WarnTf(addressStateProviderName, "NIC %d removed when trying to remove address %s due to explicit removal request: %s", pi.nicid, pi.protocolAddr.AddressWithPrefix, status)
	default:
		panic(fmt.Sprintf("NICID=%d unknown error trying to remove address %s upon explicit removal request: %s", pi.nicid, pi.protocolAddr.AddressWithPrefix, status))
	}
	return nil
}

func (pi *adminAddressStateProviderImpl) sendOnAddressRemovedEventAndCancelServeLocked() {
	if err := pi.mu.eventProxy.OnAddressRemoved(fidlconv.ToAddressRemovalReason(pi.mu.removedReason)); err != nil {
		var zxError *zx.Error
		if !errors.As(err, &zxError) || (zxError.Status != zx.ErrPeerClosed && zxError.Status != zx.ErrBadHandle) {
			_ = syslog.WarnTf(addressStateProviderName, "NICID=%d failed to send OnAddressRemoved(%s) for %s: %s", pi.nicid, pi.mu.removedReason, pi.protocolAddr.AddressWithPrefix.Address, err)
		}
	}
	pi.cancelServe()
}

func (pi *adminAddressStateProviderImpl) OnRemoved(reason stack.AddressRemovalReason) {
	_ = syslog.DebugTf(addressStateProviderName, "NIC=%d addr=%s removed reason=%s", pi.nicid, pi.protocolAddr.AddressWithPrefix, reason)

	pi.mu.Lock()
	defer pi.mu.Unlock()

	pi.mu.removedReason = reason

	// If the OnAddressAdded event has not been sent yet, then that means the
	// address was removed while fuchsia.net.interfaces.admin/Control.AddAddress
	// operation is in-progress. This can happen when immediately after adding the
	// address to the core (gVisor) netstack but before the OnAddressAdded event
	// was sent (when no locks are held), DAD fails which triggers address
	// removal.
	if pi.mu.sentAddedEvent {
		pi.sendOnAddressRemovedEventAndCancelServeLocked()
	}
}

func (pi *adminAddressStateProviderImpl) cleanUpSubnetRoute() {
	if pi.addSubnetRoute {
		pi.ns.DelRouteSet(&pi.subnetRouteSetId)
	}
}

func (ci *adminControlImpl) AddAddress(_ fidl.Context, subnet net.Subnet, parameters admin.AddressParameters, request admin.AddressStateProviderWithCtxInterfaceRequest) error {
	protocolAddr := fidlconv.ToTCPIPProtocolAddress(subnet)
	addr := protocolAddr.AddressWithPrefix.Address

	ifs := ci.getNICContext()

	ctx, cancel := context.WithCancel(context.Background())
	impl := &adminAddressStateProviderImpl{
		ns:           ci.ns,
		nicid:        ci.nicid,
		ready:        make(chan struct{}, 1),
		cancelServe:  cancel,
		protocolAddr: protocolAddr,
	}
	impl.mu.Lock()
	impl.mu.eventProxy.Channel = request.Channel
	impl.mu.Unlock()

	addrDisp := &addressDispatcher{
		watcherDisp: watcherAddressDispatcher{
			nicid:        ifs.nicid,
			protocolAddr: protocolAddr,
			ch:           ifs.ns.interfaceEventChan,
		},
	}
	addrDisp.mu.Lock()
	addrDisp.mu.aspImpl = impl
	addrDisp.mu.Unlock()
	properties := stack.AddressProperties{
		Temporary: parameters.GetTemporaryWithDefault(false),
		Disp:      addrDisp,
	}
	if parameters.HasInitialProperties() {
		properties.Lifetimes = propertiesToLifetimes(parameters.GetInitialProperties())
	}

	if parameters.HasPerformDad() {
		_ = syslog.WarnTf(controlName, "ignoring 'perform_dad=%t' parameter. Not Supported.", parameters.GetPerformDad())
	}

	var reason admin.AddressRemovalReason
	if protocolAddr.AddressWithPrefix.PrefixLen > protocolAddr.AddressWithPrefix.Address.BitLen() {
		reason = admin.AddressRemovalReasonInvalid
	} else if ok, status := ifs.addAddress(protocolAddr, properties); !ok {
		reason = status
	}
	if reason != 0 {
		defer cancel()
		impl.mu.Lock()
		defer impl.mu.Unlock()
		if err := impl.mu.eventProxy.OnAddressRemoved(reason); err != nil {
			var zxError *zx.Error
			if !errors.As(err, &zxError) || (zxError.Status != zx.ErrPeerClosed && zxError.Status != zx.ErrBadHandle) {
				_ = syslog.WarnTf(controlName, "NICID=%d failed to send OnAddressRemoved(%s) for %s: %s", impl.nicid, reason, protocolAddr.AddressWithPrefix.Address, err)
			}
		}
		if err := impl.mu.eventProxy.Close(); err != nil {
			_ = syslog.WarnTf(controlName, "NICID=%d failed to close %s channel", impl.nicid, addressStateProviderName)
		}
		return nil
	}

	addSubnetRoute := parameters.GetAddSubnetRouteWithDefault(false)
	impl.addSubnetRoute = addSubnetRoute
	if addSubnetRoute {
		// We treat every address as being associated with its own route set.
		// This allows us to close the address's route set when the address is
		// removed, guaranteeing we clean up the associated subnet route.
		impl.ns.AddRoute(
			addressWithPrefixRoute(ifs.nicid, protocolAddr.AddressWithPrefix),
			nil,   /* metric*/
			false, /* dynamic */
			false, /* replaceMatchingGvisorRoutes */
			&impl.subnetRouteSetId,
		)
	}

	impl.mu.Lock()
	defer impl.mu.Unlock()
	impl.mu.sentAddedEvent = true
	if err := impl.mu.eventProxy.OnAddressAdded(); err != nil {
		_ = syslog.WarnTf(controlName, "NICID=%d failed to send OnAddressAdded() for %s - THIS MAY RESULT IN DROPPED ASP REQUESTS (https://fxbug.dev/42081560): %s", impl.nicid, protocolAddr.AddressWithPrefix.Address, err)
	}

	// If the address was removed before we sent the OnAddressAdded event, then
	// that means we need to send the (pending) OnAddressRemoved event. See
	// pi.OnRemoved for details.
	if impl.mu.removedReason != 0 {
		impl.sendOnAddressRemovedEventAndCancelServeLocked()
	}

	go func() {
		defer cancel()
		component.Serve(ctx, &admin.AddressStateProviderWithCtxStub{Impl: impl}, request.Channel, component.ServeOptions{
			Concurrent: true,
			OnError: func(err error) {
				_ = syslog.WarnTf(addressStateProviderName, "NICID=%d address state provider for %s: %s", impl.nicid, addr, err)
			},
		})

		impl.mu.Lock()
		detached, removedReason := impl.mu.detached, impl.mu.removedReason
		impl.mu.Unlock()

		addrDisp.mu.Lock()
		if detached {
			addrDisp.mu.aspImpl = nil
		}
		addrDisp.mu.Unlock()

		if !detached && removedReason == 0 {
			_ = syslog.DebugTf(addressStateProviderName, "NICID=%d removing address %s from NIC %d due to protocol closure", impl.nicid, addr, ci.nicid)
			switch status := ifs.removeAddress(impl.protocolAddr); status {
			case zx.ErrOk, zx.ErrNotFound:
			case zx.ErrBadState:
				_ = syslog.WarnTf(addressStateProviderName, "NIC %d removed when trying to remove address %s upon channel closure: %s", ci.nicid, addr, status)
			default:
				panic(fmt.Sprintf("NICID=%d unknown error trying to remove address %s upon channel closure: %s", impl.nicid, addr, status))
			}
		}

		if !detached {
			impl.cleanUpSubnetRoute()
		}
	}()
	return nil
}

func (ci *adminControlImpl) RemoveAddress(_ fidl.Context, address net.Subnet) (admin.ControlRemoveAddressResult, error) {
	protocolAddr := fidlconv.ToTCPIPProtocolAddress(address)
	nicInfo, ok := ci.ns.stack.NICInfo()[ci.nicid]
	if !ok {
		panic(fmt.Sprintf("NIC %d not found when removing %s", ci.nicid, protocolAddr.AddressWithPrefix))
	}
	ifs := nicInfo.Context.(*ifState)

	// If the address the caller requested to remove was assigned through DHCP,
	// just stop DHCP since that will result in the removal of the address.
	ifs.dhcpLock <- struct{}{}
	ifs.mu.Lock()
	defer func() {
		ifs.mu.Unlock()
		<-ifs.dhcpLock
	}()

	// DHCP client is only available for Ethernet interfaces.
	if ifs.mu.dhcp.Client != nil {
		if info := ifs.mu.dhcp.Client.Info(); info.Assigned.Address == protocolAddr.AddressWithPrefix.Address {
			ifs.setDHCPStatusLocked(nicInfo.Name, false)
			return admin.ControlRemoveAddressResultWithResponse(admin.ControlRemoveAddressResponse{DidRemove: true}), nil
		}
	}

	switch zxErr := ifs.removeAddress(protocolAddr); zxErr {
	case zx.ErrOk:
		return admin.ControlRemoveAddressResultWithResponse(admin.ControlRemoveAddressResponse{DidRemove: true}), nil
	case zx.ErrNotFound:
		return admin.ControlRemoveAddressResultWithResponse(admin.ControlRemoveAddressResponse{DidRemove: false}), nil
	default:
		panic(fmt.Sprintf("removeInterfaceAddress(%d, %v, false) = %s", ci.nicid, protocolAddr, zxErr))
	}
}

func (ci *adminControlImpl) GetId(fidl.Context) (uint64, error) {
	return uint64(ci.nicid), nil
}

// handleIPForwardingConfigurationResult handles the result of getting or
// setting IP forwarding or IP multicast forwarding configuration.
//
// Returns the result if the invoked function was successful. Otherwise, panics
// if an unexpected error occurred.
func handleIPForwardingConfigurationResult(result bool, err tcpip.Error, invokedFunction string) bool {
	switch err.(type) {
	case nil:
		return result
	case *tcpip.ErrUnknownNICID:
		// Impossible as this Control would be invalid if the interface is not
		// recognized.
		panic(fmt.Sprintf("got UnknownNICID error when Control is still valid from %s = %s", invokedFunction, err))
	default:
		panic(fmt.Sprintf("%s: %s", invokedFunction, err))
	}
}

// setIPForwardingLocked sets the IP forwarding configuration for the interface.
//
// The caller must hold the interface's write lock.
func (ci *adminControlImpl) setIPForwardingLocked(netProto tcpip.NetworkProtocolNumber, enabled bool) bool {
	prevEnabled, err := ci.ns.stack.SetNICForwarding(tcpip.NICID(ci.nicid), netProto, enabled)
	return handleIPForwardingConfigurationResult(prevEnabled, err, fmt.Sprintf("ci.ns.stack.SetNICForwarding(tcpip.NICID(%d), %d, %t)", ci.nicid, netProto, enabled))
}

// setMulticastIPForwardingLocked sets the IP multicast forwarding configuration
// for the interface.
//
// The caller must hold the interface's write lock.
func (ci *adminControlImpl) setMulticastIPForwardingLocked(netProto tcpip.NetworkProtocolNumber, enabled bool) bool {
	prevEnabled, err := ci.ns.stack.SetNICMulticastForwarding(tcpip.NICID(ci.nicid), netProto, enabled)
	return handleIPForwardingConfigurationResult(prevEnabled, err, fmt.Sprintf("ci.ns.stack.SetNICMulticastForwarding(tcpip.NICID(%d), %d, %t)", ci.nicid, netProto, enabled))
}

func (ci *adminControlImpl) getNetworkEndpoint(netProto tcpip.NetworkProtocolNumber) stack.NetworkEndpoint {
	ep, err := ci.ns.stack.GetNetworkEndpoint(tcpip.NICID(ci.nicid), netProto)
	if err != nil {
		panic(fmt.Sprintf("ci.ns.stack.GetNetworkEndpoint(tcpip.NICID(%d), %d): %s", ci.nicid, netProto, err))
	}

	return ep
}

func toAdminNudConfiguration(stackNud stack.NUDConfigurations) admin.NudConfiguration {
	var adminNud admin.NudConfiguration
	adminNud.SetMaxMulticastSolicitations(uint16(stackNud.MaxMulticastProbes))
	adminNud.SetMaxUnicastSolicitations(uint16(stackNud.MaxUnicastProbes))
	adminNud.SetBaseReachableTime(stackNud.BaseReachableTime.Nanoseconds())
	return adminNud
}

func (ci *adminControlImpl) getNUDConfig(netProto tcpip.NetworkProtocolNumber) stack.NUDConfigurations {
	config, err := ci.ns.stack.NUDConfigurations(tcpip.NICID(ci.nicid), netProto)
	if err != nil {
		panic(fmt.Sprintf("ci.ns.stack.NUDConfigurations(tcpip.NICID(%d), %d): %s", ci.nicid, netProto, err))
	}
	return config
}

func (ci *adminControlImpl) applyNUDConfig(netProto tcpip.NetworkProtocolNumber, nudConfig *admin.NudConfiguration) admin.NudConfiguration {
	var previousNudConfig admin.NudConfiguration

	// NB: We're reading and updating in place here without acquiring
	// locks, this is fine because we don't serve
	// fuchsia.net.interfaces.admin.Control concurrently.
	stackNudConfig := ci.getNUDConfig(netProto)
	needsNudUpdate := false
	if nudConfig.HasMaxMulticastSolicitations() {
		prev := stackNudConfig.MaxMulticastProbes
		stackNudConfig.MaxMulticastProbes = uint32(nudConfig.MaxMulticastSolicitations)
		previousNudConfig.SetMaxMulticastSolicitations(uint16(prev))
		needsNudUpdate = true
	}
	if nudConfig.HasMaxUnicastSolicitations() {
		prev := stackNudConfig.MaxUnicastProbes
		stackNudConfig.MaxUnicastProbes = uint32(nudConfig.MaxUnicastSolicitations)
		previousNudConfig.SetMaxUnicastSolicitations(uint16(prev))
		needsNudUpdate = true
	}
	if nudConfig.HasBaseReachableTime() {
		prev := stackNudConfig.BaseReachableTime
		stackNudConfig.BaseReachableTime = time.Duration(nudConfig.BaseReachableTime)
		previousNudConfig.SetBaseReachableTime(prev.Nanoseconds())
		needsNudUpdate = true
	}

	if needsNudUpdate {
		if err := ci.ns.stack.SetNUDConfigurations(tcpip.NICID(ci.nicid), netProto, stackNudConfig); err != nil {
			panic(fmt.Sprintf("ci.ns.stack.SetNUDConfigurations(tcpip.NICID(%d), %d, %v): %s", ci.nicid, netProto, stackNudConfig, err))
		}
	}
	return previousNudConfig
}

func (ci *adminControlImpl) getIGMPEndpoint() ipv4.IGMPEndpoint {
	// We want this to panic if EP does not implement ipv4.IGMPEndpoint.
	return ci.getNetworkEndpoint(ipv4.ProtocolNumber).(ipv4.IGMPEndpoint)
}

func (ci *adminControlImpl) getMLDEndpoint() ipv6.MLDEndpoint {
	// We want this to panic if EP does not implement ipv6.MLDEndpoint.
	return ci.getNetworkEndpoint(ipv6.ProtocolNumber).(ipv6.MLDEndpoint)
}

func toAdminIgmpVersion(v ipv4.IGMPVersion) admin.IgmpVersion {
	switch v {
	case ipv4.IGMPVersion1:
		return admin.IgmpVersionV1
	case ipv4.IGMPVersion2:
		return admin.IgmpVersionV2
	case ipv4.IGMPVersion3:
		return admin.IgmpVersionV3
	default:
		panic(fmt.Sprintf("unrecognized version = %d", v))
	}
}

func toAdminMldVersion(v ipv6.MLDVersion) admin.MldVersion {
	switch v {
	case ipv6.MLDVersion1:
		return admin.MldVersionV1
	case ipv6.MLDVersion2:
		return admin.MldVersionV2
	default:
		panic(fmt.Sprintf("unrecognized version = %d", v))
	}
}

func (ci *adminControlImpl) SetConfiguration(_ fidl.Context, config admin.Configuration) (admin.ControlSetConfigurationResult, error) {
	if config.HasIpv4() {
		// Loopback does not support forwarding.
		if ci.isLoopback() {
			if config.Ipv4.HasUnicastForwarding() && config.Ipv4.UnicastForwarding {
				return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIpv4ForwardingUnsupported), nil
			}

			if config.Ipv4.HasMulticastForwarding() && config.Ipv4.MulticastForwarding {
				return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIpv4MulticastForwardingUnsupported), nil
			}

			if config.Ipv4.HasArp() {
				return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorArpNotSupported), nil
			}
		}

		// Make sure the IGMP version (if specified) is supported.
		if config.Ipv4.HasIgmp() && config.Ipv4.Igmp.HasVersion() {
			switch config.Ipv4.Igmp.Version {
			case admin.IgmpVersionV1, admin.IgmpVersionV2, admin.IgmpVersionV3:
			default:
				return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIpv4IgmpVersionUnsupported), nil
			}
		}

		if config.Ipv4.HasArp() {
			if config.Ipv4.Arp.HasNud() {
				if config.Ipv4.Arp.Nud.HasMaxMulticastSolicitations() && config.Ipv4.Arp.Nud.MaxMulticastSolicitations == 0 {
					return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIllegalZeroValue), nil
				}
				if config.Ipv4.Arp.Nud.HasMaxUnicastSolicitations() && config.Ipv4.Arp.Nud.MaxUnicastSolicitations == 0 {
					return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIllegalZeroValue), nil
				}
				if config.Ipv4.Arp.Nud.HasBaseReachableTime() {
					if config.Ipv4.Arp.Nud.BaseReachableTime < 0 {
						return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIllegalNegativeValue), nil
					} else if config.Ipv4.Arp.Nud.BaseReachableTime == 0 {
						return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIllegalZeroValue), nil
					}

				}
			}
		}
	}

	if config.HasIpv6() {
		// Loopback does not support forwarding.
		if ci.isLoopback() {
			if config.Ipv6.HasUnicastForwarding() && config.Ipv6.UnicastForwarding {
				return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIpv6ForwardingUnsupported), nil
			}

			if config.Ipv6.HasMulticastForwarding() && config.Ipv6.MulticastForwarding {
				return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIpv6MulticastForwardingUnsupported), nil
			}

			if config.Ipv6.HasNdp() {
				return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorNdpNotSupported), nil
			}
		}

		// Make sure the MLD version (if specified) is supported.
		if config.Ipv6.HasMld() && config.Ipv6.Mld.HasVersion() {
			switch config.Ipv6.Mld.Version {
			case admin.MldVersionV1, admin.MldVersionV2:
			default:
				return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIpv6MldVersionUnsupported), nil
			}
		}

		if config.Ipv6.HasNdp() {
			if config.Ipv6.Ndp.HasNud() {
				if config.Ipv6.Ndp.Nud.HasMaxMulticastSolicitations() && config.Ipv6.Ndp.Nud.MaxMulticastSolicitations == 0 {
					return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIllegalZeroValue), nil
				}
				if config.Ipv6.Ndp.Nud.HasMaxUnicastSolicitations() && config.Ipv6.Ndp.Nud.MaxUnicastSolicitations == 0 {
					return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIllegalZeroValue), nil
				}
				if config.Ipv6.Ndp.Nud.HasBaseReachableTime() {
					if config.Ipv6.Ndp.Nud.BaseReachableTime < 0 {
						return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIllegalNegativeValue), nil
					} else if config.Ipv6.Ndp.Nud.BaseReachableTime == 0 {
						return admin.ControlSetConfigurationResultWithErr(admin.ControlSetConfigurationErrorIllegalZeroValue), nil
					}

				}
			}
		}
	}

	ifs := ci.getNICContext()
	ifs.mu.Lock()
	defer ifs.mu.Unlock()

	var previousConfig admin.Configuration

	if config.HasIpv4() {
		var previousIpv4Config admin.Ipv4Configuration
		ipv4Config := config.Ipv4

		if ipv4Config.HasUnicastForwarding() {
			previousIpv4Config.SetUnicastForwarding(ci.setIPForwardingLocked(ipv4.ProtocolNumber, ipv4Config.UnicastForwarding))
		}

		if ipv4Config.HasMulticastForwarding() {
			previousIpv4Config.SetMulticastForwarding(ci.setMulticastIPForwardingLocked(ipv4.ProtocolNumber, ipv4Config.MulticastForwarding))
		}

		if ipv4Config.HasIgmp() {
			var previousIgmpConfig admin.IgmpConfiguration
			igmpConfig := ipv4Config.Igmp

			if igmpConfig.HasVersion() {
				var newVersion ipv4.IGMPVersion
				switch igmpConfig.Version {
				case admin.IgmpVersionV1:
					newVersion = ipv4.IGMPVersion1
				case admin.IgmpVersionV2:
					newVersion = ipv4.IGMPVersion2
				case admin.IgmpVersionV3:
					newVersion = ipv4.IGMPVersion3
				default:
					// We validated IGMP version above.
					panic(fmt.Sprintf("unexpected IGMP version = %d", igmpConfig.Version))
				}

				previousIgmpConfig.SetVersion(toAdminIgmpVersion(ci.getIGMPEndpoint().SetIGMPVersion(newVersion)))
			}

			previousIpv4Config.SetIgmp(previousIgmpConfig)
		}

		if ipv4Config.HasArp() {
			var previousArpConfig admin.ArpConfiguration
			if ipv4Config.Arp.HasNud() {
				previousArpConfig.SetNud(ci.applyNUDConfig(ipv4.ProtocolNumber, &ipv4Config.Arp.Nud))
			}
			if ipv4Config.Arp.HasDad() {
				_ = syslog.WarnTf(controlName, "ignoring unsupported Ipv4 DAD configuration")
			}

			previousIpv4Config.SetArp(previousArpConfig)
		}

		previousConfig.SetIpv4(previousIpv4Config)
	}

	if config.HasIpv6() {
		var previousIpv6Config admin.Ipv6Configuration
		ipv6Config := config.Ipv6

		if ipv6Config.HasUnicastForwarding() {
			previousIpv6Config.SetUnicastForwarding(ci.setIPForwardingLocked(ipv6.ProtocolNumber, ipv6Config.UnicastForwarding))
		}

		if ipv6Config.HasMulticastForwarding() {
			previousIpv6Config.SetMulticastForwarding(ci.setMulticastIPForwardingLocked(ipv6.ProtocolNumber, ipv6Config.MulticastForwarding))
		}

		if ipv6Config.HasMld() {
			var previousMldConfig admin.MldConfiguration
			mldConfig := ipv6Config.Mld

			if mldConfig.HasVersion() {
				var newVersion ipv6.MLDVersion
				switch mldConfig.Version {
				case admin.MldVersionV1:
					newVersion = ipv6.MLDVersion1
				case admin.MldVersionV2:
					newVersion = ipv6.MLDVersion2
				default:
					// We validated MLD version above.
					panic(fmt.Sprintf("unexpected MLD version = %d", mldConfig.Version))
				}

				previousMldConfig.SetVersion(toAdminMldVersion(ci.getMLDEndpoint().SetMLDVersion(newVersion)))
			}

			previousIpv6Config.SetMld(previousMldConfig)
		}

		if ipv6Config.HasNdp() {
			var previousNdpConfig admin.NdpConfiguration
			if ipv6Config.Ndp.HasNud() {
				previousNdpConfig.SetNud(ci.applyNUDConfig(ipv6.ProtocolNumber, &ipv6Config.Ndp.Nud))
			}

			if ipv6Config.Ndp.HasDad() {
				_ = syslog.WarnTf(controlName, "ignoring unsupported IPv6 DAD configuration")
			}

			if ipv6Config.Ndp.HasSlaac() {
				var previousSLAACConfig admin.SlaacConfiguration
				if ipv6Config.Ndp.Slaac.HasTemporaryAddress() {
					ndpEP := ci.getNetworkEndpoint(ipv6.ProtocolNumber).(ipv6.NDPEndpoint)
					config := ndpEP.NDPConfigurations()
					previousSLAACConfig.SetTemporaryAddress(config.AutoGenTempGlobalAddresses)
					config.AutoGenTempGlobalAddresses = ipv6Config.Ndp.Slaac.TemporaryAddress
					ndpEP.SetNDPConfigurations(config)
				}
				previousNdpConfig.SetSlaac(previousSLAACConfig)
			}

			previousIpv6Config.SetNdp(previousNdpConfig)
		}

		previousConfig.SetIpv6(previousIpv6Config)
	}

	// Invalidate all clients' destination caches, as disabling forwarding may
	// cause an existing cached route to become invalid.
	ifs.ns.resetDestinationCache()

	return admin.ControlSetConfigurationResultWithResponse(admin.ControlSetConfigurationResponse{
		PreviousConfig: previousConfig,
	}), nil
}

// ipForwardingRLocked gets the IP forwarding configuration for the interface.
//
// The caller must hold the interface's read lock.
func (ci adminControlImpl) ipForwardingRLocked(netProto tcpip.NetworkProtocolNumber) bool {
	enabled, err := ci.ns.stack.NICForwarding(tcpip.NICID(ci.nicid), netProto)
	return handleIPForwardingConfigurationResult(enabled, err, fmt.Sprintf("ci.ns.stack.NICForwarding(tcpip.NICID(%d), %d)", ci.nicid, netProto))
}

// multicastIPForwardingRLocked gets the IP multicast forwarding configuration
// for the interface.
//
// The caller must hold the interface's read lock.
func (ci adminControlImpl) multicastIPForwardingRLocked(netProto tcpip.NetworkProtocolNumber) bool {
	enabled, err := ci.ns.stack.NICMulticastForwarding(tcpip.NICID(ci.nicid), netProto)
	return handleIPForwardingConfigurationResult(enabled, err, fmt.Sprintf("ci.ns.stack.NICMulticastForwarding(tcpip.NICID(%d), %d)", ci.nicid, netProto))
}

func (ci *adminControlImpl) GetConfiguration(fidl.Context) (admin.ControlGetConfigurationResult, error) {
	ifs := ci.getNICContext()
	ifs.mu.RLock()
	defer ifs.mu.RUnlock()

	var config admin.Configuration

	{
		var ipv4Config admin.Ipv4Configuration
		ipv4Config.SetUnicastForwarding(ci.ipForwardingRLocked(ipv4.ProtocolNumber))
		ipv4Config.SetMulticastForwarding(ci.multicastIPForwardingRLocked(ipv4.ProtocolNumber))

		var igmpConfig admin.IgmpConfiguration
		igmpConfig.SetVersion(toAdminIgmpVersion(ci.getIGMPEndpoint().GetIGMPVersion()))
		ipv4Config.SetIgmp(igmpConfig)
		if !ci.isLoopback() {
			var arpConfig admin.ArpConfiguration
			arpConfig.SetNud(toAdminNudConfiguration(ci.getNUDConfig(ipv4.ProtocolNumber)))
			ipv4Config.SetArp(arpConfig)
		}

		config.SetIpv4(ipv4Config)
	}

	{
		var ipv6Config admin.Ipv6Configuration
		ipv6Config.SetUnicastForwarding(ci.ipForwardingRLocked(ipv6.ProtocolNumber))
		ipv6Config.SetMulticastForwarding(ci.multicastIPForwardingRLocked(ipv6.ProtocolNumber))

		var mldConfig admin.MldConfiguration
		mldConfig.SetVersion(toAdminMldVersion(ci.getMLDEndpoint().GetMLDVersion()))
		ipv6Config.SetMld(mldConfig)
		if !ci.isLoopback() {
			var ndpConfig admin.NdpConfiguration
			ndpConfig.SetNud(toAdminNudConfiguration(ci.getNUDConfig(ipv6.ProtocolNumber)))

			var slaacConfig admin.SlaacConfiguration
			config := ci.getNetworkEndpoint(ipv6.ProtocolNumber).(ipv6.NDPEndpoint).NDPConfigurations()
			slaacConfig.SetTemporaryAddress(config.AutoGenTempGlobalAddresses)
			ndpConfig.SetSlaac(slaacConfig)

			ipv6Config.SetNdp(ndpConfig)
		}

		config.SetIpv6(ipv6Config)
	}

	return admin.ControlGetConfigurationResultWithResponse(admin.ControlGetConfigurationResponse{
		Config: config,
	}), nil
}

type adminControlCollection struct {
	mu struct {
		sync.Mutex
		tearingDown    bool
		controls       map[*adminControlImpl]struct{}
		strongRefCount uint
	}
}

func (c *adminControlCollection) stopServing() []zx.Channel {
	c.mu.Lock()
	controls := c.mu.controls
	c.mu.controls = nil
	c.mu.tearingDown = true
	c.mu.Unlock()

	for control := range controls {
		control.cancelServe()
	}

	var pendingTerminal []zx.Channel
	for control := range controls {
		pending := <-control.doneChannel
		if pending.Handle().IsValid() {
			pendingTerminal = append(pendingTerminal, pending)
		}
	}

	return pendingTerminal
}

func sendControlTerminationReason(pending []zx.Channel, reason admin.InterfaceRemovedReason) {
	for _, c := range pending {
		eventProxy := admin.ControlEventProxy{Channel: c}
		if err := eventProxy.OnInterfaceRemoved(reason); err != nil {
			_ = syslog.WarnTf(controlName, "failed to send interface close reason %s: %s", reason, err)
		}
		// Close the channel iff the event proxy hasn't done it already.
		if channel := eventProxy.Channel; channel.Handle().IsValid() {
			if err := channel.Close(); err != nil {
				_ = syslog.ErrorTf(controlName, "channel.Close() = %s", err)
			}
		}
	}
}

func (ifs *ifState) addAdminConnection(request admin.ControlWithCtxInterfaceRequest, strong bool) {

	impl, ctx, cancel := func() (*adminControlImpl, context.Context, context.CancelFunc) {
		ifs.adminControls.mu.Lock()
		defer ifs.adminControls.mu.Unlock()

		// Do not add more connections to an interface that is tearing down.
		if ifs.adminControls.mu.tearingDown {
			if err := request.Channel.Close(); err != nil {
				_ = syslog.ErrorTf(controlName, "request.channel.Close() = %s", err)
			}
			return nil, nil, nil
		}

		ctx, cancel := context.WithCancel(context.Background())
		impl := &adminControlImpl{
			ns:          ifs.ns,
			nicid:       ifs.nicid,
			cancelServe: cancel,
			// We need a buffer in this channel to accommodate for the Remove
			// call, which buffers the request channel here before removing the
			// interface.
			doneChannel: make(chan zx.Channel, 1),
			isStrongRef: strong,
		}

		ifs.adminControls.mu.controls[impl] = struct{}{}
		if impl.isStrongRef {
			ifs.adminControls.mu.strongRefCount++
		}

		return impl, ctx, cancel
	}()
	if impl == nil {
		return
	}

	go func() {
		defer cancel()
		defer close(impl.doneChannel)
		requestChannel := request.Channel

		component.Serve(ctx, &admin.ControlWithCtxStub{Impl: impl}, requestChannel, component.ServeOptions{
			Concurrent:       false,
			KeepChannelAlive: true,
			OnError: func(err error) {
				_ = syslog.WarnTf(controlName, "%s", err)
			},
		})

		// NB: anonymous function is used to restrict section where the lock is
		// held.
		ifStateToRemove := func() *ifState {
			ifs.adminControls.mu.Lock()
			defer ifs.adminControls.mu.Unlock()
			wasCanceled := errors.Is(ctx.Err(), context.Canceled)

			if keepInterface := func() bool {
				// Always proceed with removal if this was a synchronous remove
				// request.
				if impl.syncRemoval {
					return false
				}

				// Don't consider destroying if not a strong ref.
				//
				// Note that the implementation can change from a strong to a
				// weak ref if Detach is called, which is how we allow
				// interfaces to leak.
				//
				// This is also how we prevent destruction from interfaces
				// created with the legacy API, since they never have strong
				// refs.
				if !impl.isStrongRef {
					return true
				}

				ifs.adminControls.mu.strongRefCount--
				// If serving was canceled, that means that removal happened due
				// to outside cancelation already.
				if wasCanceled {
					return true
				}

				// Don't destroy if there are any strong refs left.
				if ifs.adminControls.mu.strongRefCount != 0 {
					return true
				}

				return false
			}(); keepInterface {
				if wasCanceled {
					// If we were canceled, pass the request channel along so
					// it'll receive the epitaph later.
					impl.doneChannel <- requestChannel
				} else {
					delete(ifs.adminControls.mu.controls, impl)
					if err := requestChannel.Close(); err != nil {
						_ = syslog.ErrorTf(controlName, "requestChannel.Close() = %s", err)
					}
				}
				return nil
			}

			// We're good to remove this interface.
			// Prevent new connections while we're holding the collection lock,
			// avoiding races between here and removing the interface below.
			ifs.adminControls.mu.tearingDown = true

			// Stash the requestChannel in our finalization channel so the
			// terminal event is sent later.
			impl.doneChannel <- requestChannel

			nicInfo, ok := impl.ns.stack.NICInfo()[impl.nicid]
			if !ok {
				ifs.mu.Lock()
				removed := ifs.mu.removed
				ifs.mu.Unlock()
				if removed {
					// This can happen if the interface was removed because the
					// device port was closed. In that case, we don't have an
					// entry in the NICInfo map.
					_ = syslog.DebugTf(controlName, "interface %d already removed", impl.nicid)
					return nil
				}
				panic(fmt.Sprintf("failed to find interface %d", impl.nicid))
			}
			// We can safely remove the interface now because we're certain that
			// this control impl is not in the collection anymore, so it can't
			// deadlock waiting for control interfaces to finish.
			return nicInfo.Context.(*ifState)
		}()

		if ifStateToRemove != nil {
			ifStateToRemove.RemoveByUser()
		}

	}()
}

var _ admin.InstallerWithCtx = (*interfacesAdminInstallerImpl)(nil)

type interfacesAdminInstallerImpl struct {
	ns *Netstack
}

func (i *interfacesAdminInstallerImpl) InstallDevice(_ fidl.Context, device network.DeviceWithCtxInterface, deviceControl admin.DeviceControlWithCtxInterfaceRequest) error {
	client, err := netdevice.NewClient(context.Background(), &device, &netdevice.SimpleSessionConfigFactory{})
	if err != nil {
		_ = syslog.WarnTf(controlName, "InstallDevice: %s", err)
		_ = deviceControl.Close()
		return nil
	}

	ctx, cancel := context.WithCancel(context.Background())
	impl := &interfacesAdminDeviceControlImpl{
		ns:           i.ns,
		deviceClient: client,
	}

	// Running the device client and serving the FIDL are tied to the same
	// context because their lifecycles are linked.
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		impl.deviceClient.Run(ctx)
		cancel()
	}()

	go func() {
		component.Serve(ctx, &admin.DeviceControlWithCtxStub{Impl: impl}, deviceControl.Channel, component.ServeOptions{
			OnError: func(err error) {
				_ = syslog.WarnTf(deviceControlName, "%s", err)
			},
		})
		if !impl.detached {
			cancel()
		}
		// Wait for device goroutine to finish before closing the device.
		wg.Wait()
		if err := impl.deviceClient.Close(); err != nil {
			_ = syslog.ErrorTf(deviceControlName, "deviceClient.Close() = %s", err)
		}
	}()

	return nil
}

func (i *interfacesAdminInstallerImpl) InstallBlackholeInterface(_ fidl.Context, control admin.ControlWithCtxInterfaceRequest, _ admin.Options) error {
	_ = syslog.ErrorTf(admin.InstallerName, "InstallBlackholeInterface is not supported")
	component.CloseWithEpitaph(control.Channel, zx.ErrNotSupported)
	return nil
}

var _ admin.DeviceControlWithCtx = (*interfacesAdminDeviceControlImpl)(nil)

type interfacesAdminDeviceControlImpl struct {
	ns           *Netstack
	deviceClient *netdevice.Client
	detached     bool
}

func (d *interfacesAdminDeviceControlImpl) CreateInterface(_ fidl.Context, portId network.PortId, control admin.ControlWithCtxInterfaceRequest, options admin.Options) error {

	ifs, closeReason := func() (*ifState, admin.InterfaceRemovedReason) {
		port, err := d.deviceClient.NewPort(context.Background(), portId)
		if err != nil {
			_ = syslog.WarnTf(deviceControlName, "NewPort(_, %d) failed: %s", portId, err)
			{
				var unsupported *netdevice.InvalidPortOperatingModeError
				if errors.As(err, &unsupported) {
					return nil, admin.InterfaceRemovedReasonBadPort
				}
			}
			{
				var alreadyBound *netdevice.PortAlreadyBoundError
				if errors.As(err, &alreadyBound) {
					return nil, admin.InterfaceRemovedReasonPortAlreadyBound
				}
			}

			// Assume all other errors are due to problems communicating with the
			// port.
			return nil, admin.InterfaceRemovedReasonPortClosed
		}
		defer func() {
			if port != nil {
				_ = port.Close()
			}
		}()

		var namePrefix string
		var linkEndpoint stack.LinkEndpoint
		switch mode := port.Mode(); mode {
		case netdevice.PortModeEthernet:
			namePrefix = "eth"
			linkEndpoint = ethernet.New(port)
		case netdevice.PortModeIp:
			namePrefix = "ip"
			linkEndpoint = port
		default:
			panic(fmt.Sprintf("unknown port mode %d", mode))
		}

		metric := defaultInterfaceMetric
		if options.HasMetric() {
			metric = routetypes.Metric(options.GetMetric())
		}
		ifs, err := d.ns.addEndpoint(
			makeEndpointName(namePrefix, options.GetNameWithDefault("")),
			linkEndpoint,
			port,
			port,
			metric,
			qdiscConfig{numQueues: numQDiscFIFOQueues, queueLen: int(port.TxDepth()) * qdiscTxDepthMultiplier},
		)
		if err != nil {
			_ = syslog.WarnTf(deviceControlName, "addEndpoint failed: %s", err)
			var tcpipError *TcpIpError
			if errors.As(err, &tcpipError) {
				switch tcpipError.Err.(type) {
				case *tcpip.ErrDuplicateNICID:
					return nil, admin.InterfaceRemovedReasonDuplicateName
				}
			}
			panic(fmt.Sprintf("unexpected error ns.AddEndpoint(..) = %s", err))
		}

		// Prevent deferred cleanup from running.
		port = nil

		return ifs, 0
	}()

	if closeReason != 0 {
		proxy := admin.ControlEventProxy{
			Channel: control.Channel,
		}
		if err := proxy.OnInterfaceRemoved(closeReason); err != nil {
			_ = syslog.WarnTf(deviceControlName, "failed to write terminal event %s: %s", closeReason, err)
		}
		_ = control.Close()
		return nil
	}

	ifs.addAdminConnection(control, true /* strong */)

	return nil
}

func (d *interfacesAdminDeviceControlImpl) Detach(fidl.Context) error {
	d.detached = true
	return nil
}
