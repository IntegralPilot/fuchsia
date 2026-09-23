// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netstack

import (
	// #include "zircon/process.h"
	"C"

	"fmt"
	"runtime"
	"syscall/zx"
	"syscall/zx/fidl"

	"go.fuchsia.dev/fuchsia/src/lib/component"
	syslog "go.fuchsia.dev/fuchsia/src/lib/syslog/go"

	"fidl/fuchsia/hardware/network"
	"fidl/fuchsia/net/debug"
	"gvisor.dev/gvisor/pkg/tcpip"
)

var _ debug.InterfacesWithCtx = (*debugInterfacesImpl)(nil)

type debugInterfacesImpl struct {
	ns *Netstack
}

func (ci *debugInterfacesImpl) GetPort(_ fidl.Context, nicid uint64, request network.PortWithCtxInterfaceRequest) error {
	closeRequest := func(epitaph zx.Status) {
		if err := component.CloseWithEpitaph(request.Channel, epitaph); err != nil {
			_ = syslog.WarnTf(debug.InterfacesName, "GetPort(%d) close error: %s", nicid, err)
		}
	}

	nicInfo, ok := ci.ns.stack.NICInfo()[tcpip.NICID(nicid)]
	if !ok {
		closeRequest(zx.ErrNotFound)
		return nil
	}

	ifs := nicInfo.Context.(*ifState)
	if ifs.controller == nil {
		closeRequest(zx.ErrNotSupported)
		return nil
	}

	ifs.controller.ConnectPort(request)
	return nil
}

func (ci *debugInterfacesImpl) CloseBackingSession(_ fidl.Context, id uint64) (debug.InterfacesCloseBackingSessionResult, error) {
	return debug.InterfacesCloseBackingSessionResultWithErr(debug.CloseSessionErrorNotSupported), nil
}

var _ debug.DiagnosticsWithCtx = (*debugDiagnosticsImpl)(nil)

type debugDiagnosticsImpl struct {
}

func (d *debugDiagnosticsImpl) LogDebugInfoToSyslog(fidl.Context) error {
	s := func() string {
		buf := make([]byte, 4096)
		for {
			n := runtime.Stack(buf, true)
			if n < len(buf) {
				return string(buf[:n])
			}
			buf = make([]byte, 2*len(buf))
		}
	}()
	// Print the stack to syslog using stdio so we don't need to do the work of
	// splitting into messages.
	fmt.Printf("Dumping goroutines to syslog as requested from %s, this is not a crash.\n", debug.DiagnosticsName)
	fmt.Println(s)
	fmt.Println("End of debug info")

	return nil
}

func (d *debugDiagnosticsImpl) GetProcessHandleForInspection(fidl.Context) (zx.Handle, error) {
	self := zx.Handle(C.zx_process_self())
	return self.Duplicate(zx.RightInspect | zx.RightTransfer)
}
