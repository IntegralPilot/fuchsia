// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package repo

import (
	"crypto/sha512"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"syscall"
	"time"

	tuf "github.com/theupdateframework/go-tuf"
)

var roles = []string{"timestamp", "targets", "snapshot"}

// TimeProvider provides the service to get Unix timestamp.
type TimeProvider interface {
	// UnixTimestamp returns the Unix timestamp.
	UnixTimestamp() int
}

type Repo struct {
	*tuf.Repo
	path          string
	blobsDir      string
	encryptionKey []byte
	timeProvider  TimeProvider
}

var NotCreatingNonExistentRepoError = errors.New("repo does not exist and createIfNotExist is false, so not creating one")

// SystemProvider uses the time pkg to get Unix timestamp.
type SystemTimeProvider struct{}

func (*SystemTimeProvider) UnixTimestamp() int {
	return int(time.Now().Unix())
}

func passphrase(role string, confirm bool) ([]byte, error) { return []byte{}, nil }

// New initializes a new Repo structure that may read/write repository data at
// the given path.
func New(path, blobsDir string) (*Repo, error) {
	info, err := os.Stat(path)
	if err != nil {
		if os.IsNotExist(err) {
			err = os.MkdirAll(path, 0755)
		}
		if err != nil {
			return nil, fmt.Errorf("repository directory %q: %w", path, err)
		}
	}
	if info != nil && !info.IsDir() {
		return nil, fmt.Errorf("repository path %q: %w", path, syscall.ENOTDIR)
	}

	repo, err := tuf.NewRepo(tuf.FileSystemStore(path, passphrase), "sha512")
	if err != nil {
		return nil, err
	}
	r := &Repo{repo, path, blobsDir, nil, &SystemTimeProvider{}}

	if err := os.MkdirAll(blobsDir, os.ModePerm); err != nil {
		return nil, err
	}
	if err := os.MkdirAll(r.stagedFilesPath(), os.ModePerm); err != nil {
		return nil, err
	}

	return r, nil
}

// Init initializes a repository, preparing it for publishing. If a
// repository already exists, either os.ErrExist, or a TUF error are returned.
// If a repository does not exist at the given location, a repo will be created there.
func (r *Repo) Init() error {
	return r.OptionallyInitAtLocation(true)
}

// OptionallyInitAtLocation initializes a new repository, preparing it
// for publishing, if a repository does not already exist at its
// location and createIfNotExists is true.
// If a repository already exists, either os.ErrExist, or a TUF error are returned.
func (r *Repo) OptionallyInitAtLocation(createIfNotExists bool) error {
	if _, err := os.Stat(filepath.Join(r.path, "repository", "root.json")); err == nil {
		return os.ErrExist
	}

	// The repo doesn't exist at this location, but the caller may
	// want to treat this as an error.  The common case here is
	// that a command line user makes a typo and doesn't
	// understand why a new repo was silently created.
	if !createIfNotExists {
		return NotCreatingNonExistentRepoError
	}

	// Fuchsia repositories always use consistent snapshots.
	if err := r.Repo.Init(true); err != nil {
		return err
	}

	rk, err := r.Repo.RootKeys()
	if err != nil || len(rk) == 0 {
		err = r.GenKeys()
	}

	return err
}

// GenKeys will generate a full suite of the necessary keys for signing a
// repository.
func (r *Repo) GenKeys() error {
	if _, err := r.GenKey("root"); err != nil {
		return err
	}
	for _, role := range roles {
		if _, err := r.GenKey(role); err != nil {
			return err
		}
	}
	return nil
}

// CommitUpdates finalizes the changes to the update repository that have been
// staged by calling AddPackageFile. Setting dateVersioning to true will set
// the version of the targets, snapshot, and timestamp metadata files based on
// an offset in seconds from epoch (1970-01-01 00:00:00 UTC).
func (r *Repo) CommitUpdates(dateVersioning bool) error {
	if dateVersioning {
		dTime := r.timeProvider.UnixTimestamp()
		tVer, err := r.TargetsVersion()
		if err != nil {
			return err
		}
		if dTime > tVer {
			r.SetTargetsVersion(dTime)
		}
		sVer, err := r.SnapshotVersion()
		if err != nil {
			return err
		}
		if dTime > sVer {
			r.SetSnapshotVersion(dTime)
		}
		tsVer, err := r.TimestampVersion()
		if err != nil {
			return err
		}
		if dTime > tsVer {
			r.SetTimestampVersion(dTime)
		}
	}
	return r.commitUpdates()
}

func (r *Repo) commitUpdates() error {
	// TUF-1.0 section 4.4.2 states that the expiration must be in the
	// ISO-8601 format in the UTC timezone with no nanoseconds.
	expires := time.Now().AddDate(0, 0, 30).UTC().Round(time.Second)
	if err := r.SnapshotWithExpires(expires); err != nil {
		return fmt.Errorf("snapshot: %s", err)
	}
	if err := r.TimestampWithExpires(expires); err != nil {
		return fmt.Errorf("timestamp: %s", err)
	}
	if err := r.Commit(); err != nil {
		return fmt.Errorf("commit: %s", err)
	}

	return r.fixupRootConsistentSnapshot()
}

func (r *Repo) stagedFilesPath() string {
	return filepath.Join(r.path, "staged", "targets")
}

// when the repository is "pre-initialized" by a root.json from the build, but
// no root keys are available to the publishing step, the commit process does
// not produce a consistent snapshot file for the root json manifest. This
// method implements that production.
func (r *Repo) fixupRootConsistentSnapshot() error {
	b, err := os.ReadFile(filepath.Join(r.path, "repository", "root.json"))
	if err != nil {
		return err
	}
	sum512 := sha512.Sum512(b)
	rootSnap := filepath.Join(r.path, "repository", fmt.Sprintf("%x.root.json", sum512))
	if _, err := os.Stat(rootSnap); os.IsNotExist(err) {
		if err := os.WriteFile(rootSnap, b, 0666); err != nil {
			return err
		}
	}
	return nil
}
