// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package updater

import (
	"bufio"
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"regexp"
	"strings"
	"time"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/avb"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/omaha_tool"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/packages"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/util"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/zbi"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"

	"golang.org/x/crypto/ssh"
)

const (
	updateErrorSleepTime = 30 * time.Second

	// The default fuchsia update package path.
	defaultUpdatePackagePath = "update/0"
)

type client interface {
	ExpectReboot(
		ctx context.Context,
		ffxTool *ffx.FFXTool,
		f func() error,
	) error
	Reboot(
		ctx context.Context,
		ffxTool *ffx.FFXTool,
	) error
	RunReboot(ctx context.Context) error
	DisconnectionListener() <-chan struct{}
	ServePackageRepository(
		ctx context.Context,
		repo *packages.Repository,
		name string) (*packages.Server, error)
	RegisterPackageRepository(
		ctx context.Context,
		ffxTool *ffx.FFXTool,
		repo *packages.Server,
		repoName string,
		createRewriteRule bool,
		rewritePackages []string,
		sshAddr string) error
	Run(ctx context.Context, command []string, stdout io.Writer, stderr io.Writer) error
	SetUpdateChannel(ctx context.Context, ffxTool *ffx.FFXTool, target string, channel string) error
	MonitorUpdate(ctx context.Context, ffxTool *ffx.FFXTool, target string) (string, error)
	ForceInstall(ctx context.Context, ffxTool *ffx.FFXTool, target string, url string) error
}

type Updater interface {
	Update(
		ctx context.Context,
		ffxTool *ffx.FFXTool,
		c client,
		target string,
		updatePackage *packages.UpdatePackage,
	) error
}

func checkSyslogForUnknownFirmware(ctx context.Context, c client) error {
	logger.Infof(ctx, "Checking system log for errors")

	// Try to dump logs using the new logger.
	var stdout bytes.Buffer
	if err := c.Run(
		ctx,
		[]string{"log_listener", "--tag", "system-updater", "dump"},
		&stdout,
		os.Stderr,
	); err != nil {
		// Don't bother trying to fall back to the old command if we
		// disconnected from the device.
		var errExitMissing *ssh.ExitMissingError
		if errors.As(err, &errExitMissing) {
			return err
		}

		// Otherwise fall back to the old logger
		stdout = bytes.Buffer{}
		if err := c.Run(
			ctx,
			[]string{"log_listener", "--tag", "system-updater", "--dump_logs", "yes"},
			&stdout,
			os.Stderr,
		); err != nil {
			return err
		}
	}

	re := regexp.MustCompile("skipping unsupported .* type:")

	scanner := bufio.NewScanner(&stdout)
	for scanner.Scan() {
		line := scanner.Text()
		if re.MatchString(line) {
			return fmt.Errorf("System Updater should not have skipped installing firmware: %s", line)
		}
	}

	return nil
}

// SystemUpdateChecker uses `update check-now` to install a package.
type SystemUpdateChecker struct {
	updatePackage           *packages.UpdatePackage
	checkForUnknownFirmware bool
}

func NewSystemUpdateChecker(checkForUnknownFirmware bool) *SystemUpdateChecker {
	return &SystemUpdateChecker{
		checkForUnknownFirmware: checkForUnknownFirmware,
	}
}

func (u *SystemUpdateChecker) Update(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	c client,
	target string,
	srcUpdatePackage *packages.UpdatePackage,
) error {
	// If we're using the default update package url, we can directly update
	// with it.
	if srcUpdatePackage.Path() == defaultUpdatePackagePath {
		return updateCheckNow(
			ctx,
			ffxTool,
			c,
			target,
			srcUpdatePackage.Repository(),
			true,
			u.checkForUnknownFirmware,
		)
	}

	// Otherwise, copy the repository into a temporary directory, and publish
	// the update package to `fuchsia-pkg://fuchsia.com/update/0`, then update
	// with the temp repository.

	logger.Infof(
		ctx,
		"update package %s isn't default, cloning the repository so it can be made %s",
		srcUpdatePackage,
		defaultUpdatePackagePath,
	)

	tempDir, err := os.MkdirTemp("", "")
	if err != nil {
		return fmt.Errorf("failed to create a temp directory: %w", err)
	}
	defer os.RemoveAll(tempDir)

	tempRepo, err := srcUpdatePackage.Repository().CloneIntoDir(ctx, tempDir)
	if err != nil {
		return fmt.Errorf(
			"failed to copy %s into %s: %w",
			srcUpdatePackage.Repository(),
			tempDir,
			err,
		)
	}

	tempSrcUpdate, err := tempRepo.OpenUpdatePackage(ctx, srcUpdatePackage.Path())
	if err != nil {
		return fmt.Errorf("failed to open %s: %w", srcUpdatePackage, err)
	}

	_, err = tempSrcUpdate.EditContents(ctx, defaultUpdatePackagePath, func(tempDir string) error { return nil })
	if err != nil {
		return fmt.Errorf("failed to publish %s: %w", defaultUpdatePackagePath, err)
	}

	return updateCheckNow(
		ctx,
		ffxTool,
		c,
		target,
		tempRepo,
		true,
		u.checkForUnknownFirmware,
	)
}

func updateCheckNow(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	c client,
	target string,
	repo *packages.Repository,
	createRewriteRule bool,
	checkForUnknownFirmware bool,
) error {
	logger.Infof(ctx, "Triggering OTA")

	startTime := time.Now()
	err := c.ExpectReboot(ctx, ffxTool, func() error {
		// Since an update can trigger a reboot, we can run into all
		// sorts of races. The two main ones are:
		//
		//  * the network connection is torn down before we see the
		//    `update` command exited cleanly.
		//  * the system updater service was torn down before the
		//    `update` process, which would show up as the channel to
		//    be closed.
		//
		// In order to avoid this races, we need to:
		//
		//  * assume the ssh connection was closed means the OTA was
		//    probably installed and the device rebooted as normal.
		//  * `update` exiting with a error could be we just lost the
		//    shutdown race. So if we get an `update` error, wait a few
		//    seconds to see if the device disconnects. If so, treat it
		//    like the OTA was successful.

		// We pass createRewriteRule=true for versions of system-update-checker prior to
		// fxrev.dev/504000. Newer versions need to have `update channel set` called below.
		repoName := "trigger-ota"
		server, err := c.ServePackageRepository(ctx, repo, repoName)
		if err != nil {
			return fmt.Errorf("error setting up server: %w", err)
		}
		defer server.Shutdown(ctx)
		// Some initial attempts to register the repository may fail and hang
		// forever. It's not very clear the cause, but the ServePackageRepository
		// (packages.Server.newServer) uses a mux under the hood and it may take
		// some time for the mux to be properly initialized.
		// So the registration is retried up to 3 times with a short timeout for
		// each attempt.
		for i := 0; i < 3; i++ {
			childCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
			defer cancel()
			if err := c.RegisterPackageRepository(childCtx, ffxTool, server, repoName, createRewriteRule, nil, target); err == nil {
				break
			}
			if i == 2 {
				return fmt.Errorf("error registering repository with target: %w", err)
			}
		}

		ch := c.DisconnectionListener()

		{
			// Older versions of ffx may randomly stuck on setting the update channel.
			// The root cause is very unclear, but we cannot fix old ffx, so we just
			// work around the problem by falling back to /bin/update.
			childCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
			defer cancel()
			if err := c.SetUpdateChannel(childCtx, ffxTool, target, "trigger-ota"); err != nil {
				logger.Warningf(ctx, "update channel set via ffx failed: %v. The device may be running an old version of system-update-checker or incompatible RCS.", err)
				logger.Warningf(ctx, "retrying with /bin/update")
				cmd := []string{
					"/bin/update",
					"channel",
					"set",
					"trigger-ota",
				}
				if err := c.Run(ctx, cmd, os.Stdout, os.Stderr); err != nil {
					logger.Warningf(ctx, "update channel set failed: %v.", err)
				}
			}
		}

		{
			// Same to SetUpdateChannel, older versions of ffx may randomly stuck on
			// monitoring the update.
			childCtx, cancel := context.WithTimeout(ctx, 30*time.Second)
			defer cancel()
			stdout, err := c.MonitorUpdate(childCtx, ffxTool, target)
			if err != nil {
				logger.Warningf(ctx, "update monitoring via ffx failed: %v. Retrying via /bin/update.", err)
				cmd := []string{
					"/bin/update",
					"check-now",
					"--monitor",
				}
				var stdout_target bytes.Buffer
				err = c.Run(ctx, cmd, &stdout_target, os.Stderr)
				stdout = stdout_target.String()
				if err != nil {
					// tefmocheck checks for the string "remote command exited without
					// exit status or exit signal" and will mark the test as failed if
					// it sees it. However it's normal for us to get that error since
					// ssh might get disconnected before the update command completes.
					var errExitMissing *ssh.ExitMissingError
					if errors.As(err, &errExitMissing) {
						logger.Warningf(ctx, "update monitoring via /bin/update failed: ssh exited without status or signal")
					} else {
						logger.Warningf(ctx, "update monitoring via /bin/update failed: %v.", err)
					}
				}
			}
			logger.Debugf(ctx, "Output from check-now monitor: %s", stdout)

			if err == nil && checkForUnknownFirmware {
				// FIXME(https://fxbug.dev/42077484): We wouldn't have to ignore disconnects
				// if we could trigger an update without it automatically rebooting.
				err = checkSyslogForUnknownFirmware(ctx, c)
			}

			for _, line := range strings.Split(stdout, "\n") {
				if strings.Contains(line, "InstallationDeferredByPolicy") {
					logger.Debugf(ctx, "InstallationDeferredByPolicy state detected, forcing reboot")
					if err := c.RunReboot(ctx); err != nil {
						return fmt.Errorf("failed to reboot the device after InstallationDeferredByPolicy state: %w", err)
					}
					break
				}
			}
		}

		if err != nil {
			// If the device rebooted before ssh was able to tell
			// us the command ran, it will tell us the session
			// exited without passing along an exit code. So,
			// ignore that specific error.
			var errExitMissing *ssh.ExitMissingError
			if errors.As(err, &errExitMissing) {
				logger.Warningf(ctx, "disconnected, assuming this was because OTA triggered reboot")
				return nil
			}

			logger.Warningf(ctx, "update errored out, but maybe it lost the race, waiting a moment to see if the device reboots: %v", err)

			// We got an error, but maybe we lost the reboot race.
			// So wait a few moments to see if the device reboots
			// anyway.
			select {
			case <-ch:
				logger.Warningf(ctx, "disconnected, assuming this was because OTA triggered reboot")
				return nil
			case <-time.After(updateErrorSleepTime):
				return fmt.Errorf("failed to trigger OTA: %w", err)
			}
		}
		return nil
	})
	if err != nil {
		return err
	}

	cmd := []string{"/bin/update", "wait-for-commit"}
	if err := c.Run(ctx, cmd, os.Stdout, os.Stderr); err != nil {
		logger.Warningf(ctx, "update wait-for-commit after OTA attempt failed: %v", err)
	}

	logger.Infof(ctx, "OTA completed in %s", time.Now().Sub(startTime))

	return nil
}

// SystemUpdater uses the `system-updater` to install a package.
type SystemUpdater struct {
	checkForUnknownFirmware bool
}

func NewSystemUpdater(checkForUnknownFirmware bool) *SystemUpdater {
	return &SystemUpdater{
		checkForUnknownFirmware: checkForUnknownFirmware,
	}
}

func (u *SystemUpdater) Update(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	c client,
	target string,
	srcUpdate *packages.UpdatePackage,
) error {
	startTime := time.Now()

	repoName := "download-ota"
	dstUpdate, err := srcUpdate.RehostUpdatePackage(
		ctx,
		repoName,
		util.AddSuffixToPackageName(srcUpdate.Path(), "system-updater"),
	)
	if err != nil {
		return fmt.Errorf("error rehosting the update package: %w", err)
	}

	server, err := c.ServePackageRepository(ctx, dstUpdate.Repository(), repoName)
	if err != nil {
		return fmt.Errorf("error setting up server: %w", err)
	}
	defer server.Shutdown(ctx)
	if err := c.RegisterPackageRepository(ctx, ffxTool, server, repoName, true, nil, target); err != nil {
		return fmt.Errorf("error registering repository with target: %w", err)
	}

	updatePackageUrl := fmt.Sprintf("fuchsia-pkg://%s/%s", repoName, dstUpdate.Path())
	logger.Infof(ctx, "Downloading OTA %q", updatePackageUrl)

	if err := c.ForceInstall(ctx, ffxTool, target, fmt.Sprintf("%q", updatePackageUrl)); err != nil {
		logger.Errorf(ctx, "failed to run system updater via ffx: %w, retrying via /bin/update", err)
		cmd := []string{
			"/bin/update",
			"force-install",
			"--reboot", "false",
			fmt.Sprintf("%q", updatePackageUrl),
		}
		if err := c.Run(ctx, cmd, os.Stdout, os.Stderr); err != nil {
			return fmt.Errorf("failed to run system updater via /bin/update as well: %w", err)
		}
	}

	logger.Infof(ctx, "OTA successfully downloaded in %s", time.Now().Sub(startTime))

	if err := checkSyslogForUnknownFirmware(ctx, c); err != nil {
		return err
	}

	logger.Infof(ctx, "Rebooting device")
	startTime = time.Now()

	if err = c.Reboot(ctx, ffxTool); err != nil {
		return fmt.Errorf("device failed to reboot after OTA applied: %w", err)
	}

	logger.Infof(ctx, "Reboot complete in %s", time.Now().Sub(startTime))

	startTime = time.Now()
	cmd := []string{"/bin/update", "wait-for-commit"}
	if err := c.Run(ctx, cmd, os.Stdout, os.Stderr); err != nil {
		logger.Warningf(ctx, "update wait-for-commit failed: %v", err)
	}
	logger.Infof(ctx, "Commit successful in %s", time.Now().Sub(startTime))

	return nil
}

type OmahaUpdater struct {
	omahaTool                   *omaha_tool.OmahaTool
	avbTool                     *avb.AVBTool
	zbiTool                     *zbi.ZBITool
	workaroundOtaNoRewriteRules bool
	checkForUnknownFirmware     bool
}

func NewOmahaUpdater(
	omahaTool *omaha_tool.OmahaTool,
	avbTool *avb.AVBTool,
	zbiTool *zbi.ZBITool,
	workaroundOtaNoRewriteRules bool,
	checkForUnknownFirmware bool,
) *OmahaUpdater {
	return &OmahaUpdater{
		omahaTool:                   omahaTool,
		avbTool:                     avbTool,
		zbiTool:                     zbiTool,
		workaroundOtaNoRewriteRules: workaroundOtaNoRewriteRules,
		checkForUnknownFirmware:     checkForUnknownFirmware,
	}
}

func (u *OmahaUpdater) Update(
	ctx context.Context,
	ffxTool *ffx.FFXTool,
	c client,
	target string,
	srcUpdate *packages.UpdatePackage,
) error {
	logger.Infof(ctx, "injecting omaha_url into %q", srcUpdate)

	// Create a ZBI with the omaha_url argument.
	destZbi, err := os.CreateTemp("", "omaha_argument.zbi")
	if err != nil {
		return fmt.Errorf("failed to create temp file: %w", err)
	}
	defer os.Remove(destZbi.Name())

	imageArguments := map[string]string{
		"omaha_url":    u.omahaTool.URL(),
		"omaha_app_id": u.omahaTool.Args.AppId,
		"ota_channel":  "ota-test-channel",
	}

	logger.Infof(ctx, "Omaha Server URL set in vbmeta to %q", u.omahaTool.URL())

	if err := u.zbiTool.MakeImageArgsZbi(ctx, destZbi.Name(), imageArguments); err != nil {
		return fmt.Errorf("failed to create ZBI: %w", err)
	}

	// Create a vbmeta that includes the ZBI we just created.
	propFiles := map[string]string{
		"zbi": destZbi.Name(),
	}

	repoName := "trigger-ota"

	dstUpdate, err := srcUpdate.EditUpdatePackageWithVBMetaProperties(
		ctx,
		u.avbTool,
		repoName,
		util.AddSuffixToPackageName(srcUpdate.Path(), "omaha-client"),
		propFiles,
	)
	if err != nil {
		return fmt.Errorf("failed to inject vbmeta properties into update package: %w", err)
	}

	omahaPackageURL := fmt.Sprintf(
		"fuchsia-pkg://%s/%s?hash=%s",
		repoName,
		dstUpdate.Path(),
		dstUpdate.Merkle(),
	)

	logger.Infof(ctx, "Update Package URL: %q", omahaPackageURL)

	// Configure the Omaha server with the new omaha package URL.
	if err := u.omahaTool.SetPkgURL(ctx, omahaPackageURL); err != nil {
		return fmt.Errorf("Failed to set Omaha update package: %w", err)
	}

	// Trigger an update
	return updateCheckNow(
		ctx,
		ffxTool,
		c,
		target,
		dstUpdate.Repository(),
		!u.workaroundOtaNoRewriteRules,
		u.checkForUnknownFirmware,
	)
}
