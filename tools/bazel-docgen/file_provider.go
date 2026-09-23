// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bazel_docgen

import (
	"archive/zip"
	"io"
	"log"
	"os"
	"path/filepath"
)

type FileProvider interface {
	Open()
	NewFile(name string) io.Writer
	Close()
}

type DirectoryFileProvider struct {
	dir string
}

func NewDirectoryFileProvider(outDir string) DirectoryFileProvider {
	return DirectoryFileProvider{
		outDir,
	}
}

func (fp *DirectoryFileProvider) Open() {
	// no-op
}

func (fp *DirectoryFileProvider) NewFile(name string) io.Writer {
	f, err := os.Create(filepath.Join(fp.dir, name))
	if err != nil {
		log.Fatalln("Failed to create new file:", err)
	}
	return f
}

func (fp *DirectoryFileProvider) Close() {
	// no-op
}

type ZipFileProvider struct {
	zipFileName string
	archive     *os.File
	zipWriter   *zip.Writer
}

func NewZipFileProvider(zipFile string) ZipFileProvider {
	return ZipFileProvider{
		zipFileName: zipFile,
	}
}

func (fp *ZipFileProvider) Open() {
	archive, err := os.Create(fp.zipFileName)
	if err != nil {
		log.Fatalln("Failed to create zip file:", err)
	}

	// We need to hold on to the underlying writer because Close does not
	// actually close it.
	fp.archive = archive
	fp.zipWriter = zip.NewWriter(archive)
}

func (fp *ZipFileProvider) NewFile(name string) io.Writer {
	f, err := fp.zipWriter.Create(name)
	if err != nil {
		log.Fatalln("Failed to create new file:", err)
	}
	return f
}

func (fp *ZipFileProvider) Close() {
	fp.zipWriter.Close()
	fp.archive.Close()
}
