// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package repo

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

type fakeTimeProvider struct {
	time int
}

func (f *fakeTimeProvider) UnixTimestamp() int {
	return f.time
}

var roleJsons = []string{"root.json", "timestamp.json", "targets.json", "snapshot.json"}

func TestInitRepoWithCreate_no_directory(t *testing.T) {
	repoDir := t.TempDir()
	// Remove the repository directory that tempdir just created, as we want to
	// check that a new one is created.
	if err := os.RemoveAll(repoDir); err != nil {
		t.Fatal(err)
	}
	r, err := New(repoDir, t.TempDir())
	if err != nil {
		t.Fatalf("Repo new returned error %v", err)
	}
	if err := r.OptionallyInitAtLocation(true); err != nil {
		t.Fatal(err)
	}

	for _, rolejson := range roleJsons {
		path := filepath.Join(repoDir, "keys", rolejson)
		if _, err := os.Stat(path); err != nil {
			t.Fatal(err)
		}
	}
}
func TestInitRepoWithCreate_existing_directory(t *testing.T) {
	repoDir := t.TempDir()
	// The tempdir is not removed, so the repo directory already exists but is
	// empty.
	r, err := New(repoDir, t.TempDir())
	if err != nil {
		t.Fatalf("Repo new returned error %v", err)
	}
	if err := r.OptionallyInitAtLocation(true); err != nil {
		t.Fatal(err)
	}

	for _, rolejson := range roleJsons {
		path := filepath.Join(repoDir, "keys", rolejson)
		if _, err := os.Stat(path); err != nil {
			t.Fatal(err)
		}
	}
}
func TestInitRepoWithCreate_file_at_target_path(t *testing.T) {
	repoPath := filepath.Join(t.TempDir(), "publish-test-repo")
	if err := os.WriteFile(repoPath, []byte("foo"), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, err := New(repoPath, t.TempDir()); err == nil {
		t.Fatal("expected error, did not get one")
	} else if !errors.Is(err, syscall.ENOTDIR) {
		t.Fatalf("unexpected error: %#v", err)
	}
}

func TestInitRepoNoCreate(t *testing.T) {
	repoDir := t.TempDir()
	r, err := New(repoDir, t.TempDir())
	if err != nil {
		t.Fatalf("Repo init returned error %v", err)
	}

	// With the false param, we _don't_ want to create this repository if
	// it doesn't already exist (which it doesn't, because there isn't a root.json).
	// Make sure we get the correct error.
	err = r.OptionallyInitAtLocation(false)
	if err == nil {
		// We actually want an error here.
		t.Fatal("repo did not exist but was possibly created")
	}

	if err != NotCreatingNonExistentRepoError {
		t.Fatalf("other init error: %v", err)
	}
}

func copyFile(src string, dest string) error {
	b, err := os.ReadFile(src)
	if err != nil {
		return fmt.Errorf("ReadFile: failed to read file %s, err: %w", src, err)
	}
	if err := os.WriteFile(dest, b, 0o600); err != nil {
		return fmt.Errorf("WriteFile: failed to write file %s, err: %w", dest, err)
	}
	return nil
}

func TestLoadExistingRepo(t *testing.T) {
	repoDir := t.TempDir()
	// Create a test repo.
	r, err := New(repoDir, t.TempDir())
	if err != nil {
		t.Fatalf("New: Repo init returned error: %v", err)
	}
	if err := r.Init(); err != nil {
		t.Fatalf("Init: failed to init repo: %v", err)
	}

	if err := r.AddTargets([]string{}, json.RawMessage{}); err != nil {
		t.Fatalf("AddTargets, failed to add empty target: %v", err)
	}

	const testVersion1 = 1
	timeProvider := fakeTimeProvider{testVersion1}
	r.timeProvider = &timeProvider
	if err = r.CommitUpdates(true); err != nil {
		t.Fatalf("CommitUpdates: failed to commit updates: %v", err)
	}

	newRepoDir := t.TempDir()
	if err := os.Mkdir(filepath.Join(newRepoDir, "repository"), 0o700); err != nil {
		t.Fatalf("Couldn't create test repo directory.")
	}
	if err := os.Mkdir(filepath.Join(newRepoDir, "keys"), 0o700); err != nil {
		t.Fatalf("Couldn't create test keys directory.")
	}
	defer os.RemoveAll(newRepoDir)
	// Copy the metadata and keys to a new test folder.
	for _, rolejson := range roleJsons {
		if err := copyFile(filepath.Join(repoDir, "repository", rolejson), filepath.Join(newRepoDir, "repository", rolejson)); err != nil {
			t.Fatalf("copyFile: failed to copy file: %v", err)
		}

		if err := copyFile(filepath.Join(repoDir, "keys", rolejson), filepath.Join(newRepoDir, "keys", rolejson)); err != nil {
			t.Fatalf("copyFile: failed to copy file: %v", err)
		}
	}

	if err := copyFile(filepath.Join(repoDir, "repository", "1.root.json"), filepath.Join(newRepoDir, "repository", "1.root.json")); err != nil {
		t.Fatalf("copyFile: failed to copy file: %v", err)
	}

	// Initiate a new repo using the folder containing existing metadata and keys.
	r, err = New(newRepoDir, t.TempDir())
	if err != nil {
		t.Fatalf("New: failed to init new repo using existing metadata files: %v", err)
	}
	if err := r.Init(); err != os.ErrExist {
		t.Fatalf("Init: expect to return os.ErrExist when the repo already exists, got %v", err)
	}
	if err := r.AddTargets([]string{}, json.RawMessage{}); err != nil {
		t.Fatalf("AddTargets, failed to add empty target: %v", err)
	}
	const testVersion2 = 2
	timeProvider = fakeTimeProvider{testVersion2}
	r.timeProvider = &timeProvider
	if err = r.CommitUpdates(true); err != nil {
		t.Fatalf("CommitUpdates: failed to commit updates: %v", err)
	}

	// Check for rolejsons and consistent snapshots:
	for _, rolejson := range roleJsons {
		bytes, err := os.ReadFile(filepath.Join(newRepoDir, "repository", rolejson))
		if err != nil {
			t.Fatal(err)
		}

		// Check metadata has a UTC timestamp, and no nanoseconds.
		var meta struct {
			Signed struct {
				Expires time.Time `json:"expires"`
			} `json:"signed"`
		}
		if err := json.Unmarshal(bytes, &meta); err != nil {
			t.Fatal(err)
		}
		zone, offset := meta.Signed.Expires.Zone()
		if zone != "UTC" || offset != 0 {
			t.Fatalf("%s expires field is not UTC: %s", rolejson, meta.Signed.Expires)
		}
		if meta.Signed.Expires.Nanosecond() != 0 {
			t.Fatalf("%s expires should not have nanoseconds: %s", rolejson, meta.Signed.Expires)
		}

		// timestamp doesn't get a consistent snapshot, as it is the entrypoint
		if rolejson == "timestamp.json" {
			continue
		}

		if rolejson == "root.json" {
			// Root version is 1 since we call GenKeys once.
			if _, err := os.Stat(filepath.Join(newRepoDir, "repository", "1.root.json")); err != nil {
				t.Fatal(err)
			}
		} else {
			if _, err := os.Stat(filepath.Join(newRepoDir, "repository", fmt.Sprintf("%d.%s", testVersion2, rolejson))); err != nil {
				t.Fatal(err)
			}
		}
	}
}
