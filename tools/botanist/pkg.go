// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

package botanist

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strconv"
	"time"

	"go.fuchsia.dev/fuchsia/tools/botanist/constants"
	"go.fuchsia.dev/fuchsia/tools/botanist/repo"
	"go.fuchsia.dev/fuchsia/tools/lib/logger"
)

const (
	repoID               = "fuchsia-pkg://fuchsia.com"
	localhostPlaceholder = "localhost"
	DefaultPkgSrvPort    = 8083
)

// cachedPkgRepo is a custom HTTP handler that acts as a GCS redirector with a
// local filesystem cache.
type cachedPkgRepo struct {
	loggerCtx  context.Context
	fileServer http.Handler
	repoPath   string

	totalBytesServed    int
	serveTimeSec        float64
	totalBytesFetched   int
	gcsFetchTimeSec     float64
	totalRequestsServed int
}

func newCachedPkgRepo(ctx context.Context, repoPath string) (*cachedPkgRepo, error) {
	return &cachedPkgRepo{
		loggerCtx:  ctx,
		fileServer: http.FileServer(http.Dir(repoPath)),
		repoPath:   repoPath,
	}, nil
}

func (c *cachedPkgRepo) logf(msg string, args ...any) {
	logger.Debugf(c.loggerCtx, fmt.Sprintf("[package server] %s", msg), args...)
}

func (c *cachedPkgRepo) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	startTime := time.Now()

	c.fileServer.ServeHTTP(w, r)

	// Update the serving metrics.
	if length := w.Header().Get("Content-Length"); length != "" {
		l, err := strconv.Atoi(length)
		if err != nil {
			c.logf("failed to convert content length %s to integer: %s", length, err)
		} else {
			c.totalBytesServed += l
			c.totalRequestsServed += 1
		}
	}
	c.serveTimeSec += time.Since(startTime).Seconds()
}

// PackageServer is the HTTP server that serves packages.
type PackageServer struct {
	loggerCtx context.Context
	srv       *http.Server
	c         *cachedPkgRepo
	RepoURL   string
	BlobURL   string
}

func (p *PackageServer) Close() error {
	logger.Debugf(p.loggerCtx, "stopping package server")
	// Calculate the bandwidth of package serving when the package server has
	// to fetch blobs from GCS.
	gcsFetchBandwidth := float64(p.c.totalBytesFetched) / p.c.gcsFetchTimeSec
	// Calculate the bandwidth of package serving when the package server is
	// serving blobs from disk.
	localServingBandwitdh := float64(p.c.totalBytesServed-p.c.totalBytesFetched) / (p.c.serveTimeSec - p.c.gcsFetchTimeSec)
	logger.Debugf(p.loggerCtx, "----------------------------------------------------")
	logger.Debugf(p.loggerCtx, "Package server data")
	logger.Debugf(p.loggerCtx, "----------------------------------------------------")
	logger.Debugf(p.loggerCtx, "Total requests served: %d", p.c.totalRequestsServed)
	logger.Debugf(p.loggerCtx, "Total data served (bytes): %d", p.c.totalBytesServed)
	logger.Debugf(p.loggerCtx, "Total time spent serving (seconds): %f", p.c.serveTimeSec)
	logger.Debugf(p.loggerCtx, "Bandwidth using local package serving (bytes per second): %f", localServingBandwitdh)
	logger.Debugf(p.loggerCtx, "Total data fetched from GCS (bytes): %d", p.c.totalBytesFetched)
	logger.Debugf(p.loggerCtx, "Total time spent downloading from GCS (seconds): %f", p.c.gcsFetchTimeSec)
	logger.Debugf(p.loggerCtx, "Bandwidth using GCS downloads (bytes per second): %f", gcsFetchBandwidth)
	logger.Debugf(p.loggerCtx, "----------------------------------------------------")
	return p.srv.Close()
}

// NewPackageServer creates and starts a local package server.
func NewPackageServer(ctx context.Context, repoPath string, port int) (*PackageServer, error) {
	logger.Debugf(ctx, "creating package server serving from %s", repoPath)

	// Create HTTP handlers for the package server.
	rootJsonBytes, err := os.ReadFile(filepath.Join(repoPath, "repository", "root.json"))
	if err != nil {
		return nil, err
	}
	cs := repo.NewConfigServer(func() []byte {
		return rootJsonBytes
	}, false)
	cPkgRepo, err := newCachedPkgRepo(ctx, repoPath)
	if err != nil {
		return nil, err
	}

	// Register the handlers and create the server.
	mux := http.NewServeMux()
	mux.Handle("/config.json", cs)
	mux.Handle("/", cPkgRepo)
	pkgSrv := &http.Server{
		Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			lw := &loggingWriter{w, 0, 0}
			mux.ServeHTTP(lw, r)
			logger.Debugf(ctx, "Served %s \"%s %s %s\" %d %d",
				r.RemoteAddr,
				r.Method,
				r.RequestURI,
				r.Proto,
				lw.Status,
				lw.ResponseSize)
		}),
	}

	// Start the server and spin off a handler to stop it when the context
	// is canceled.
	pkgSrvStarted := make(chan struct{})
	go func() {
		addr := fmt.Sprintf("0.0.0.0:%d", port)
		logger.Debugf(ctx, "[package server] starting on %s", addr)
		l, err := net.Listen("tcp", addr)
		if err != nil {
			logger.Errorf(ctx, "%s: listening on %s failed: %s", constants.FailedToServeMsg, addr, err)
		}
		close(pkgSrvStarted)
		if err := pkgSrv.Serve(l); err != nil && !errors.Is(err, http.ErrServerClosed) {
			logger.Errorf(ctx, "%s: %s", constants.FailedToServeMsg, err)
		}
	}()

	// Do not return until the package server has actually started serving.
	<-pkgSrvStarted
	logger.Debugf(ctx, "package server started")
	return &PackageServer{
		loggerCtx: ctx,
		srv:       pkgSrv,
		c:         cPkgRepo,
		RepoURL:   fmt.Sprintf("http://%s:%d/repository", localhostPlaceholder, port),
		BlobURL:   fmt.Sprintf("http://%s:%d/repository/blobs", localhostPlaceholder, port),
	}, nil
}

type loggingWriter struct {
	http.ResponseWriter
	Status       int
	ResponseSize int64
}

func (lw *loggingWriter) WriteHeader(status int) {
	lw.Status = status
	lw.ResponseWriter.WriteHeader(status)
}

func (lw *loggingWriter) Write(b []byte) (int, error) {
	n, err := lw.ResponseWriter.Write(b)
	lw.ResponseSize += int64(n)
	return n, err
}
