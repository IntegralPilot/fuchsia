// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package build

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"runtime"
	"testing"
)

var hostDir = map[string]string{"arm64": "host_arm64", "amd64": "host_x64"}[runtime.GOARCH]

func TestCompiledVersionMatchesBuildVersion(t *testing.T) {
	expectedBytes, err := os.ReadFile(filepath.Join(hostDir, "test_data/version-history/go/version_history.json"))
	if err != nil {
		t.Fatal(err)
	}

	if !bytes.Equal(expectedBytes, versionHistoryBytes) {
		t.Fatalf("expected:\n%s\ngot:\n%s", expectedBytes, versionHistoryBytes)
	}
}

func TestParseHistoryWorks(t *testing.T) {
	b, err := json.Marshal(versionHistoryJson{
		SchemaId: versionHistorySchemaId,
		Data: versionHistoryDataJson{
			Name: versionHistoryName,
			Type: versionHistoryType,
			APILevels: map[string]apiLevel{
				"1": {ABIRevision: "1", Status: Supported},
				"2": {ABIRevision: "0x2", Status: InDevelopment},
			},
		},
	})
	if err != nil {
		t.Fatal(err)
	}

	actual, err := parseVersionHistory(b)
	if err != nil {
		t.Fatal(err)
	}

	expected := []Version{
		{APILevel: 1, ABIRevision: 1, Status: Supported},
		{APILevel: 2, ABIRevision: 2, Status: InDevelopment},
	}

	if !reflect.DeepEqual(expected, actual) {
		t.Fatalf("expected %+v, got %+v", expected, actual)
	}
}

func TestParseHistoryRejectsInvalidSchema(t *testing.T) {
	b, err := json.Marshal(&versionHistoryJson{
		SchemaId: "some-schema",
		Data: versionHistoryDataJson{
			Name:      versionHistoryName,
			Type:      versionHistoryType,
			APILevels: map[string]apiLevel{},
		},
	})
	if err != nil {
		t.Fatal(err)
	}

	vs, err := parseVersionHistory(b)
	if err == nil {
		t.Fatalf("expected error, got: %+v", vs)
	}
}

func TestParseHistoryRejectsInvalidName(t *testing.T) {
	b, err := json.Marshal(&versionHistoryJson{
		SchemaId: versionHistorySchemaId,
		Data: versionHistoryDataJson{
			Name:      "some-name",
			Type:      versionHistoryType,
			APILevels: map[string]apiLevel{},
		},
	})
	if err != nil {
		t.Fatal(err)
	}

	vs, err := parseVersionHistory(b)
	if err == nil {
		t.Fatalf("expected error, got: %+v", vs)
	}
}

func TestParseHistoryRejectsInvalidType(t *testing.T) {
	b, err := json.Marshal(&versionHistoryJson{
		SchemaId: versionHistorySchemaId,
		Data: versionHistoryDataJson{
			Name:      versionHistoryName,
			Type:      "some-type",
			APILevels: map[string]apiLevel{},
		},
	})
	if err != nil {
		t.Fatal(err)
	}

	vs, err := parseVersionHistory(b)
	if err == nil {
		t.Fatalf("expected error, got: %+v", vs)
	}
}

func TestParseHistoryRejectsInvalidVersions(t *testing.T) {
	for k, v := range map[string]apiLevel{
		"some-version": {ABIRevision: "1", Status: Unsupported},
		"-1":           {ABIRevision: "1", Status: Unsupported},
		"1":            {ABIRevision: "some-revision", Status: Supported},
		"2":            {ABIRevision: "-1", Status: Supported},
	} {
		b, err := json.Marshal(&versionHistoryJson{
			SchemaId: versionHistorySchemaId,
			Data: versionHistoryDataJson{
				Name:      versionHistoryName,
				Type:      versionHistoryType,
				APILevels: map[string]apiLevel{k: v},
			},
		})
		if err != nil {
			t.Fatal(err)
		}

		vs, err := parseVersionHistory(b)
		if err == nil {
			t.Fatalf("expected error, got: %+v", vs)
		}
	}
}

func fakeVersionHistory() *VersionHistory {
	return NewForTesting([]Version{
		{
			APILevel:    4,
			ABIRevision: 0xabcd0004,
			Status:      Unsupported,
		},
		{
			APILevel:    5,
			ABIRevision: 0xabcd0005,
			Status:      Supported,
		},
		{
			APILevel:    6,
			ABIRevision: 0xabcd0006,
			Status:      Supported,
		},
		{
			APILevel:    7,
			ABIRevision: 0xabcd0007,
			Status:      InDevelopment,
		},
	})
}

func TestCheckApiLevel(t *testing.T) {
	versions := fakeVersionHistory()

	tcs := []struct {
		apiLevel    uint64
		abiRevision uint64
		hasError    bool
	}{
		{apiLevel: 42, hasError: true},
		// This currently says API 4 is supported, but it shouldn't.
		//
		// TODO: https://fxbug.dev/326096347 - Uncomment this once it passes.
		// {apiLevel: 4, hasError: true},
		{apiLevel: 5, abiRevision: 0xabcd0005},
		{apiLevel: 6, abiRevision: 0xabcd0006},
		{apiLevel: 7, abiRevision: 0xabcd0007},
	}

	for _, tc := range tcs {
		abiRevision, err := versions.CheckApiLevelForBuild(tc.apiLevel)
		if abiRevision != tc.abiRevision {
			t.Errorf("CheckApiLevelForBuild(%d) = %x; want %x", tc.apiLevel, abiRevision, tc.abiRevision)
		}
		if err == nil && tc.hasError {
			t.Errorf("CheckApiLevelForBuild(%d) = %x; want error", tc.apiLevel, abiRevision)
		}
		if err != nil && !tc.hasError {
			t.Errorf("CheckApiLevelForBuild(%d) gives error %v", tc.apiLevel, err)
		}
	}
}

func TestGetExampleAbiRevision(t *testing.T) {
	versions := fakeVersionHistory()

	exampleAbi := versions.ExampleSupportedAbiRevisionForTests()
	if exampleAbi != 0xabcd0007 {
		t.Errorf("ExampleSupportedAbiRevisionForTests() = %x; want 0xabcd0007", exampleAbi)
	}
}
