// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.package main

package readerwriters

import (
	"compress/gzip"
	"fmt"
	"io"
	"os"
	"strings"
)

// FileOrGzipReader implements io.Reader + io.Closer and can read from either
// a regular file, or a gzip-compressed one if its file path ends with ".gz".
type FileOrGzipReader struct {
	file   *os.File
	reader io.Reader
}

// Create creates new FileOrGzipReader instance from a file path.
func Open(path string) (*FileOrGzipReader, error) {
	fi, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	if strings.HasSuffix(path, ".gz") {
		gzReader, err := gzip.NewReader(fi)
		if err != nil {
			return nil, err
		}
		return &FileOrGzipReader{
			file:   fi,
			reader: gzReader,
		}, nil
	} else {
		return &FileOrGzipReader{
			file:   fi,
			reader: fi,
		}, nil
	}
}

// Implement io.Reader.Read()
func (r *FileOrGzipReader) Read(p []byte) (int, error) {
	return r.reader.Read(p)
}

// Implement io.Closer.Close()
func (r *FileOrGzipReader) Close() error {
	if gzReader, ok := r.reader.(*gzip.Reader); ok {
		if err := gzReader.Close(); err != nil {
			if err2 := r.file.Close(); err2 != nil {
				return fmt.Errorf("error closing gzip reader: %w, and error closing file: %w", err, err2)
			}
			return err
		}
	}
	return r.file.Close()
}

// FileOrGzipWriter implements io.Writer and io.Close and can write to either
// a regular files, or a gzip-compressed one if its file path ends with ".gz".
type FileOrGzipWriter struct {
	file   *os.File
	writer io.Writer
}

// Create creates a new FileOrGzipWriter instance from a file path.
func Create(path string) (*FileOrGzipWriter, error) {
	fi, err := os.Create(path)
	if err != nil {
		return nil, err
	}
	if strings.HasSuffix(path, ".gz") {
		gzWriter := gzip.NewWriter(fi)
		return &FileOrGzipWriter{
			file:   fi,
			writer: gzWriter,
		}, nil
	} else {
		return &FileOrGzipWriter{
			file:   fi,
			writer: fi,
		}, nil
	}
}

func (w *FileOrGzipWriter) Write(p []byte) (int, error) {
	return w.writer.Write(p)
}

func (w *FileOrGzipWriter) Close() error {
	if gzWriter, ok := w.writer.(*gzip.Writer); ok {
		if err := gzWriter.Close(); err != nil {
			if err2 := w.file.Close(); err2 != nil {
				return fmt.Errorf("error closing gzip writer: %w, and error closing file: %w", err, err2)
			}
			return err
		}
	}
	return w.file.Close()
}
