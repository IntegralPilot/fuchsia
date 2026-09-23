// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

import (
	_ "embed"
	"encoding/json"
	"fmt"
	"sort"
	"strconv"
)

//go:embed version_history.json
var versionHistoryBytes []byte
var history VersionHistory

const versionHistorySchemaId string = "https://fuchsia.dev/schema/version_history.json"
const versionHistoryName string = "Platform version map"
const versionHistoryType string = "version_history"

type Status string

const (
	InDevelopment Status = "in-development"
	Supported     Status = "supported"
	Unsupported   Status = "unsupported"
)

type Version struct {
	APILevel    uint64
	ABIRevision uint64
	Status      Status
}

type versionHistoryJson struct {
	SchemaId string                 `json:"schema_id"`
	Data     versionHistoryDataJson `json:"data"`
}

type versionHistoryDataJson struct {
	Name      string              `json:"name"`
	Type      string              `json:"type"`
	APILevels map[string]apiLevel `json:"api_levels"`
}

type apiLevel struct {
	ABIRevision string `json:"abi_revision"`
	Status      Status `json:"status"`
}

func parseVersionHistory(b []byte) ([]Version, error) {
	var vh versionHistoryJson

	// Load external JSON of SDK version history
	if err := json.Unmarshal(b, &vh); err != nil {
		return []Version{}, err
	}

	if vh.SchemaId != versionHistorySchemaId {
		return []Version{}, fmt.Errorf("expected schema id %q, not %q", versionHistorySchemaId, vh.SchemaId)
	}

	if vh.Data.Name != versionHistoryName {
		return []Version{}, fmt.Errorf("expected name \"version_history\", not %q", vh.Data.Name)
	}

	if vh.Data.Type != versionHistoryType {
		return []Version{}, fmt.Errorf("expected type \"version_history\", not %q", vh.Data.Type)
	}

	vs := []Version{}
	for k, v := range vh.Data.APILevels {
		apiLevel, err := strconv.ParseUint(k, 10, 64)
		if err != nil {
			return []Version{}, fmt.Errorf("failed to parse API level as an integer: %w", err)
		}

		abiRevision, err := strconv.ParseUint(v.ABIRevision, 0, 64)
		if err != nil {
			return []Version{}, fmt.Errorf("failed to parse ABI revision as an integer: %w", err)
		}

		vs = append(vs, Version{
			APILevel:    apiLevel,
			ABIRevision: uint64(abiRevision),
			Status:      v.Status,
		})
	}

	sort.Slice(vs, func(i, j int) bool { return vs[i].APILevel < vs[j].APILevel })

	return vs, nil
}

// VersionHistory stores the history of Fuchsia API levels, and lets callers
// query the support status of API levels and ABI revisions.
type VersionHistory struct {
	versions []Version
}

// NewForTesting constructs a new VersionHistory instance, so you can have your
// own hermetic "alternate history" that doesn't change over time. Outside of
// tests, callers should use the History() top-level function in this package.
//
// If you're not testing versioning in particular, and you just want an API
// level/ABI revision that works, see ExampleSupportedAbiRevisionForTests.
func NewForTesting(versions []Version) *VersionHistory {
	return &VersionHistory{versions: versions}
}

// ExampleSupportedAbiRevisionForTests returns an ABI revision to be used in
// tests that create components on the fly and need to specify a supported ABI
// revision, but don't particularly care which. The returned ABI revision will
// be consistent within a given build, but may change from build to build.
func (vh *VersionHistory) ExampleSupportedAbiRevisionForTests() uint64 {
	return vh.versions[len(vh.versions)-1].ABIRevision
}

// CheckApiLevelForBuild checks whether the platform supports building
// components that target the given API level, and if so, returns the ABI
// revision associated with that API level.
//
// TODO: https://fxbug.dev/326096347 - This doesn't actually check that the API
// level is supported, merely that it exists. At time of writing, this is the
// behavior implemented by `pm`, so I'm keeping it consistent in the interest of
// no-op refactoring.
func (vh *VersionHistory) CheckApiLevelForBuild(apiLevel uint64) (abiRevision uint64, err error) {
	v := vh.versionFromApiLevel(apiLevel)
	if v == nil {
		return 0, fmt.Errorf(
			"Unknown target API level: %d. The SDK may be too old to support it?",
			apiLevel)
	}

	return v.ABIRevision, nil
}

func (vh *VersionHistory) versionFromApiLevel(apiLevel uint64) *Version {
	for _, v := range vh.versions {
		if v.APILevel == apiLevel {
			return &v
		}
	}
	return nil
}

func init() {
	v, err := parseVersionHistory(versionHistoryBytes)
	if err != nil {
		panic(fmt.Sprintf("failed to parse version_history.json: %s", err))
	}
	history.versions = v
}

// History returns the global VersionHistory instance generated at build-time
// from the contents of `//sdk/version_history.json`.
func History() *VersionHistory {
	return &history
}
