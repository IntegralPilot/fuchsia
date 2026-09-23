// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package repo

import (
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"math"
	"net/http"
	"regexp"
	"strings"

	tuf_data "github.com/theupdateframework/go-tuf/data"

	"go.fuchsia.dev/fuchsia/src/sys/pkg/lib/repo"
)

var (
	OutOfRangeError = errors.New("out of range error")
)

// ConfigServer handles serving the config file expected for a package repo.
type ConfigServer struct {
	rootKeyFetcher func() []byte
	storageType    repo.RepositoryStorageType
}

func NewConfigServer(rootKeyFetcher func() []byte, persist bool) *ConfigServer {
	cfg := &ConfigServer{rootKeyFetcher: rootKeyFetcher, storageType: repo.Unset}
	if persist {
		cfg.storageType = repo.Persistent
	}
	return cfg
}

func (c *ConfigServer) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	var scheme = "http://"
	if r.TLS != nil {
		scheme = "https://"
	}

	repoUrl := fmt.Sprintf("%s%s", scheme, r.Host)

	cfg, err := c.parseConfig(repoUrl)
	if err != nil {
		log.Printf("%s", err)
		w.WriteHeader(http.StatusInternalServerError)
		return
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(cfg)
}

func (c *ConfigServer) parseConfig(repoUrl string) (repo.Config, error) {
	cfg := repo.Config{
		URL: buildRepoUrl(repoUrl),
		Mirrors: []repo.MirrorConfig{
			{
				URL:       repoUrl,
				Subscribe: true,
			},
		},
		StorageType: c.storageType,
	}

	root, err := func() (tuf_data.Root, error) {
		var signed tuf_data.Signed
		var root tuf_data.Root
		if err := json.Unmarshal(c.rootKeyFetcher(), &signed); err != nil {
			return root, err
		}
		if err := json.Unmarshal(signed.Signed, &root); err != nil {
			return root, err
		}
		return root, nil
	}()
	if err != nil {
		return repo.Config{}, fmt.Errorf("root.json parsing error: %w", err)
	}

	cfg.RootKeys, err = repo.GetRootKeys(&root)
	if err != nil {
		return repo.Config{}, fmt.Errorf("could not get root keys from root.json: %w", err)
	}
	cfg.RootVersion, err = intToUint32(root.Version)
	if err != nil {
		return repo.Config{}, fmt.Errorf("error parsing root version: %w", err)
	}

	if rootRole, ok := root.Roles["root"]; ok {
		cfg.RootThreshold, err = intToUint32(rootRole.Threshold)
		if err != nil {
			return repo.Config{}, fmt.Errorf("error parsing root threshold: %w", err)
		}
	}

	return cfg, nil
}

// invalidHostnameCharsPattern contains all characters not allowed by the spec in
// https://fuchsia.dev/fuchsia-src/concepts/packages/package_url
var invalidHostnameCharsPattern = regexp.MustCompile("[^a-z0-9-.]")

// buildRepoUrl ensures the repoUrl string complies with the hostname spec
// in https://fuchsia.dev/fuchsia-src/concepts/packages/package_url
func buildRepoUrl(repoUrl string) string {
	ps := strings.Index(repoUrl, "://")
	if ps >= 0 && len(repoUrl) > ps+3 {
		repoUrl = repoUrl[ps+3:]
	}
	repoUrl = invalidHostnameCharsPattern.ReplaceAllString(strings.ToLower(repoUrl), "-")
	return "fuchsia-pkg://" + repoUrl
}

func intToUint32(x int) (uint32, error) {
	if x < 0 || x > math.MaxUint32 {
		return 0, OutOfRangeError
	}

	return uint32(x), nil
}
