// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package cli

import (
	"context"
	"flag"
	"fmt"
	"strings"

	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/artifacts"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/device"
	"go.fuchsia.dev/fuchsia/src/testing/host-target-testing/ffx"
)

type repeatableBuildKind int

const (
	builderNameKind repeatableBuildKind = iota
	buildIdKind
	fuchsiaBuildDirKind
	productBundleDirKind
)

type BuildConfig struct {
	archiveConfig    *ArchiveConfig
	deviceConfig     *DeviceConfig
	prefix           string
	builderName      string
	buildID          string
	fuchsiaBuildDir  string
	productBundleDir string
}

func NewBuildConfig(
	fs *flag.FlagSet,
	archiveConfig *ArchiveConfig,
	deviceConfig *DeviceConfig,
	defaultBuildID string,
) *BuildConfig {
	return NewBuildConfigWithPrefix(fs, archiveConfig, deviceConfig, defaultBuildID, "")
}

func NewBuildConfigWithPrefix(
	fs *flag.FlagSet,
	archiveConfig *ArchiveConfig,
	deviceConfig *DeviceConfig,
	defaultBuildID string,
	prefix string,
) *BuildConfig {
	c := &BuildConfig{
		prefix:        prefix,
		archiveConfig: archiveConfig,
		deviceConfig:  deviceConfig,
	}

	fs.StringVar(&c.builderName, fmt.Sprintf("%sbuilder-name", prefix), "", "Pave to the latest version of this builder")
	fs.StringVar(&c.buildID, fmt.Sprintf("%sbuild-id", prefix), defaultBuildID, "Pave to this specific build id")
	fs.StringVar(&c.fuchsiaBuildDir, fmt.Sprintf("%sfuchsia-build-dir", prefix), "", "Pave to the build in this fuchsia build output directory")
	fs.StringVar(&c.productBundleDir, fmt.Sprintf("%sproduct-bundle-dir", prefix), "", "Flash to the build in this product bundle directory")

	return c
}

func (c *BuildConfig) Validate() error {
	defined := 0
	for _, s := range []string{
		c.builderName,
		c.buildID,
		c.fuchsiaBuildDir,
		c.productBundleDir,
	} {
		if s != "" {
			defined += 1
		}
	}
	if defined > 1 {
		return fmt.Errorf("--%sbuilder-name, --%sbuild-id, --%sfuchsia-build-dir, and %sproduct-bundle-dir are mutually exclusive",
			c.prefix,
			c.prefix,
			c.prefix,
			c.prefix,
		)
	}

	return nil
}

func (c *BuildConfig) GetBuilder() (*artifacts.Builder, error) {
	if c.builderName == "" {
		return nil, fmt.Errorf("builder not specified")
	}

	return c.archiveConfig.BuildArchive().GetBuilder(c.builderName), nil
}

func (c *BuildConfig) getBuildID(ctx context.Context) (string, error) {
	if c.builderName != "" && c.buildID == "" {
		b, err := c.GetBuilder()
		if err != nil {
			return "", err
		}
		id, err := b.GetLatestBuildID(ctx)
		if err != nil {
			return "", fmt.Errorf("failed to lookup build id: %w", err)
		}
		c.buildID = id
	}

	return c.buildID, nil
}

func (c *BuildConfig) GetBuild(
	ctx context.Context,
	deviceClient *device.Client,
	outputDir string,
) (artifacts.Build, error) {
	buildID, err := c.getBuildID(ctx)
	if err != nil {
		return nil, err
	}

	var build artifacts.Build
	if buildID != "" {
		build, err = c.archiveConfig.BuildArchive().GetBuildByID(
			ctx,
			buildID,
			outputDir,
		)
	} else if c.fuchsiaBuildDir != "" {
		build, err = artifacts.NewFuchsiaDirBuild(c.fuchsiaBuildDir), nil
	} else if c.productBundleDir != "" {
		build, err = artifacts.NewProductBundleDirBuild(c.productBundleDir), nil
	}

	if err != nil {
		return nil, err
	}

	return build, nil
}

type RepeatableBuildConfig struct {
	archiveConfig  *ArchiveConfig
	deviceConfig   *DeviceConfig
	defaultBuildID string
	builds         []repeatableBuild
}

func NewRepeatableBuildConfig(
	fs *flag.FlagSet,
	archiveConfig *ArchiveConfig,
	deviceConfig *DeviceConfig,
	defaultBuildID string,
	prefix string,
) *RepeatableBuildConfig {
	c := &RepeatableBuildConfig{
		archiveConfig:  archiveConfig,
		deviceConfig:   deviceConfig,
		defaultBuildID: defaultBuildID,
	}

	fs.Var(
		repeatableBuildVar{c: c, kind: builderNameKind},
		fmt.Sprintf("%sbuilder-name", prefix),
		"Pave to the latest version of this builder",
	)
	fs.Var(
		repeatableBuildVar{c: c, kind: buildIdKind},
		fmt.Sprintf("%sbuild-id", prefix),
		"Pave to this specific build id",
	)
	fs.Var(
		repeatableBuildVar{c: c, kind: fuchsiaBuildDirKind},
		fmt.Sprintf("%sfuchsia-build-dir", prefix),
		"Pave to the build in this fuchsia build output directory",
	)
	fs.Var(
		repeatableBuildVar{c: c, kind: productBundleDirKind},
		fmt.Sprintf("%sproduct-bundle-dir", prefix),
		"Update to the latest version of this builder",
	)

	return c
}

func (c *RepeatableBuildConfig) Validate() error {
	if len(c.builds) == 0 && c.defaultBuildID == "" {
		return fmt.Errorf("no builds configured")
	}
	return nil
}

func (c *RepeatableBuildConfig) GetBuilds(
	ctx context.Context,
	deviceClient *device.Client,
	outputDir string,
) ([]artifacts.BuildWithVersion, error) {
	var builds []artifacts.BuildWithVersion
	for _, b := range c.builds {
		var build artifacts.Build
		var err error
		switch b.kind {
		case builderNameKind:
			builder := c.archiveConfig.BuildArchive().GetBuilder(b.value)
			buildID, err := builder.GetLatestBuildID(ctx)
			if err != nil {
				return nil, fmt.Errorf("failed to lookup build id: %w", err)
			}

			build, err = c.archiveConfig.BuildArchive().GetBuildByID(
				ctx,
				buildID,
				outputDir,
			)
			if err != nil {
				return nil, err
			}

		case buildIdKind:
			build, err = c.archiveConfig.BuildArchive().GetBuildByID(
				ctx,
				b.value,
				outputDir,
			)
			if err != nil {
				return nil, err
			}
		case fuchsiaBuildDirKind:
			build = artifacts.NewFuchsiaDirBuild(b.value)
		case productBundleDirKind:
			build = artifacts.NewProductBundleDirBuild(b.value)
		}
		builds = append(builds, artifacts.BuildWithVersion{Build: build, Version: ffx.FfxVersionPolicy(b.versionPolicy)})
	}

	// Append the last build id as our final upgrade.
	if c.defaultBuildID != "" {
		build, err := c.archiveConfig.BuildArchive().GetBuildByID(
			ctx,
			c.defaultBuildID,
			outputDir,
		)
		if err != nil {
			return nil, err
		}

		builds = append(builds, artifacts.BuildWithVersion{Build: build, Version: ffx.FfxVersionPolicyLatest})
	}

	return builds, nil
}

type repeatableBuild struct {
	kind          repeatableBuildKind
	value         string
	versionPolicy string
}

type repeatableBuildVar struct {
	c    *RepeatableBuildConfig
	kind repeatableBuildKind
}

func (v repeatableBuildVar) String() string {
	if v.c != nil {
		return fmt.Sprintf("%+v", v.c.builds)
	}
	return ""
}

func (v repeatableBuildVar) Set(s string) error {
	if s == "" {
		return fmt.Errorf("%s value cannot be empty", v.kind)
	}

	val := s
	versionPolicy := string(ffx.FfxVersionPolicyLatest)
	if v.kind == buildIdKind {
		// For build IDs, we allow the format "build_id:version_policy" to specify a specific
		// ffx version policy to use with this build.
		parts := strings.Split(s, ":")
		val = parts[0]
		if val == "" {
			return fmt.Errorf("build ID cannot be empty")
		}
		if len(parts) > 1 {
			versionPolicy = parts[1]
			if versionPolicy != string(ffx.FfxVersionPolicyLatest) && versionPolicy != string(ffx.FfxVersionPolicyFromApiLevel) {
				return fmt.Errorf("invalid ffx version policy %q, must be 'latest' or 'fromApiLevel'", versionPolicy)
			}
		}
	}

	v.c.builds = append(v.c.builds, repeatableBuild{kind: v.kind, value: val, versionPolicy: versionPolicy})
	return nil
}
