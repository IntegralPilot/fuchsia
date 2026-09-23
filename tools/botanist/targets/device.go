// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package targets

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"sync/atomic"
	"time"

	"go.fuchsia.dev/fuchsia/tools/botanist"
	"go.fuchsia.dev/fuchsia/tools/lib/iomisc"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/retry"
	"go.fuchsia.dev/fuchsia/tools/lib/serial"
	serialconstants "go.fuchsia.dev/fuchsia/tools/lib/serial/constants"
	"go.fuchsia.dev/fuchsia/tools/lib/subprocess"
	"go.fuchsia.dev/fuchsia/tools/net/netboot"
	"go.fuchsia.dev/fuchsia/tools/net/sshutil"
	"go.fuchsia.dev/fuchsia/tools/net/tftp"

	"golang.org/x/crypto/ssh"
)

const (
	// Command to dump the zircon debug log over serial.
	dlogCmd = "\ndlog\n"

	// String to look for in serial log that indicates system booted. From
	// https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/kernel/top/main.cc;l=116;drc=6a0fd696cde68b7c65033da57ab911ee5db75064
	bootedLogSignature = "welcome to Zircon"

	// Idling in fastboot
	fastbootIdleSignature = "USB RESET"

	// Timeout for the overall "recover device by hard power-cycling and
	// forcing into fastboot" flow
	hardRecoveryTimeoutSecs = 60

	// Timeout to observe fastbootIdleSignature before proceeding anyway
	// after hard power cycle
	fastbootIdleWaitTimeoutSecs = 10
)

// DeviceConfig contains the static properties of a target device.
type DeviceConfig struct {
	// FastbootSernum is the fastboot serial number of the device.
	FastbootSernum string `json:"fastboot_sernum"`

	// Network is the network properties of the target.
	Network NetworkProperties `json:"network"`

	// SSHKeys are the default system keys to be used with the device.
	SSHKeys []string `json:"keys,omitempty"`

	// Serial is the path to the device file for serial i/o.
	Serial string `json:"serial,omitempty"`

	// SerialMux is the path to the device's serial multiplexer.
	SerialMux string `json:"serial_mux,omitempty"`

	// PDU is an optional reference to the power distribution unit controlling
	// power delivery to the target. This will not always be present.
	PDU *targetPDU `json:"pdu,omitempty"`

	// MaxFlashAttempts is an optional integer indicating the number of
	// attempts we will make to provision this device via flashing.  If not
	// present, we will assume its value to be 1.  It should only be set >1
	// for hardware types which have silicon bugs which make flashing
	// unreliable in a way that we cannot address with any other means.
	// Other failure modes should be resolved by fixing the source of the
	// failure, not papering over it with retries.
	MaxFlashAttempts int `json:"max_flash_attempts,omitempty"`

	// TCPFastboot is an optional bool that tells botanist whether
	// a device idles in TCP fastboot or not. Botanist uses TCP
	// ffx commands to flash devices in fastboot TCP as opposed to
	// devices in USB fastboot. This distinction is important for
	// botanist to determine which commands to use to flash devices
	// in fastboot.
	TCPFastboot bool `json:"tcp_fastboot,omitempty"`

	// Metadata for a monsoon device attached to test device
	Monsoon *targetMonsoon `json:"monsoon,omitempty"`
}

// NetworkProperties are the static network properties of a target.
type NetworkProperties struct {
	// Nodename is the hostname of the device that we want to boot on.
	Nodename string `json:"nodename"`

	// IPv4Addr is the IPv4 address, if statically given. If not provided, it may be
	// resolved via the netstack's mDNS server.
	IPv4Addr string `json:"ipv4"`
}

// LoadDeviceConfigs unmarshals a slice of device configs from a given file.
func LoadDeviceConfigs(path string) ([]DeviceConfig, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("failed to read device properties file %q", path)
	}

	var configs []DeviceConfig
	if err := json.Unmarshal(data, &configs); err != nil {
		return nil, fmt.Errorf("failed to unmarshal configs: %w", err)
	}
	return configs, nil
}

// Device represents a physical Fuchsia device.
type Device struct {
	*genericFuchsiaTarget
	config   DeviceConfig
	opts     Options
	signers  []ssh.Signer
	serial   io.ReadWriteCloser
	tftp     tftp.Client
	stopping uint32
}

var _ FuchsiaTarget = (*Device)(nil)

// NewDevice returns a new device target with a given configuration.
func NewDevice(ctx context.Context, config DeviceConfig, opts Options) (*Device, error) {
	// If an SSH key is specified in the options, prepend it the configs list so that it
	// corresponds to the authorized key that would be paved.
	if opts.SSHKey != "" {
		config.SSHKeys = append([]string{opts.SSHKey}, config.SSHKeys...)
	}
	signers, err := parseOutSigners(config.SSHKeys)
	if err != nil {
		return nil, fmt.Errorf("could not parse out signers from private keys: %w", err)
	}
	var s io.ReadWriteCloser
	if config.SerialMux != "" {
		if config.FastbootSernum == "" && !config.TCPFastboot {
			s, err = serial.NewSocket(ctx, config.SerialMux)
			if err != nil {
				return nil, fmt.Errorf("unable to open: %s: %w", config.SerialMux, err)
			}
		} else {
			// We don't want to wait for the console to be ready if the device
			// is idling in Fastboot, as Fastboot does not have an interactive
			// serial console.
			s, err = serial.NewSocketWithIOTimeout(ctx, config.SerialMux, 2*time.Minute, false)
			if err != nil {
				return nil, fmt.Errorf("unable to open: %s: %w", config.SerialMux, err)
			}
		}
		// After we've made a serial connection to determine the device is ready,
		// we should close this socket since it is no longer needed. New interactions
		// with the device over serial will create new connections with the serial mux.
		s.Close()
		s = nil
	} else if config.Serial != "" {
		s, err = serial.Open(config.Serial)
		if err != nil {
			return nil, fmt.Errorf("unable to open %s: %w", config.Serial, err)
		}
	}
	base, err := newGenericFuchsia(ctx, config.Network.Nodename, config.SerialMux, config.SSHKeys, s)
	if err != nil {
		return nil, err
	}
	return &Device{
		genericFuchsiaTarget: base,
		config:               config,
		opts:                 opts,
		signers:              signers,
		serial:               s,
	}, nil
}

// Tftp returns a tftp client interface for the device.
func (t *Device) Tftp() tftp.Client {
	return t.tftp
}

// Nodename returns the name of the node.
func (t *Device) Nodename() string {
	return t.config.Network.Nodename
}

// Serial returns the serial device associated with the target for serial i/o.
func (t *Device) Serial() io.ReadWriteCloser {
	return t.serial
}

// IPv4 returns the IPv4 address of the device.
func (t *Device) IPv4() (net.IP, error) {
	return net.ParseIP(t.config.Network.IPv4Addr), nil
}

// IPv6 returns the IPv6 of the device.
// TODO(rudymathu): Re-enable mDNS resolution of IPv6 once it is no longer
// flaky on hardware.
func (t *Device) IPv6() (*net.IPAddr, error) {
	return nil, nil
}

// SSHKey returns the private SSH key path associated with the authorized key to be paved.
func (t *Device) SSHKey() string {
	return t.config.SSHKeys[0]
}

// SSHClient returns an SSH client connected to the device.
func (t *Device) SSHClient() (*sshutil.Client, error) {
	addr, err := t.IPv4()
	if err != nil {
		return nil, err
	}
	return t.sshClient(&net.IPAddr{IP: addr}, "device")
}

// Start starts the device target.
func (t *Device) Start(ctx context.Context, args []string, pbPath string, isBootTest bool) error {
	serialSocketPath := t.SerialSocketPath()

	// Set up log listener and dump kernel output to stdout.
	l, err := netboot.NewLogListener(t.Nodename())
	if err != nil {
		return fmt.Errorf("cannot listen: %w", err)
	}
	stdout, _, flush := botanist.NewStdioWriters(ctx, "device")
	defer flush()
	go func() {
		defer l.Close()
		for atomic.LoadUint32(&t.stopping) == 0 {
			data, err := l.Listen()
			if err != nil {
				continue
			}
			if _, err := stdout.Write([]byte(data)); err != nil {
				logger.Warningf(ctx, "failed to write log to stdout: %s, data: %s", err, data)
			}
		}
	}()

	// Get authorized keys from the ssh signers.
	// We cannot have signers in netboot because there is no notion
	// of a hardware backed key when you are not booting from disk
	var authorizedKeys []byte
	if !t.opts.Netboot {
		if len(t.signers) > 0 {
			for _, s := range t.signers {
				authorizedKey := ssh.MarshalAuthorizedKey(s.PublicKey())
				authorizedKeys = append(authorizedKeys, authorizedKey...)
			}
		}
	}

	bootedLogChan := make(chan error)
	if serialSocketPath != "" {
		// Start searching for the string before we reboot, otherwise we can miss it.
		go func() {
			logger.Debugf(ctx, "watching serial for string that indicates device has booted: %q", bootedLogSignature)
			socket, err := net.Dial("unix", serialSocketPath)
			if err != nil {
				bootedLogChan <- fmt.Errorf("%s: %w", serialconstants.FailedToOpenSerialSocketMsg, err)
				return
			}
			defer socket.Close()
			_, err = iomisc.ReadUntilMatchString(ctx, socket, bootedLogSignature)
			bootedLogChan <- err
		}()
	}

	// Boot Fuchsia.
	if t.config.FastbootSernum != "" || t.config.TCPFastboot {
		maxAllowedAttempts := max(t.config.MaxFlashAttempts, 1)
		var err error
		tcpFlash := false
		target := t.config.FastbootSernum
		if target == "" {
			ipv6, err := t.genericFuchsiaTarget.IPv6()
			if err != nil {
				return err
			}

			target = ipv6.String()
			tcpFlash = true
		} else {
			target = "id:" + target
		}
		t.ffx.SetTarget(target)
		for attempt := 1; attempt <= maxAllowedAttempts; attempt++ {
			logger.Debugf(ctx, "Starting flash attempt %d/%d", attempt, maxAllowedAttempts)
			bootTimeout := 10 * time.Minute
			// TODO(https://fxbug.dev/493277370): Sorrels can take longer to flash
			// when USB hubs are busy.
			if os.Getenv("FUCHSIA_DEVICE_TYPE") == "Sorrel" {
				bootTimeout = 20 * time.Minute
			}
			bootCtx, cancel := context.WithTimeout(ctx, bootTimeout)
			defer cancel()
			// ffx target bootloader boot doesn't work for Sorrel.
			if t.opts.Netboot && os.Getenv("FUCHSIA_DEVICE_TYPE") != "Sorrel" && os.Getenv("FUCHSIA_DEVICE_TYPE") != "Iris" {
				if err = t.ffx.BootloaderBoot(bootCtx, target, pbPath, tcpFlash); err == nil {
					// If successful, early exit.
					break
				}
			} else {
				if err = t.flash(bootCtx, pbPath, target, tcpFlash); err == nil {
					// If successful, early exit.
					break
				}
			}
			if attempt == maxAllowedAttempts {
				logger.Errorf(ctx, "Flashing attempt %d/%d failed: %s.", attempt, maxAllowedAttempts, err)
				return err
			} else {
				// If not successful, and we have remaining attempts,
				// try placing the device in fastboot and try again.
				logger.Warningf(ctx, "Flashing attempt %d/%d failed: %s.  Attempting to put device in fastboot.", attempt, maxAllowedAttempts, err)
				err = t.placeInFastboot(ctx)
				if err != nil {
					errWrapped := fmt.Errorf("while placing device back in fastboot: %w", err)
					logger.Errorf(ctx, "%s", errWrapped)
					return errWrapped
				}
			}
		}
	}

	if serialSocketPath != "" {
		connectionTimeout := t.connectionTimeout
		if connectionTimeout == 0 {
			connectionTimeout = 5 * time.Minute
		}
		select {
		case err := <-bootedLogChan:
			return err
		case <-time.After(connectionTimeout):
			return fmt.Errorf("timed out after %v waiting for device to boot", connectionTimeout)
		}
	}

	return nil
}

func (t *Device) flash(ctx context.Context, productBundle, target string, tcpFlash bool) error {
	// Print logs to avoid hitting the I/O timeout.
	ticker := time.NewTicker(2 * time.Minute)
	defer ticker.Stop()
	go func() {
		for range ticker.C {
			logger.Debugf(ctx, "still flashing...")
		}
	}()

	// TODO(https://fxbug.dev/42168777): Need support for ffx target flash for cuckoo tests.
	return t.ffx.Flash(ctx, target, "", productBundle, tcpFlash)
}

// placeInFastboot runs the dmc health-check command which attempts to recover
// the device by placing it in fastboot.
func (t *Device) placeInFastboot(ctx context.Context) error {
	// there should be an env var DMC_PATH with the path to dmc
	cmdline := []string{
		os.Getenv("DMC_PATH"),
		"health-check",
		t.Nodename(),
	}
	runner := subprocess.Runner{}
	// Run the dmc invocation and wait for the subprocess call to complete.
	// This usually takes ~20 seconds.
	return retry.Retry(ctx, retry.WithMaxAttempts(retry.NewConstantBackoff(time.Second), 3), func() error {
		return runner.Run(ctx, cmdline, subprocess.RunOptions{Setpgid: true})
	}, nil)
}

// Stop stops the device.
func (t *Device) Stop() error {
	t.genericFuchsiaTarget.Stop()
	atomic.StoreUint32(&t.stopping, 1)
	return nil
}

// Wait waits for the device target to stop.
func (t *Device) Wait(context.Context) error {
	return ErrUnimplemented
}

// Config returns fields describing the target.
func (t *Device) TestConfig(expectsSSH bool) (any, error) {
	return TestbedConfig(t, expectsSSH, &testbedConfigOptions{PDU: t.config.PDU, Monsoon: t.config.Monsoon})
}

func parseOutSigners(keyPaths []string) ([]ssh.Signer, error) {
	if len(keyPaths) == 0 {
		return nil, errors.New("must supply SSH keys in the config")
	}
	var keys [][]byte
	for _, keyPath := range keyPaths {
		p, err := os.ReadFile(keyPath)
		if err != nil {
			return nil, fmt.Errorf("could not read SSH key file %q: %w", keyPath, err)
		}
		keys = append(keys, p)
	}

	var signers []ssh.Signer
	for _, p := range keys {
		signer, err := ssh.ParsePrivateKey(p)
		if err != nil {
			return nil, err
		}
		signers = append(signers, signer)
	}
	return signers, nil
}
