// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package bundler

import (
	"context"
	"io"
	"time"

	"cloud.google.com/go/storage"

	"go.fuchsia.dev/fuchsia/tools/lib/logger"
	"google.golang.org/api/iterator"
)

// DataSink is an abstract data sink, providing a mockable interface to
// cloudSink, the GCS-backed implementation below.
type DataSink interface {
	ReadFromGCS(ctx context.Context, object string) ([]byte, error)
	GetBucketName() string
	DoesPathExist(ctx context.Context, prefix string) (bool, error)
}

// CloudSink is a GCS-backed data sink.
type cloudSink struct {
	client     *storage.Client
	bucket     *storage.BucketHandle
	bucketName string
}

func NewCloudSink(ctx context.Context, bucket string) (*cloudSink, error) {
	return newCloudSink(ctx, bucket)
}

func newCloudSink(ctx context.Context, bucket string) (*cloudSink, error) {
	client, err := storage.NewClient(ctx)
	if err != nil {
		return nil, err
	}
	return &cloudSink{
		client:     client,
		bucket:     client.Bucket(bucket),
		bucketName: bucket,
	}, nil
}

func (s *cloudSink) Close() {
	s.client.Close()
}

// readFromGCS reads an object from GCS.
func (s *cloudSink) ReadFromGCS(ctx context.Context, object string) ([]byte, error) {
	logger.Debugf(ctx, "reading %s from GCS", object)
	ctx, cancel := context.WithTimeout(ctx, time.Second*50)
	defer cancel()
	rc, err := s.bucket.Object(object).NewReader(ctx)
	if err != nil {
		return nil, err
	}
	defer rc.Close()

	data, err := io.ReadAll(rc)
	if err != nil {
		return nil, err
	}
	return data, nil
}

func (s *cloudSink) GetBucketName() string {
	return s.bucketName
}

// doesPathExist checks if a path exists in GCS.
func (s *cloudSink) DoesPathExist(ctx context.Context, prefix string) (bool, error) {
	logger.Debugf(ctx, "checking if %s is a valid path in GCS", prefix)
	it := s.bucket.Objects(ctx, &storage.Query{
		Prefix:    prefix,
		Delimiter: "/",
	})
	_, err := it.Next()
	// If the first object in the iterator is the end of the iterator, the path
	// is invalid and doesn't exist in GCS.
	if err == iterator.Done {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return true, nil
}
