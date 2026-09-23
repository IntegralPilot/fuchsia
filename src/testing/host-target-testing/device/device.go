// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package device

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"net"
	"net/url"
	"os"
	"strconv"
	"strings"
	"sync/atomic"
	"time"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/artifacts"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/build"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/packages"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/paver"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/sl4f"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"go.fuchsia.dev/fuchsia/tools/lib/retry"
	"go.fuchsia.dev/fuchsia/tools/net/sshutil"
	"golang.org/x/crypto/ssh"
)

const rebootCheckPath = "/tmp/ota_test_should_reboot"

// Client manages the connection to the device.
type Client struct {
	resolverMode             string
	nodeName                 string
	host                     string
	sshPort                  int
	ffx                      *ffx.FFXTool
	sshClient                *sshutil.Client
	initialMonotonicTime     time.Time
	workaroundBrokenTimeSkip bool
	bootCounter              *uint32
	repoPort                 int
	flashRetrySleep          time.Duration
}

// AddrResolver wrapper for sshutil
type addrResolver struct {
	client *Client
}

func (a *addrResolver) Resolve(ctx context.Context) (net.Addr, error) {
	resolver, err := a.client.getResolver(ctx)
	if err != nil {
		return nil, err
	}
	addr, err := resolver.ResolveSshAddress(ctx)
	if err != nil {
		return nil, err
	}
	return net.ResolveTCPAddr("tcp", addr)
}

// NewClient creates a new Client.
func NewClient(
	ctx context.Context,
	repoPort int,
	resolverMode string,
	nodeName string,
	host string,
	sshPort int,
	privateKey ssh.Signer,
	sshConnectBackoff retry.Backoff,
	workaroundBrokenTimeSkip bool,
	serialConn *SerialConn,
	ffxTool *ffx.FFXTool,
) (*Client, error) {
	sshConfig, err := newSSHConfig(privateKey)
	if err != nil {
		return nil, err
	}

	c := &Client{
		resolverMode:             resolverMode,
		nodeName:                 nodeName,
		host:                     host,
		sshPort:                  sshPort,
		ffx:                      ffxTool,
		workaroundBrokenTimeSkip: workaroundBrokenTimeSkip,
		repoPort:                 repoPort,
		flashRetrySleep:          12 * time.Second,
	}

	sshClient, err := sshutil.NewClient(
		ctx,
		&addrResolver{
			client: c,
		},
		sshConfig,
		sshConnectBackoff,
	)
	if err != nil {
		return nil, err
	}

	c.sshClient = sshClient

	bootCounter := new(uint32)
	if serialConn != nil {
		go func() {
			for {
				line, err := serialConn.ReadLine()
				if err != nil {
					logger.Errorf(ctx, "failed to read from serial: %v", err)
					break
				}
				if strings.HasSuffix(line, "Welcome to Zircon\n") {
					atomic.AddUint32(bootCounter, 1)
				}
			}
		}()
	}

	c.bootCounter = bootCounter

	if err := c.postConnectSetup(ctx, ffxTool); err != nil {
		c.Close()
		return nil, err

	}

	return c, nil
}

// SetFFXTool sets the active ffx tool used by the client and its dynamic resolver.
func (c *Client) SetFFXTool(t *ffx.FFXTool) {
	c.ffx = t
}

// getResolver creates a short-lived resolver based on the current client state.
func (c *Client) getResolver(ctx context.Context) (DeviceResolver, error) {
	switch c.resolverMode {
	case "constant":
		return NewConstantHostResolver(ctx, c.nodeName, c.host, c.sshPort), nil
	case "ffx", "mdns": // Both use ffx. ffx is preferred over a raw mDNS resolver
		// because it uses mDNS but is more general (e.g. supports USB).
		return NewFfxResolver(ctx, c.ffx, c.nodeName, c.host)
	default:
		return nil, fmt.Errorf("invalid resolver mode: %s", c.resolverMode)
	}
}

// Name returns the node name of the device.
func (c *Client) Name() string {
	return c.nodeName
}

// Construct a new `ssh.ClientConfig` for a given key file, or return an error if
// the key is invalid.
func newSSHConfig(privateKey ssh.Signer) (*ssh.ClientConfig, error) {
	config := &ssh.ClientConfig{
		User: "fuchsia",
		Auth: []ssh.AuthMethod{
			ssh.PublicKeys(privateKey),
		},
		HostKeyCallback: ssh.InsecureIgnoreHostKey(),
		Timeout:         30 * time.Second,
	}

	return config, nil
}

// Close the Client connection
func (c *Client) Close() {
	c.sshClient.Close()
}

// Run all setup steps after we've connected to a device.
func (c *Client) postConnectSetup(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
) error {
	// TODO(https://fxbug.dev/42154680): The device might drop connections
	// early after boot when the RTC is updated, which typically happens
	// about 10 seconds after boot. To avoid this, if we find that we
	// connected before 15s, we'll disconnect, sleep, then connect again.
	if c.workaroundBrokenTimeSkip {
		logger.Infof(ctx, "Sleeping 15s in case https://fxbug.dev/42154685 causes a spurious disconnection")
		time.Sleep(15 * time.Second)

		if err := c.sshClient.Reconnect(ctx); err != nil {
			return err
		}
	}

	c.setInitialMonotonicTime(ctx, ffxTool)

	return nil
}

func (c *Client) Reconnect(ctx context.Context, ffxTool *ffx.FFXTool) error {
	if err := c.sshClient.Reconnect(ctx); err != nil {
		return err
	}

	return c.postConnectSetup(ctx, ffxTool)
}

func (c *Client) getTargetSpecifier(ctx context.Context, ffxTool *ffx.FFXTool) (string, error) {
	if ffxTool == nil {
		return "", fmt.Errorf("ffx is nil")
	}
	var target string
	nodeName := c.Name()

	// Prioritize finding an address or valid specifier using ffxTool.
	// This handles devices known only to ffx (like Pontis or user emulators).
	if nodeName != "" {
		if resolver, err := NewFfxResolver(ctx, ffxTool, nodeName, ""); err == nil {
			if addr, err := resolver.ResolveSshAddress(ctx); err == nil {
				target = addr
				logger.Infof(ctx, "getTargetSpecifier: resolved target %q using FfxResolver", target)
			}
		}
	}

	// If that fails, fall back to standard resolver.
	if target == "" {
		if resolver, err := c.getResolver(ctx); err == nil {
			if addr, err := resolver.ResolveSshAddress(ctx); err == nil {
				target = addr
				logger.Infof(ctx, "getTargetSpecifier: resolved target %q using standard resolver", target)
			}
		}
	}

	// Fall back to node name if still empty.
	if target == "" {
		target = nodeName
		logger.Infof(ctx, "getTargetSpecifier: resolved target %q using node name", target)
	}

	if target == "" {
		target = ffxTool.GetTarget()
		logger.Infof(ctx, "getTargetSpecifier: resolved target %q using ffxTool default target", target)
	}

	// Clean up IPv6 brackets if needed.
	if target != "" {
		if host, _, err := net.SplitHostPort(target); err == nil {
			target = host
		}
		if strings.Contains(target, ":") && !strings.HasPrefix(target, "[") {
			target = fmt.Sprintf("[%s]", target)
		}
	}

	// Verify it is a valid target specifier for strict mode!
	if target != "" {
		// Strip brackets and scope ID to validate if the target is an IP address.
		ipStr := strings.Trim(target, "[]")
		if idx := strings.Index(ipStr, "%"); idx != -1 {
			ipStr = ipStr[:idx]
		}

		if net.ParseIP(ipStr) == nil &&
			!strings.HasPrefix(target, "usb:") && !strings.HasPrefix(target, "vsock:") && !strings.HasPrefix(target, "id:") {
			return "", fmt.Errorf("cannot resolve a valid target specifier for strict mode (got node name %q). Ffx strict mode requires an IP address or a valid prefix (\"id:<serial-number>\", \"usb:cid\", \"vsock:cid\").", target)
		}
	} else {
		return "", fmt.Errorf("cannot resolve a target specifier. Ffx strict mode requires an IP address or a valid prefix (\"id:<serial-number>\", \"usb:cid\", \"vsock:cid\").")
	}

	return target, nil
}

func (c *Client) setInitialMonotonicTime(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
) {
	targetSpecifier, err := c.getTargetSpecifier(ctx, ffxTool)
	if err != nil {
		logger.Warningf(ctx, "failed to get target specifier for setInitialMonotonicTime: %v", err)
		return
	}
	args := []string{"--target", targetSpecifier}
	args = append(args, "--machine", "raw", "target", "get-time")

	t0 := time.Now()
	stdout, err := ffxTool.RunAndGetOutput(ctx, args...)
	t1 := time.Now()

	if err == nil {
		stdoutStr := strings.TrimSpace(stdout)
		t, errParse := strconv.ParseInt(stdoutStr, 10, 64)

		if errParse == nil {
			monotonicTime := time.Duration(t) * time.Nanosecond
			latency := t1.Sub(t0) / 2
			c.initialMonotonicTime = t1.Add(-(monotonicTime + latency))
			return
		}
	}

	logger.Warningf(ctx, "failed to get time with ffx: %v", err)
	logger.Warningf(ctx, "resetting time to zero")
	c.initialMonotonicTime = time.Time{}
}

func (c *Client) getEstimatedMonotonicTime() time.Duration {
	if c.initialMonotonicTime.IsZero() {
		return 0
	}
	return time.Since(c.initialMonotonicTime)
}

// Run a command to completion on the remote device and write STDOUT and STDERR
// to the passed in io.Writers.
func (c *Client) Run(ctx context.Context, command []string, stdout io.Writer, stderr io.Writer) error {
	return c.sshClient.Run(ctx, command, stdout, stderr)
}

// DisconnectionListener returns a channel that is closed when the client is
// disconnected.
func (c *Client) DisconnectionListener() <-chan struct{} {
	return c.sshClient.DisconnectionListener()
}

func (c *Client) GetSSHConnection(ctx context.Context) (string, error) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	cmd := []string{"PATH=''", "echo", "$SSH_CONNECTION"}
	if err := c.Run(ctx, cmd, &stdout, &stderr); err != nil {
		return "", fmt.Errorf("failed to read SSH_CONNECTION: %w: %s", err, string(stderr.Bytes()))
	}
	return strings.Split(string(stdout.Bytes()), " ")[0], nil
}

func (c *Client) GetSystemImageMerkle(ctx context.Context) (build.MerkleRoot, error) {
	const systemImageMeta = "/system/meta"
	merkleBytes, err := c.ReadRemotePath(ctx, systemImageMeta)
	if err != nil {
		return build.MerkleRoot{}, err
	}

	return build.DecodeMerkleRoot([]byte(strings.TrimSpace(string(merkleBytes))))
}

// Reboot asks the device to reboot. It waits until the device reconnects
// before returning.
func (c *Client) Reboot(ctx context.Context, ffxTool *ffx.FFXTool) error {
	logger.Infof(ctx, "rebooting")

	return c.ExpectReboot(ctx, ffxTool, func() error {
		// Run the reboot in the background, which gives us a chance to
		// observe us successfully executing the reboot command.
		return c.RunReboot(ctx)
	})
}

// RunReboot runs the reboot command
func (c *Client) RunReboot(ctx context.Context) error {
	cmd := []string{"dm", "reboot", "&", "exit", "0"}
	if err := c.Run(ctx, cmd, os.Stdout, os.Stderr); err != nil {
		// If the device rebooted before ssh was able to tell
		// us the command ran, it will tell us the session
		// exited without passing along an exit code. So,
		// ignore that specific error.
		var exitErr *ssh.ExitMissingError
		if errors.As(err, &exitErr) {
			logger.Infof(ctx, "ssh disconnected before returning a status")
		} else {
			return fmt.Errorf("failed to reboot: %w", err)
		}
	}
	return nil
}

// RebootToBootloader asks the device to reboot into the bootloader. It
// waits until the device disconnects before returning.
func (c *Client) RebootToBootloader(ctx context.Context, ffxTool *ffx.FFXTool) error {
	logger.Infof(ctx, "Rebooting to bootloader")

	targetSpecifier, err := c.getTargetSpecifier(ctx, ffxTool)
	if err != nil {
		return err
	}

	return c.ExpectDisconnect(ctx, func() error {
		return ffxTool.RebootToBootloader(ctx, targetSpecifier)
	})
}

// RebootToRecovery asks the device to reboot into the recovery partition. It
// waits until the device disconnects before returning.
func (c *Client) RebootToRecovery(ctx context.Context) error {
	logger.Infof(ctx, "Rebooting to recovery")

	return c.ExpectDisconnect(ctx, func() error {
		// Run the reboot in the background, which gives us a chance to
		// observe us successfully executing the reboot command.
		cmd := []string{"dm", "reboot-recovery", "&", "exit", "0"}
		if err := c.Run(ctx, cmd, os.Stdout, os.Stderr); err != nil {
			// If the device rebooted before ssh was able to tell
			// us the command ran, it will tell us the session
			// exited without passing along an exit code. So,
			// ignore that specific error.
			var exitErr *ssh.ExitMissingError
			if errors.As(err, &exitErr) {
				logger.Infof(ctx, "ssh disconnected before returning a status")
			} else {
				return fmt.Errorf("failed to reboot into recovery: %w", err)
			}
		}

		return nil
	})
}

// Suspend asks the device to suspend. It waits until the device disconnects
// before returning.
func (c *Client) Suspend(ctx context.Context) error {
	logger.Infof(ctx, "Suspending")

	return c.ExpectDisconnect(ctx, func() error {
		// Run the suspend in the background, which gives us a chance to
		// observe us successfully executing the suspend command.
		cmd := []string{"dm", "suspend", "&", "exit", "0"}
		if err := c.Run(ctx, cmd, os.Stdout, os.Stderr); err != nil {
			// If the device suspends before ssh was able to tell
			// us the command ran, it will tell us the session
			// exited without passing along an exit code. So,
			// ignore that specific error.
			var exitErr *ssh.ExitMissingError
			if errors.As(err, &exitErr) {
				logger.Infof(ctx, "ssh disconnected before returning a status")
			} else {
				return fmt.Errorf("failed to suspend: %w", err)
			}
		}

		return nil
	})
}

func (c *Client) ExpectDisconnect(ctx context.Context, f func() error) error {
	ch := c.DisconnectionListener()

	if err := f(); err != nil {
		return err
	}

	// Wait until we get a signal that we have disconnected
	select {
	case <-ch:
	case <-ctx.Done():
		return fmt.Errorf("device did not disconnect: %w", ctx.Err())
	}

	logger.Infof(ctx, "device disconnected")

	return nil
}

// ExpectReboot prepares a device for a reboot, runs a closure `f` that should
// reboot the device, then finally verifies whether a reboot actually took
// place. It does this by writing a unique value to
// `/tmp/ota_test_should_reboot`, then executing the closure. After we
// reconnect, we check if `/tmp/ota_test_should_reboot` exists. If not, exit
// with `nil`. Otherwise, we failed to reboot, or some competing test is also
// trying to reboot the device. Either way, err out.
func (c *Client) ExpectReboot(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	f func() error,
) error {
	// Generate a unique value.
	b := make([]byte, 16)
	_, err := rand.Read(b)
	if err != nil {
		return fmt.Errorf("failed to generate a unique boot number: %w", err)
	}

	// Encode the id into hex so we can write it through the shell.
	bootID := hex.EncodeToString(b)

	// Write the value to the file. Err if the file already exists by setting the
	// noclobber setting.
	cmd := fmt.Sprintf(
		`(
			set -C &&
			PATH= echo "%s" > "%s"
        )`, bootID, rebootCheckPath)

	// Delete the stale reboot check file if it exists.
	exists, err := c.RemoteFileExists(ctx, rebootCheckPath)
	if err == nil && exists {
		if err := c.DeleteRemotePath(ctx, rebootCheckPath); err != nil {
			logger.Warningf(ctx, "failed to delete stale reboot check file %q: %v", rebootCheckPath, err)
		}
	}

	if err := c.Run(ctx, strings.Fields(cmd), os.Stdout, os.Stderr); err != nil {
		return fmt.Errorf("failed to write reboot check file: %w", err)
	}

	// As a sanity check, make sure the file actually exists and has the correct
	// value.
	b, err = c.ReadRemotePath(ctx, rebootCheckPath)
	if err != nil {
		return fmt.Errorf("failed to read reboot check file: %w", err)
	}
	actual := strings.TrimSpace(string(b))

	if actual != bootID {
		return fmt.Errorf("reboot check file has wrong value: expected %q, got %q", bootID, actual)
	}

	// Look up the boot count before we reboot the device.
	initialBootCount := *c.bootCounter

	ch := c.DisconnectionListener()

	if err := f(); err != nil {
		return err
	}

	// Wait until we get a signal that we have disconnected
	select {
	case <-ch:
	case <-ctx.Done():
		return fmt.Errorf("device did not disconnect: %w", ctx.Err())
	}

	logger.Infof(ctx, "device disconnected, waiting for device to boot")

	if err := c.Reconnect(ctx, ffxTool); err != nil {
		return fmt.Errorf("failed to reconnect: %w", err)
	}

	// We've reconnected to the device, so count how many times we've rebooted.
	afterBootCount := *c.bootCounter

	// If we have boot counting enabled (signified by the initial boot
	// count not being zero), then check how many times we rebooted. It
	// should be 1 more than our initial count. If not, error out.
	logger.Infof(ctx, "device appears to have rebooted %d times", afterBootCount-initialBootCount)
	if initialBootCount != 0 && initialBootCount+1 != afterBootCount {
		return fmt.Errorf("device appears to have rebooted more than once! %d != %d", initialBootCount, afterBootCount)
	}

	// We reconnected to the device. Check that the reboot check file doesn't exist.
	exists, err = c.RemoteFileExists(ctx, rebootCheckPath)
	if err != nil {
		return fmt.Errorf(`failed to check if %q exists: %w`, rebootCheckPath, err)
	}
	if exists {
		// The reboot file exists. This could have happened because either we
		// didn't reboot, or some other test is also trying to reboot the
		// device. We can distinguish the two by comparing the file contents
		// with the bootID we wrote earlier.
		b, err = c.ReadRemotePath(ctx, rebootCheckPath)
		if err != nil {
			return fmt.Errorf("failed to read reboot check file: %w", err)
		}
		actual := strings.TrimSpace(string(b))

		// If the contents match, then we failed to reboot.
		if actual == bootID {
			return fmt.Errorf("reboot check file exists after reboot, device did not reboot")
		}

		return fmt.Errorf(
			"reboot check file exists after reboot, and has unexpected value: expected %q, got %q",
			bootID,
			actual,
		)
	}

	return nil
}

// ValidateStaticPackages checks that all static packages have no missing blobs.
func (c *Client) ValidateStaticPackages(ctx context.Context) error {
	logger.Infof(ctx, "validating static packages")

	path := "/pkgfs/ctl/validation/missing"
	f, err := c.ReadRemotePath(ctx, path)
	if err != nil {
		return fmt.Errorf("error reading %q: %w", path, err)
	}

	merkles := strings.TrimSpace(string(f))
	if merkles != "" {
		return fmt.Errorf("static packages are missing the following blobs:\n%s", merkles)
	}

	logger.Infof(ctx, "all static package blobs are accounted for")
	return nil
}

// ReadRemotePath read a file off the remote device.
func (c *Client) ReadRemotePath(ctx context.Context, path string) ([]byte, error) {
	var stdout bytes.Buffer
	var stderr bytes.Buffer
	cmd := fmt.Sprintf(
		`(
		test -e "%s" &&
		while IFS='' read f; do
			echo "$f";
		done < "%s" &&
		if [ ! -z "$f" ];
			then echo "$f";
		fi
		)`, path, path)
	if err := c.Run(ctx, strings.Fields(cmd), &stdout, &stderr); err != nil {
		return nil, fmt.Errorf("failed to read %q: %w: %s", path, err, string(stderr.Bytes()))
	}

	return stdout.Bytes(), nil
}

// DeleteRemotePath deletes a file off the remote device.
func (c *Client) DeleteRemotePath(ctx context.Context, path string) error {
	var stderr bytes.Buffer
	cmd := []string{"PATH=''", "rm", path}
	if err := c.Run(ctx, cmd, os.Stdout, &stderr); err != nil {
		return fmt.Errorf("failed to delete %q: %w: %s", path, err, string(stderr.Bytes()))
	}

	return nil
}

// RemoteFileExists checks if a file exists on the remote device.
func (c *Client) RemoteFileExists(ctx context.Context, path string) (bool, error) {
	var stderr bytes.Buffer
	cmd := []string{"PATH=''", "test", "-e", path}

	if err := c.Run(ctx, cmd, io.Discard, &stderr); err != nil {
		if e, ok := err.(*ssh.ExitError); ok {
			if e.ExitStatus() == 1 {
				return false, nil
			}
		}
		return false, fmt.Errorf("error reading %q: %w: %s", path, err, string(stderr.Bytes()))
	}

	return true, nil
}

// RegisterPackageRepository adds the repository as a repository via ffxTool.
// If rewritePackages is not nil, the rewrite rule will only affect the passed packages.
func (c *Client) RegisterPackageRepository(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	repo *packages.Server,
	repoName string,
	createRewriteRule bool,
	rewritePackages []string,
	sshAddr string,
) error {
	logger.Infof(ctx, "registering package repository: %s", repo.Dir)

	// From ffx's viewpoint, this repo is served on localhost, not via the qemu link local
	// scope in the json config file for the target. Hence, we need to rewrite the URL
	// to let ffx fetch the config from [::1].
	url, err := url.Parse(repo.URL)
	if err != nil {
		return err
	}
	ffx_repo_url := fmt.Sprintf("http://[::1]:%v%v", url.Port(), url.Path)

	if createRewriteRule {
		target := sshAddr
		if target == "" {
			var err error
			target, err = c.getTargetSpecifier(ctx, ffxTool)
			if err != nil {
				logger.Warningf(ctx, "failed to get target specifier for RegisterPackageRepository: %v", err)
				target = c.nodeName
			}
		}
		if err := ffxTool.RegisterPackageRepository(ctx, target, ffx_repo_url); err != nil {
			logger.Errorf(ctx,
				"%v %v\n%v",
				"unable to register package repository via ffx:",
				err,
				"registering via ffx might be unsupported on the target, falling back to pkgctl.")
			cmd := []string{"pkgctl", "repo", "add", "url", "-n", repoName, repo.URL}
			if err := c.Run(ctx, cmd, os.Stdout, os.Stderr); err != nil {
				return err
			}
		}
		logger.Infof(ctx, "establishing rewriting rule for: %s", repo.URL)
		ruleTemplate := `'{"version":"1","content":[
			{"host_match":"fuchsia.com","host_replacement":"%[1]v","path_prefix_match":"/","path_prefix_replacement":"/"},
			{"host_match":"chromium.org","host_replacement":"%[1]v","path_prefix_match":"/","path_prefix_replacement":"/"}
		]}'`
		if rewritePackages != nil {
			ruleTemplate = `'{"version":"1","content":[`
			for i, p := range rewritePackages {
				if i > 0 {
					ruleTemplate += ","
				}
				ruleTemplate += `{
					"host_match":"fuchsia.com",
					"host_replacement":"%[1]v",
					"path_prefix_match":"/` + p + `",
					"path_prefix_replacement":"/` + p + `"
				}, {
					"host_match":"fuchsia.com",
					"host_replacement":"%[1]v",
					"path_prefix_match":"/` + p + `/0",
					"path_prefix_replacement":"/` + p + `/0"
				}`
			}
			ruleTemplate += `]}'`
		}
		cmd := []string{"pkgctl", "rule", "replace", "json", fmt.Sprintf(ruleTemplate, repoName)}
		return c.Run(ctx, cmd, os.Stdout, os.Stderr)
	} else {
		if err := ffxTool.RegisterPackageRepository(ctx, c.nodeName, ffx_repo_url); err != nil {
			logger.Errorf(ctx,
				"%v %v\n%v",
				"unable to register package repository via ffx:",
				err,
				"registering via ffx might be unsupported on the target, falling back to pkgctl.")
			cmd := []string{"pkgctl", "repo", "add", "url", repo.URL}
			return c.Run(ctx, cmd, os.Stdout, os.Stderr)
		} else {
			return nil
		}
	}
}

func (c *Client) ServePackageRepository(
	ctx context.Context,
	repo *packages.Repository,
	repoName string,
) (*packages.Server, error) {
	// Make sure the device doesn't have any broken static packages.
	if err := c.ValidateStaticPackages(ctx); err != nil {
		return nil, err
	}

	// Tell the device to connect to our repository.
	localHostname, err := c.GetSSHConnection(ctx)
	if err != nil {
		return nil, err
	}

	// Serve the repository before the test begins.
	server, err := repo.Serve(ctx, localHostname, repoName, c.repoPort)
	if err != nil {
		return nil, err
	}

	return server, nil
}

func (c *Client) StartRpcSession(ctx context.Context, ffxTool *ffx.FFXTool, repo *packages.Repository) (*sl4f.Client, error) {
	logger.Infof(ctx, "connecting to sl4f")
	startTime := time.Now()

	// Configure the target to use this repository as "fuchsia-pkg://host_target_testing_sl4f".
	repoName := "host-target-testing-sl4f"
	repoServer, err := c.ServePackageRepository(ctx, repo, repoName)
	if err != nil {
		return nil, fmt.Errorf("error serving repo to device: %w", err)
	}
	defer repoServer.Shutdown(ctx)

	resolver, err := c.getResolver(ctx)
	if err != nil {
		return nil, fmt.Errorf("error getting resolver: %w", err)
	}

	sshAddr, err := resolver.ResolveSshAddress(ctx)
	if err != nil {
		return nil, fmt.Errorf("error resolving device host: %w", err)
	}

	if err := c.RegisterPackageRepository(ctx, ffxTool, repoServer, repoName, true, []string{"sl4f", "start_sl4f"}, sshAddr); err != nil {
		return nil, fmt.Errorf("error registering repository with target: %w", err)
	}

	deviceHostname, _, err := net.SplitHostPort(sshAddr)
	if err != nil {
		return nil, fmt.Errorf("error parsing ssh address %v: %w", sshAddr, err)
	}

	rpcClient, err := sl4f.NewClient(ctx, c.sshClient, net.JoinHostPort(deviceHostname, "80"), "fuchsia.com")
	if err != nil {
		return nil, fmt.Errorf("error creating sl4f client: %w", err)
	}

	logger.Infof(ctx, "connected to sl4f in %s", time.Now().Sub(startTime))

	return rpcClient, nil
}

// Pave paves the device to the specified build. It assumes the device is
// already in recovery, since there are multiple ways to get a device into
// recovery. Does not reconnect to the device.
func (c *Client) Pave(
	ctx context.Context,
	build artifacts.Build,
	sshPublicKey ssh.PublicKey,
) error {
	p, err := build.GetPaver(ctx, sshPublicKey)
	if err != nil {
		return fmt.Errorf("failed to get paver to pave device: %w", err)
	}

	if err := c.RebootToRecovery(ctx); err != nil {
		return fmt.Errorf("failed to reboot to recovery during paving: %w", err)
	}

	// First, pave the build's zedboot onto the device.
	logger.Infof(ctx, "waiting for device to enter zedboot")
	resolver, err := c.getResolver(ctx)
	if err != nil {
		return fmt.Errorf("error getting resolver: %w", err)
	}

	listeningName, err := resolver.WaitToFindDeviceInNetboot(ctx)
	if err != nil {
		return fmt.Errorf("failed to wait for device to reboot into zedboot: %w", err)
	}

	if err = p.PaveWithOptions(ctx, listeningName, paver.Options{Mode: paver.ZedbootOnly}); err != nil {
		return fmt.Errorf("device failed to pave: %w", err)
	}

	// Next, pave the build onto the device.
	logger.Infof(ctx, "paved zedboot, waiting for the device to boot into zedboot")
	listeningName, err = resolver.WaitToFindDeviceInNetboot(ctx)
	if err != nil {
		return fmt.Errorf("failed to wait for device to reboot into zedboot: %w", err)
	}

	if err = p.PaveWithOptions(ctx, listeningName, paver.Options{Mode: paver.SkipZedboot}); err != nil {
		return fmt.Errorf("device failed to pave: %w", err)
	}

	logger.Infof(ctx, "paver completed, waiting for device to boot")

	return nil
}

// Flash the device to the specified build. Does not reconnect to the device.
func (c *Client) Flash(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	build artifacts.Build,
	publicKey ssh.PublicKey,
) error {
	flasher := ffxTool.Flasher()
	flasher.SetDiscoveryTimeout(12000)
	flasher.SetRetries(3)
	flasher.SetRetrySleep(c.flashRetrySleep)
	flasher.SetSSHPublicKey(publicKey)

	if productBundleDir, err := build.GetProductBundleDir(ctx); err == nil {
		logger.Infof(ctx, "Flashing with the product bundle %s", productBundleDir)
		flasher.SetProductBundle(productBundleDir)
	} else {
		logger.Warningf(ctx, "Failed to download the product bundle, trying to fall back to the flash manifest: %v", err)
		manifest, errMan := build.GetFlashManifest(ctx)
		if errMan != nil {
			return fmt.Errorf("failed to get flash manifest from build: %w", errMan)
		}
		logger.Infof(ctx, "Flashing with the flash manifest %s", manifest)
		flasher.SetManifest(manifest)
	}

	// Try to get the serial number while the device is still in Product state
	// because in strict mode ffx might not be able to resolve the nodename
	// when the device is in fastboot state later.
	var serialNumber string
	entries, err := ffxTool.TargetList(ctx, c.nodeName, 0)
	if err != nil {
		return fmt.Errorf("failed to list devices in Product state: %w", err)
	}
	if len(entries) > 0 {
		serialNumber = entries[0].Serial
		logger.Infof(ctx, "Found serial number %s in Product state", serialNumber)
	}

	logger.Infof(ctx, "rebooting device to bootloader before flashing in strict mode")
	if err := c.RebootToBootloader(ctx, ffxTool); err != nil {
		logger.Warningf(ctx, "failed to reboot to bootloader: %v. The device might already be in fastboot.", err)
	}
	logger.Infof(ctx, "waiting for device to enter fastboot")
	resolver, err := c.getResolver(ctx)
	if err != nil {
		return fmt.Errorf("error getting resolver: %w", err)
	}
	fastbootTarget, err := resolver.WaitToFindDeviceInFastboot(ctx, serialNumber)
	if err != nil {
		return fmt.Errorf("device failed to enter fastboot mode: %w", err)
	}

	// If the target is the nodename, try to find the serial number instead
	// because in strict mode ffx needs the serial number for fastboot devices.
	if fastbootTarget == c.nodeName {
		if serialNumber != "" {
			fastbootTarget = serialNumber
			logger.Infof(ctx, "Using serial number %s for fastboot target extracted in Product state", fastbootTarget)
		} else {
			entries, err := ffxTool.TargetList(ctx, "", 12*time.Second)
			if err != nil {
				return fmt.Errorf("failed to list devices in Fastboot state: %w", err)
			}
			for _, entry := range entries {
				if entry.NodeName == fastbootTarget && entry.TargetState == "Fastboot" {
					if entry.Serial != "" {
						fastbootTarget = entry.Serial
						logger.Infof(ctx, "Found serial number %s for fastboot target", fastbootTarget)
						break
					}
				}
			}
		}
	}

	if fastbootTarget != "" {
		logger.Infof(ctx, "found fastboot target: %s", fastbootTarget)
		flasher.SetTarget(fastbootTarget)
	}

	_, err = flasher.Flash(ctx)
	return err
}

// Forces an install of an update from an url, without requesting a reboot
func (c *Client) ForceInstall(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	target string,
	url string,
) error {
	return ffxTool.TargetUpdateForceInstallNoReboot(ctx, target, url)
}

// Monitors the update for the connected client
func (c *Client) MonitorUpdate(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	target string,
) (string, error) {
	stdout, err := ffxTool.TargetUpdateCheckNowMonitor(ctx, target)
	return string(stdout), err
}

// Set the update channel for the connected client
func (c *Client) SetUpdateChannel(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	target string,
	channel string,
) error {
	targetSpecifier, err := c.getTargetSpecifier(ctx, ffxTool)
	if err != nil {
		return fmt.Errorf("failed to get target specifier: %w", err)
	}
	ffxTool.SetTarget(targetSpecifier)
	return ffxTool.TargetUpdateChannelSet(ctx, targetSpecifier, channel)
}
