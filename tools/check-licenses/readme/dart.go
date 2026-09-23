// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package readme

import (
	"os"

	"gopkg.in/yaml.v2"
)

type pubspecYaml struct {
	Name        string `yaml:"name"`
	Version     string `yaml:"version"`
	Description string `yaml:"description"`
	Homepage    string `yaml:"homepage"`
	Repository  string `yaml:"repository"`
}

// ParsePubspecYaml reads a pubspec.yaml file and returns a synthetic Readme struct.
func ParsePubspecYaml(path string) ([]*Readme, error) {
	bytes, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}

	var pubspec pubspecYaml
	if err := yaml.Unmarshal(bytes, &pubspec); err != nil {
		return nil, err
	}

	url := pubspec.Repository
	if url == "" {
		url = pubspec.Homepage
	}
	if url == "" && pubspec.Name != "" {
		url = "https://pub.dev/packages/" + pubspec.Name
	}

	return []*Readme{
		{
			Name:     pubspec.Name,
			Version:  pubspec.Version,
			URL:      url,
			Location: ".",
		},
	}, nil
}
