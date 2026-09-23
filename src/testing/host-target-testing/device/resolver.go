// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package device

import (
	"context"
	"fmt"
	"net"
	"strconv"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/tools/botanist/targets"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

type DeviceResolver interface {
	// NodeName returns a nodename for a device.
	NodeName() string

	// Resolve the device's nodename into an ssh address.
	ResolveSshAddress(ctx context.Context) (string, error)

	// Block until the device appears to be in fastboot.
	WaitToFindDeviceInFastboot(ctx context.Context, serialNumber string) (string, error)

	// Block until the device appears to be in netboot.
	WaitToFindDeviceInNetboot(ctx context.Context) (string, error)
}

// ConstantHostResolver returns a fixed hostname for the specified nodename.
type ConstantHostResolver struct {
	nodeName string
	host     string
	sshPort  int
}

// NewConstantHostResolver constructs a fixed host.
func NewConstantHostResolver(
	ctx context.Context,
	nodeName string,
	host string,
	sshPort int,
) ConstantHostResolver {
	return ConstantHostResolver{
		nodeName: nodeName,
		host:     host,
		sshPort:  sshPort,
	}
}

func (r ConstantHostResolver) NodeName() string {
	return r.nodeName
}

func (r ConstantHostResolver) ResolveSshAddress(ctx context.Context) (string, error) {
	host := r.host
	if strings.HasPrefix(host, "[") && strings.HasSuffix(host, "]") {
		host = host[1 : len(host)-1]
	}
	return net.JoinHostPort(host, strconv.Itoa(r.sshPort)), nil
}

func (r ConstantHostResolver) WaitToFindDeviceInFastboot(ctx context.Context, serialNumber string) (string, error) {
	// We have no way to tell if the device is in fastboot, so just exit.
	logger.Warningf(ctx, "ConstantHostResolver cannot tell if device is in fastboot, assuming nodename is %s", r.nodeName)
	return r.nodeName, nil
}

func (r ConstantHostResolver) WaitToFindDeviceInNetboot(ctx context.Context) (string, error) {
	// We have no way to tell if the device is in netboot, so just exit.
	logger.Warningf(ctx, "ConstantHostResolver cannot tell if device is in netboot, assuming nodename is %s", r.nodeName)
	return r.nodeName, nil
}

// MdnsResolver resolves a nodename into a hostname using mDNS.
type MdnsResolver struct {
	nodeName string
	sshPort  int
}

// NewMdnsResolver constructs a new `MdnsResolver` for the specific nodename.
func NewMdnsResolver(
	ctx context.Context,
	nodeName string,
	sshPort int,
) *MdnsResolver {
	return &MdnsResolver{
		nodeName: nodeName,
		sshPort:  sshPort,
	}
}

func (r *MdnsResolver) NodeName() string {
	return r.nodeName
}

func (r *MdnsResolver) ResolveSshAddress(ctx context.Context) (string, error) {
	ipv4Addr, ipv6Addr, err := targets.ResolveIP(ctx, r.nodeName)
	if err != nil {
		return "", err
	}

	var ip string
	if ipv6Addr.IP != nil {
		ip = ipv6Addr.String()
	} else if ipv4Addr != nil {
		ip = ipv4Addr.String()
	} else {
		return "", fmt.Errorf("cannot resolve target ssh address via mDNS lookup")
	}

	return net.JoinHostPort(ip, strconv.Itoa(r.sshPort)), nil
}

func (r *MdnsResolver) WaitToFindDeviceInFastboot(ctx context.Context, serialNumber string) (string, error) {
	// We have no way to tell if the device is in fastboot, so just exit.
	logger.Warningf(ctx, "MdnsResolver cannot tell if device is in fastboot, assuming nodename is %s", r.nodeName)
	return r.nodeName, nil
}

func (r *MdnsResolver) WaitToFindDeviceInNetboot(ctx context.Context) (string, error) {
	// We have no way to tell if the device is in netboot, so just exit.
	logger.Warningf(ctx, "MdnsResolver cannot tell if device is in netboot, assuming nodename is %s", r.nodeName)
	return r.nodeName, nil
}

// FfxResolver uses `ffx target list` to resolve a nodename into a hostname.
type FfxResolver struct {
	ffx      *ffx.FFXTool
	nodeName string
}

// NewFfxResolver constructs a new `FfxResolver` for the specific nodename.
func NewFfxResolver(
	ctx context.Context,
	ffxInst *ffx.FFXTool,
	nodeName string,
	address string,
) (DeviceResolver, error) {
	if nodeName == "" && address != "" {
		ffxInst.SetTarget(address)
		if err := ffxInst.TargetWait(ctx, address); err != nil {
			return nil, fmt.Errorf("failed waiting for target with address %s: %w", address, err)
		}
		entries, errList := ffxInst.TargetList(ctx, address, 0)
		if errList != nil {
			return nil, fmt.Errorf("failed to list devices: %w", errList)
		}
		if len(entries) == 0 {
			return nil, fmt.Errorf("failed to find target with address: %s after wait", address)
		}
		logger.Infof(ctx, "resolved device name %v from address %v", entries[0].NodeName, address)
		nodeName = entries[0].NodeName
	}

	if nodeName == "" {
		if err := ffxInst.TargetWait(ctx, ""); err != nil {
			return nil, fmt.Errorf("failed waiting for target: %w", err)
		}
		entries, listErr := ffxInst.TargetList(ctx, "", 0)
		if listErr != nil {
			return nil, fmt.Errorf("failed to list devices: %w", listErr)
		}

		if len(entries) != 1 {
			return nil, fmt.Errorf("cannot use empty nodename with multiple devices: %v", entries)
		}

		nodeName = entries[0].NodeName
	}

	return &FfxResolver{
		ffx:      ffxInst,
		nodeName: nodeName,
	}, nil
}

func (r *FfxResolver) NodeName() string {
	return r.nodeName
}

func (r *FfxResolver) ResolveName(ctx context.Context) (string, error) {
	nodeName := r.NodeName()
	logger.Infof(ctx, "resolving the nodename %v hostname", nodeName)

	targets, err := r.ffx.TargetList(ctx, nodeName, 0)
	if err != nil {
		return "", err
	}

	logger.Infof(ctx, "resolved the nodename %v to %v", nodeName, targets)

	if len(targets) == 0 {
		return "", fmt.Errorf("no addresses found for nodename: %v", nodeName)
	}

	if len(targets) > 1 {
		return "", fmt.Errorf("multiple addresses found for nodename %v: %v", nodeName, targets)
	}

	target := targets[0]

	if len(target.Addresses) == 0 {
		return "", fmt.Errorf("no address found for nodename %v: %v", nodeName, target)
	}

	for _, v := range target.Addresses {
		if v.Type == "Ip" {
			return v.IP, nil
		}
	}

	return "", fmt.Errorf("no IP address found for nodename %v: %v", nodeName, target)
}

func (r *FfxResolver) ResolveSshAddress(ctx context.Context) (string, error) {
	nodeName := r.NodeName()
	logger.Infof(ctx, "resolving the nodename %v ssh address", nodeName)

	targets, err := r.ffx.TargetList(ctx, nodeName, 0)
	if err != nil {
		return "", fmt.Errorf("failed to list devices: %w", err)
	}

	for _, target := range targets {
		for _, addr := range target.Addresses {
			if addr.Type == "Ip" {
				if addr.SSHPort != 0 {
					return net.JoinHostPort(addr.IP, strconv.Itoa(int(addr.SSHPort))), nil
				}
				return net.JoinHostPort(addr.IP, "22"), nil
			}
		}
	}

	return "", fmt.Errorf("no IP address found for nodename %v", nodeName)
}

func (r *FfxResolver) WaitToFindDeviceInFastboot(ctx context.Context, serialNumber string) (string, error) {
	nodeName := r.NodeName()

	// Wait for the device to be listening in netboot.
	logger.Infof(ctx, "waiting for the device to be listening on the node name:%v serial number:%v", nodeName, serialNumber)

	attempt := 0
	for {
		attempt += 1

		entries, err := r.ffx.TargetList(ctx, "", 12*time.Second)
		if err == nil {
			for _, entry := range entries {
				logger.Infof(ctx, "device node name:%v serial number:%v is in %v", entry.NodeName, entry.Serial, entry.TargetState)
				if entry.TargetState == "Fastboot" {
					if entry.Serial == serialNumber {
						logger.Infof(ctx, "device node name:%v serial number:%v is listening on %v", entry.NodeName, entry.Serial, entry)
						return entry.Serial, nil
					}

					if entry.NodeName == nodeName {
						logger.Infof(ctx, "device node name:%v serial number:%v is listening on %v", entry.NodeName, entry.Serial, entry)
						if entry.Serial != "" {
							return entry.Serial, nil
						}
						if len(entry.Addresses) > 0 {
							addr := entry.Addresses[0].IP
							if strings.Contains(addr, ":") && !strings.HasPrefix(addr, "[") {
								addr = fmt.Sprintf("[%s]", addr)
							}
							return addr, nil
						}
						return entry.NodeName, nil
					}
				}
			}
			logger.Infof(ctx, "attempt %d waiting for device node name:%v serial number:%v to boot into fastboot", attempt, nodeName, serialNumber)
			time.Sleep(5 * time.Second)
		} else {
			logger.Infof(ctx, "attempt %d failed to resolve device node name:%v serial number:%v: %v", attempt, nodeName, serialNumber, err)
			time.Sleep(5 * time.Second)
		}
	}
}

func (r *FfxResolver) WaitToFindDeviceInNetboot(ctx context.Context) (string, error) {
	// Exit early if ffx is not configured to listen for devices in zedboot.
	// With strict mode, zedboot discovery is generally off or unneeded unless extra config
	// is explicitly provided upfront. Let's stub logically if unsupported.
	supported, err := r.ffx.SupportsZedbootDiscovery(ctx)
	if err == nil && !supported {
		logger.Warningf(ctx, "ffx not configured to listen for devices in zedboot, assuming nodename is %s", r.nodeName)
		return r.nodeName, nil
	}

	nodeName := r.NodeName()

	// Wait for the device to be listening in netboot.
	logger.Infof(ctx, "waiting for the to be listening on the nodename: %v", nodeName)

	attempt := 0
	for {
		attempt += 1

		entries, err := r.ffx.TargetList(ctx, nodeName, 0)
		if err == nil {
			for _, entry := range entries {
				logger.Infof(ctx, "device is in %v", entry.TargetState)
				if entry.TargetState == "Zedboot (R)" {
					logger.Infof(ctx, "device %v is listening on %v", entry.NodeName, entry)
					return entry.NodeName, nil
				}
			}
			logger.Infof(ctx, "attempt %d waiting for device %v to boot into zedboot", attempt, nodeName)
			time.Sleep(5 * time.Second)
		} else {
			logger.Infof(ctx, "attempt %d failed to resolve nodename %v: %v", attempt, nodeName, err)
			time.Sleep(5 * time.Second)
		}
	}
}
