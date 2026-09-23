// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package netstack

import (
	"context"
	"sync"
	"syscall/zx"
	"testing"

	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routes"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/routetypes"
	"go.fuchsia.dev/fuchsia/src/connectivity/network/netstack/util"

	fnetRoutes "fidl/fuchsia/net/routes"

	"gvisor.dev/gvisor/pkg/tcpip"
)

type watcherHolder struct {
	version routetypes.IpProtoTag
	v4      *fnetRoutes.WatcherV4WithCtxInterface
	v6      *fnetRoutes.WatcherV6WithCtxInterface
}

func (w watcherHolder) expectPeerClosed(ctx context.Context, t *testing.T) {
	var err error
	switch w.version {
	case routetypes.IPv4:
		_, err = w.v4.Watch(ctx)
	case routetypes.IPv6:
		_, err = w.v6.Watch(ctx)
	default:
		t.Fatalf("unsupported IP protocol version: %d", w.version)
	}

	if err == nil {
		t.Errorf("call to Watch unexpectedly succeeded.")
	} else if zxErr, ok := err.(*zx.Error); !ok || zxErr.Status != zx.ErrPeerClosed {
		t.Errorf("got err=%s, want=zx.ErrPeerClosed", err)
	}
}

// TestRoutesWatcherMetrics verifies that fidlRoutesWatcherMetrics properly
// track Watcher creation & deletion.
func TestRoutesWatcherMetrics(t *testing.T) {
	// Instantiate Watcher clients.
	watcher_v4_req, watcher_v4, err := fnetRoutes.NewWatcherV4WithCtxInterfaceRequest()
	if err != nil {
		t.Fatalf("failed to instantiate WatcherV4 Client")
	}
	watcher_v6_req, watcher_v6, err := fnetRoutes.NewWatcherV6WithCtxInterfaceRequest()
	if err != nil {
		t.Fatalf("failed to instantiate WatcherV6 Client")
	}

	// Instantiate the routesWatcherEventLoop.
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	interruptsChan := make(chan routeInterrupt, maxPendingInterrupts)
	metrics := fidlRoutesWatcherMetrics{}
	var wg sync.WaitGroup
	go func() {
		wg.Add(1)
		routesWatcherEventLoop(ctx, interruptsChan, &metrics)
		wg.Done()
	}()

	// Connect the Watcher clients, using Watch as a means to synchronize and
	// wait for the watcher to be installed.
	interruptsChan <- &getWatcherV4Request{
		req:     watcher_v4_req,
		options: fnetRoutes.WatcherOptionsV4{},
	}
	if _, err = watcher_v4.Watch(ctx); err != nil {
		t.Errorf("failed to Watch: %s", err)
	}
	if got := metrics.count_v4.Load(); got != 1 {
		t.Errorf("got %d WatcherV4 client; expected 1", got)
	}
	if got := metrics.count_v6.Load(); got != 0 {
		t.Errorf("got %d WatcherV6 client; expected 0", got)
	}
	interruptsChan <- &getWatcherV6Request{
		req:     watcher_v6_req,
		options: fnetRoutes.WatcherOptionsV6{},
	}
	if _, err = watcher_v6.Watch(ctx); err != nil {
		t.Errorf("failed to Watch: %s", err)
	}
	if got := metrics.count_v4.Load(); got != 1 {
		t.Errorf("got %d WatcherV4 client; expected 1", got)
	}
	if got := metrics.count_v6.Load(); got != 1 {
		t.Errorf("got %d WatcherV6 client; expected 1", got)
	}

	// Abort the backing implementations
	cancel()

	wg.Wait()
	if got := metrics.count_v4.Load(); got != 0 {
		t.Errorf("got %d WatcherV4 client; expected 0", got)
	}
	if got := metrics.count_v6.Load(); got != 0 {
		t.Errorf("got %d WatcherV6 client; expected 0", got)
	}
}

// TestRoutesWatcherSlowClient verifies that clients who consume events slower
// than they're produced eventually get terminated.
func TestRoutesWatcherSlowClient(t *testing.T) {
	// Instantiate Watcher clients.
	watcher_v4_req, watcher_v4, err := fnetRoutes.NewWatcherV4WithCtxInterfaceRequest()
	if err != nil {
		t.Fatalf("failed to instantiate WatcherV4 Client")
	}
	watcher_v6_req, watcher_v6, err := fnetRoutes.NewWatcherV6WithCtxInterfaceRequest()
	if err != nil {
		t.Fatalf("failed to instantiate WatcherV6 Client")
	}

	tests := []struct {
		name        string
		subnet      tcpip.Address
		subnet_mask tcpip.AddressMask
		req         routesGetWatcherRequest
		watcher     watcherHolder
	}{
		{
			name:        "IPv4",
			subnet:      util.Parse("192.168.0.0"),
			subnet_mask: util.ParseMask("255.255.255.0"),
			req: &getWatcherV4Request{
				req:     watcher_v4_req,
				options: fnetRoutes.WatcherOptionsV4{},
			},
			watcher: watcherHolder{
				version: routetypes.IPv4,
				v4:      watcher_v4,
			},
		},
		{
			name:        "IPv6",
			subnet:      util.Parse("fe80::"),
			subnet_mask: util.ParseMask("ffff:ffff:ffff:ffff::"),
			req: &getWatcherV6Request{
				req:     watcher_v6_req,
				options: fnetRoutes.WatcherOptionsV6{},
			},
			watcher: watcherHolder{
				version: routetypes.IPv6,
				v6:      watcher_v6,
			},
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			// Instantiate the routesWatcherEventLoop.
			ctx, cancel := context.WithCancel(context.Background())
			defer cancel()
			// Note; don't buffer maxPendingInterrupts; buffering would break the
			// synchronization of the test and make it flaky.
			interruptChan := make(chan routeInterrupt, 1)
			metrics := fidlRoutesWatcherMetrics{}
			go routesWatcherEventLoop(ctx, interruptChan, &metrics)

			// Connect the Watcher client.
			switch req := test.req.(type) {
			case *getWatcherV4Request:
				interruptChan <- req
			case *getWatcherV6Request:
				interruptChan <- req
			}

			// Add and remove a route to generate events.
			subnet, err := tcpip.NewSubnet(test.subnet, test.subnet_mask)
			if err != nil {
				t.Fatalf("failed to create subnet: %s", err)
			}
			route := routetypes.ExtendedRoute{
				Route: tcpip.Route{
					Destination: subnet,
				},
			}
			for i := 0; i <= int(maxPendingEventsPerClient)+1; i++ {
				change := routes.RoutingTableChange{
					Route: route,
				}
				if i%2 == 0 {
					change.Change = routetypes.RouteAdded
				} else {
					change.Change = routetypes.RouteRemoved
				}
				interruptChan <- &routingTableChange{change}
			}

			// Observe PEER_CLOSED on the watcher.
			test.watcher.expectPeerClosed(ctx, t)
		})
	}
}
