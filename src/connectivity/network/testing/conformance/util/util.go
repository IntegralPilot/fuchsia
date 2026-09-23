// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package util

import (
	"os"
	"path/filepath"
)

func GetBuildDirectory() (string, error) {
	executablePath, err := os.Executable()
	if err != nil {
		return "", err
	}
	buildDir, err := filepath.Abs(filepath.Dir(filepath.Dir(executablePath)))
	if err != nil {
		return "", err
	}
	return buildDir, nil
}

func GetHostOutDirectory() (string, error) {
	executablePath, err := os.Executable()
	if err != nil {
		return "", err
	}
	dir, err := filepath.Abs(filepath.Dir(executablePath))
	if err != nil {
		return "", err
	}
	parent, filePart := filepath.Split(dir)
	if filePart == "host-tools" {
		// We're running at desk and actually need the host_x64 directory instead.
		return filepath.Join(parent, "host_x64"), nil
	}
	return dir, nil
}

func GetHostToolsDirectory() (string, error) {
	hostOut, err := GetHostOutDirectory()
	if err != nil {
		return "", err
	}

	parent, _ := filepath.Split(hostOut)
	return filepath.Join(parent, "host-tools"), nil
}
