// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This library implements a *very basic* fuchsia.io implementation for directories, files, and
// services. Most functionality is not available, nor does this library enforce any kind of
// connection rights. However, nodes are read-only from a client perspective (e.g. writing to files
// is not supported), and no new nodes can be created by clients.

// TODO(https://fxbug.dev/356225729): This library does not perform any rights checks, nor does it
// enforce hierarchal rights. This is mainly used for publishing services and read-only directory
// entries from components.

package component

import (
	"bytes"
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	stdio "io"
	"log"
	"runtime"
	"runtime/pprof"
	"strings"
	"syscall"
	"syscall/zx"
	"syscall/zx/fdio"
	"syscall/zx/fidl"
	"unsafe"

	"fidl/fuchsia/io"
	"fidl/fuchsia/unknown"
)

func respondDeprecated(flags io.OpenFlags, req io.NodeWithCtxInterfaceRequest, err error, node Node) error {
	_ = req.Close()
	return nil
}

func logError(err error) {
	log.Print(err)
}

type Node interface {
	getIO() (io.NodeWithCtx, func() error, error)
	addConnection(flags io.Flags, channel zx.Channel) error
	Representation() io.Representation
	addConnectionDeprecated(flags io.OpenFlags, mode io.ModeType, req io.NodeWithCtxInterfaceRequest) error
	DescribeDeprecated() io.NodeInfoDeprecated
}

func noop() error {
	return nil
}

type Service struct {
	// AddFn is called serially with an incoming request. It must not block, and
	// is expected to handle incoming calls on the request.
	AddFn func(context.Context, zx.Channel) error
}

var _ Node = (*Service)(nil)
var _ io.NodeWithCtx = (*Service)(nil)

func (s *Service) getIO() (io.NodeWithCtx, func() error, error) {
	return s, noop, nil
}

func (s *Service) addConnection(flags io.Flags, channel zx.Channel) error {
	if flags&io.FlagsProtocolNode != 0 {
		stub := io.NodeWithCtxStub{Impl: s}
		go Serve(context.Background(), &stub, channel, ServeOptions{
			OnError: logError,
		})
		if flags&io.FlagsFlagSendRepresentation != 0 {
			proxy := io.NodeEventProxy{Channel: channel}
			return proxy.OnRepresentation(s.Representation())
		}
		return nil
	}
	return s.AddFn(context.Background(), channel)
}

func (s *Service) addConnectionDeprecated(flags io.OpenFlags, mode io.ModeType, req io.NodeWithCtxInterfaceRequest) error {
	if flags&io.OpenFlagsNodeReference != 0 {
		stub := io.NodeWithCtxStub{Impl: s}
		go Serve(context.Background(), &stub, req.Channel, ServeOptions{
			OnError: logError,
		})
		return respondDeprecated(flags, req, nil, s)
	}
	return respondDeprecated(flags, req, s.AddFn(context.Background(), req.Channel), s)
}

func (s *Service) DeprecatedClone(ctx fidl.Context, flags io.OpenFlags, req io.NodeWithCtxInterfaceRequest) error {
	return s.addConnectionDeprecated(flags, 0, req)
}

func (s *Service) Clone(ctx fidl.Context, req unknown.CloneableWithCtxInterfaceRequest) error {
	return s.addConnection(io.Flags(0), req.Channel)
}

func (*Service) Close(fidl.Context) (unknown.CloseableCloseResult, error) {
	return unknown.CloseableCloseResultWithResponse(unknown.CloseableCloseResponse{}), nil
}

func (*Service) DescribeDeprecated() io.NodeInfoDeprecated {
	var nodeInfo io.NodeInfoDeprecated
	nodeInfo.SetService(io.Service{})
	return nodeInfo
}

func (*Service) Representation() io.Representation {
	return io.Representation{}
}

func (*Service) Sync(fidl.Context) (io.NodeSyncResult, error) {
	return io.NodeSyncResultWithErr(int32(zx.ErrNotSupported)), nil
}

const ModeTypeMask uint32 = 1044480
const ModeTypeService uint32 = 65536

func (*Service) DeprecatedGetAttr(fidl.Context) (int32, io.NodeAttributes, error) {
	return int32(zx.ErrOk), io.NodeAttributes{
		Mode:      ModeTypeService,
		Id:        io.InoUnknown,
		LinkCount: 1,
	}, nil
}

func (*Service) DeprecatedSetAttr(fidl.Context, io.NodeAttributeFlags, io.NodeAttributes) (int32, error) {
	return int32(zx.ErrNotSupported), nil
}

func (*Service) GetAttributes(fidl.Context, io.NodeAttributesQuery) (io.NodeGetAttributesResult, error) {
	attrs := io.NodeAttributes2{}
	attrs.ImmutableAttributes.SetProtocols(io.NodeProtocolKindsConnector)
	attrs.ImmutableAttributes.SetAbilities(io.OperationsConnect)
	return io.NodeGetAttributesResultWithResponse(attrs), nil
}

func (*Service) UpdateAttributes(fidl.Context, io.MutableNodeAttributes) (io.NodeUpdateAttributesResult, error) {
	return io.NodeUpdateAttributesResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*Service) ListExtendedAttributes(_ fidl.Context, request io.ExtendedAttributeIteratorWithCtxInterfaceRequest) error {
	if err := closeWithEpitaphIgnorePeerClosed(request.Channel, zx.ErrNotSupported); err != nil {
		logError(fmt.Errorf("Service.ListExtendedAttributes: %w", err))
	}
	return nil
}

func (*Service) GetExtendedAttribute(fidl.Context, []uint8) (io.NodeGetExtendedAttributeResult, error) {
	return io.NodeGetExtendedAttributeResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*Service) SetExtendedAttribute(fidl.Context, []uint8, io.ExtendedAttributeValue, io.SetExtendedAttributeMode) (io.NodeSetExtendedAttributeResult, error) {
	return io.NodeSetExtendedAttributeResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*Service) RemoveExtendedAttribute(fidl.Context, []uint8) (io.NodeRemoveExtendedAttributeResult, error) {
	return io.NodeRemoveExtendedAttributeResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*Service) DeprecatedGetFlags(fidl.Context) (int32, io.OpenFlags, error) {
	return int32(zx.ErrNotSupported), 0, nil
}

func (*Service) DeprecatedSetFlags(fidl.Context, io.OpenFlags) (int32, error) {
	return int32(zx.ErrNotSupported), nil
}

func (*Service) GetFlags(fidl.Context) (io.NodeGetFlagsResult, error) {
	return io.NodeGetFlagsResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*Service) SetFlags(fidl.Context, io.Flags) (io.NodeSetFlagsResult, error) {
	return io.NodeSetFlagsResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*Service) QueryFilesystem(fidl.Context) (int32, *io.FilesystemInfo, error) {
	return int32(zx.ErrNotSupported), nil, nil
}

func (*Service) Query(fidl.Context) ([]uint8, error) {
	return []byte(io.NodeProtocolName_), nil
}

type Directory interface {
	Get(string) (Node, bool)
	ForEach(func(string, Node) error) error
}

var _ Directory = mapDirectory(nil)

type mapDirectory map[string]Node

func (md mapDirectory) Get(name string) (Node, bool) {
	node, ok := md[name]
	return node, ok
}

func (md mapDirectory) ForEach(fn func(string, Node) error) error {
	for name, node := range md {
		if err := fn(name, node); err != nil {
			return err
		}
	}
	return nil
}

var _ Directory = (*pprofDirectory)(nil)

type pprofDirectory struct{}

func (*pprofDirectory) Get(name string) (Node, bool) {
	if p := pprof.Lookup(name); p != nil {
		return &FileWrapper{
			File: &pprofFile{
				p: p,
			},
		}, true
	}
	return nil, false
}

func (*pprofDirectory) ForEach(fn func(string, Node) error) error {
	for _, p := range pprof.Profiles() {
		if err := fn(p.Name(), &FileWrapper{
			File: &pprofFile{
				p: p,
			},
		}); err != nil {
			return err
		}
	}
	return nil
}

type DirectoryWrapper struct {
	Directory Directory
}

var _ Node = (*DirectoryWrapper)(nil)

func (dir *DirectoryWrapper) GetDirectory() io.DirectoryWithCtx {
	return &directoryState{
		DirectoryWrapper: dir,
		hasTraverse:      true,
	}
}

func (dir *DirectoryWrapper) getIO() (io.NodeWithCtx, func() error, error) {
	return dir.GetDirectory(), noop, nil
}

func (dir *DirectoryWrapper) addConnection(flags io.Flags, channel zx.Channel) error {
	ioDir := &directoryState{
		DirectoryWrapper: dir,
		hasTraverse:      (flags & io.FlagsPermTraverse) != 0,
	}
	stub := io.DirectoryWithCtxStub{Impl: ioDir}
	go Serve(context.Background(), &stub, channel, ServeOptions{
		OnError: logError,
	})
	if flags&io.FlagsFlagSendRepresentation != 0 {
		proxy := io.NodeEventProxy{Channel: channel}
		return proxy.OnRepresentation(dir.Representation())
	}
	return nil
}

func (dir *DirectoryWrapper) addConnectionDeprecated(flags io.OpenFlags, mode io.ModeType, req io.NodeWithCtxInterfaceRequest) error {
	ioDir := &directoryState{
		DirectoryWrapper: dir,
		hasTraverse:      (flags & io.OpenFlagsNodeReference) == 0,
	}
	stub := io.DirectoryWithCtxStub{Impl: ioDir}
	go Serve(context.Background(), &stub, req.Channel, ServeOptions{
		OnError: logError,
	})
	return respondDeprecated(flags, req, nil, dir)
}

var _ io.DirectoryWithCtx = (*directoryState)(nil)

type directoryState struct {
	*DirectoryWrapper

	reading bool
	dirents bytes.Buffer

	hasTraverse bool
}

func (dirState *directoryState) DeprecatedClone(ctx fidl.Context, flags io.OpenFlags, req io.NodeWithCtxInterfaceRequest) error {
	return dirState.addConnectionDeprecated(flags, 0, req)
}

func (dirState *directoryState) Clone(ctx fidl.Context, req unknown.CloneableWithCtxInterfaceRequest) error {
	return dirState.addConnection(io.Flags(0), req.Channel)
}

func (*directoryState) Close(fidl.Context) (unknown.CloseableCloseResult, error) {
	return unknown.CloseableCloseResultWithResponse(unknown.CloseableCloseResponse{}), nil
}

func (*DirectoryWrapper) Representation() io.Representation {
	var repr io.Representation
	repr.SetDirectory(io.DirectoryInfo{})
	return repr
}

func (*DirectoryWrapper) DescribeDeprecated() io.NodeInfoDeprecated {
	var nodeInfo io.NodeInfoDeprecated
	nodeInfo.SetDirectory(io.DirectoryObject{})
	return nodeInfo
}

func (*directoryState) Sync(fidl.Context) (io.NodeSyncResult, error) {
	return io.NodeSyncResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*directoryState) DeprecatedGetAttr(fidl.Context) (int32, io.NodeAttributes, error) {
	return int32(zx.ErrOk), io.NodeAttributes{
		Mode:      uint32(fdio.VtypeDir | fdio.VtypeIRUSR),
		Id:        io.InoUnknown,
		LinkCount: 1,
	}, nil
}

func (*directoryState) DeprecatedSetAttr(fidl.Context, io.NodeAttributeFlags, io.NodeAttributes) (int32, error) {
	return int32(zx.ErrNotSupported), nil
}

func (*directoryState) GetAttributes(fidl.Context, io.NodeAttributesQuery) (io.NodeGetAttributesResult, error) {
	attrs := io.NodeAttributes2{}
	attrs.ImmutableAttributes.SetProtocols(io.NodeProtocolKindsDirectory)
	attrs.ImmutableAttributes.SetAbilities(io.RStarDir)
	return io.NodeGetAttributesResultWithResponse(attrs), nil
}

func (*directoryState) UpdateAttributes(fidl.Context, io.MutableNodeAttributes) (io.NodeUpdateAttributesResult, error) {
	return io.NodeUpdateAttributesResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*directoryState) ListExtendedAttributes(_ fidl.Context, request io.ExtendedAttributeIteratorWithCtxInterfaceRequest) error {
	if err := closeWithEpitaphIgnorePeerClosed(request.Channel, zx.ErrNotSupported); err != nil {
		logError(fmt.Errorf("directoryState.ListExtendedAttributes: %w", err))
	}
	return nil
}

func (*directoryState) GetExtendedAttribute(fidl.Context, []uint8) (io.NodeGetExtendedAttributeResult, error) {
	return io.NodeGetExtendedAttributeResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*directoryState) SetExtendedAttribute(fidl.Context, []uint8, io.ExtendedAttributeValue, io.SetExtendedAttributeMode) (io.NodeSetExtendedAttributeResult, error) {
	return io.NodeSetExtendedAttributeResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*directoryState) RemoveExtendedAttribute(fidl.Context, []uint8) (io.NodeRemoveExtendedAttributeResult, error) {
	return io.NodeRemoveExtendedAttributeResultWithErr(int32(zx.ErrNotSupported)), nil
}

const dot = "."

func (dirState *directoryState) DeprecatedOpen(ctx fidl.Context, flags io.OpenFlags, mode io.ModeType, path string, req io.NodeWithCtxInterfaceRequest) error {
	return respondDeprecated(flags, req, &zx.Error{Status: zx.ErrNotSupported}, dirState)
}

func (dirState *directoryState) Open(ctx fidl.Context, path string, flags io.Flags, options io.Options, channel zx.Channel) error {
	if path != dot && !dirState.hasTraverse {
		if err := closeWithEpitaphIgnorePeerClosed(channel, zx.ErrAccessDenied); err != nil {
			logError(fmt.Errorf("directoryState.Open: %w", err))
		}
		return nil
	}
	if path == dot {
		return dirState.addConnection(flags, channel)
	}
	const slash = "/"
	if strings.HasSuffix(path, slash) {
		path = path[:len(path)-len(slash)]
	}

	if i := strings.Index(path, slash); i != -1 {
		if node, ok := dirState.Directory.Get(path[:i]); ok {
			proxy, cleanup, err := node.getIO()
			if err != nil {
				return err
			}
			defer cleanup()
			if dir, ok := proxy.(io.DirectoryWithCtx); ok {
				return dir.Open(ctx, path[i+len(slash):], flags, options, channel)
			}
			if err := closeWithEpitaphIgnorePeerClosed(channel, zx.ErrNotDir); err != nil {
				logError(fmt.Errorf("directoryState.Open: %w", err))
			}
			return nil
		}
	} else if node, ok := dirState.Directory.Get(path); ok {
		return node.addConnection(flags, channel)
	}

	if err := closeWithEpitaphIgnorePeerClosed(channel, zx.ErrNotDir); err != nil {
		logError(fmt.Errorf("directoryState.Open: %w", err))
	}
	return nil
}

func (*directoryState) Unlink(fidl.Context, string, io.UnlinkOptions) (io.DirectoryUnlinkResult, error) {
	return io.DirectoryUnlinkResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*directoryState) CreateSymlink(fidl.Context, string, []uint8, io.SymlinkWithCtxInterfaceRequest) (io.DirectoryCreateSymlinkResult, error) {
	return io.DirectoryCreateSymlinkResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (dirState *directoryState) ReadDirents(ctx fidl.Context, maxOut uint64) (int32, []uint8, error) {
	if !dirState.reading {
		writeFn := func(name string, node Node) error {
			ioNode, cleanup, err := node.getIO()
			if err != nil {
				return err
			}
			defer cleanup()
			status, attr, err := ioNode.DeprecatedGetAttr(ctx)
			if err != nil {
				return err
			}
			if status := zx.Status(status); status != zx.ErrOk {
				return fmt.Errorf("io.Node.GetAttr returned non-ok zx.Status %s", status)
			}
			dirent := syscall.Dirent{
				Ino:  attr.Id,
				Size: uint8(len(name)),
				Type: uint8(func() io.DirentType {
					switch modeType := attr.Mode & ModeTypeMask; modeType {
					case uint32(fdio.VtypeDir):
						return io.DirentTypeDirectory
					case uint32(fdio.VtypeFile):
						return io.DirentTypeFile
					case ModeTypeService:
						return io.DirentTypeService
					default:
						panic(fmt.Sprintf("unknown mode type: %b", modeType))
					}
				}()),
			}
			if err := binary.Write(&dirState.dirents, binary.LittleEndian, dirent); err != nil {
				return err
			}
			dirState.dirents.Truncate(dirState.dirents.Len() - int(unsafe.Sizeof(syscall.Dirent{}.Name)))
			if _, err := dirState.dirents.WriteString(name); err != nil {
				return err
			}
			return nil
		}
		if err := writeFn(dot, dirState); err != nil {
			return 0, nil, err
		}
		if err := dirState.Directory.ForEach(writeFn); err != nil {
			return 0, nil, err
		}
		dirState.reading = true
	} else if dirState.dirents.Len() == 0 {
		status, err := dirState.Rewind(ctx)
		if err != nil {
			return 0, nil, err
		}
		if status := zx.Status(status); status != zx.ErrOk {
			return 0, nil, fmt.Errorf("dirState.Rewind(_) = %s", status)
		}
	}
	return int32(zx.ErrOk), dirState.dirents.Next(int(maxOut)), nil
}

func (dirState *directoryState) Rewind(fidl.Context) (int32, error) {
	dirState.reading = false
	dirState.dirents.Reset()
	return int32(zx.ErrOk), nil
}

func (*directoryState) GetToken(fidl.Context) (int32, zx.Handle, error) {
	return int32(zx.ErrNotSupported), zx.HandleInvalid, nil
}

func (*directoryState) Rename(fidl.Context, string, zx.Event, string) (io.DirectoryRenameResult, error) {
	return io.DirectoryRenameResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*directoryState) Link(fidl.Context, string, zx.Handle, string) (int32, error) {
	return int32(zx.ErrNotSupported), nil
}

func (*directoryState) Watch(_ fidl.Context, _ io.WatchMask, _ uint32, watcher io.DirectoryWatcherWithCtxInterfaceRequest) (int32, error) {
	if err := watcher.Close(); err != nil {
		logError(err)
	}
	return int32(zx.ErrNotSupported), nil
}

func (*directoryState) DeprecatedGetFlags(fidl.Context) (int32, io.OpenFlags, error) {
	return int32(zx.ErrNotSupported), 0, nil
}

func (*directoryState) DeprecatedSetFlags(fidl.Context, io.OpenFlags) (int32, error) {
	return int32(zx.ErrNotSupported), nil
}

func (*directoryState) GetFlags(fidl.Context) (io.NodeGetFlagsResult, error) {
	return io.NodeGetFlagsResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*directoryState) SetFlags(fidl.Context, io.Flags) (io.NodeSetFlagsResult, error) {
	return io.NodeSetFlagsResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (dirState *directoryState) AdvisoryLock(fidl.Context, io.AdvisoryLockRequest) (io.AdvisoryLockingAdvisoryLockResult, error) {
	return io.AdvisoryLockingAdvisoryLockResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*directoryState) QueryFilesystem(fidl.Context) (int32, *io.FilesystemInfo, error) {
	return int32(zx.ErrNotSupported), nil, nil
}

func (*directoryState) Query(fidl.Context) ([]uint8, error) {
	return []byte(io.DirectoryProtocolName_), nil
}

type File interface {
	// If error is non-nil, GetReader returns a component.Reader for the file
	// contents, and the length of the file contents.
	GetReader() (Reader, uint64, error)
}

var _ File = (*pprofFile)(nil)

type pprofFile struct {
	p *pprof.Profile
}

func (p *pprofFile) GetReader() (Reader, uint64, error) {
	var b bytes.Buffer
	if err := p.p.WriteTo(&b, 0); err != nil {
		return nil, 0, err
	}
	return NoVMO(NopCloser(bytes.NewReader(b.Bytes()))), uint64(b.Len()), nil
}

var _ File = (*stackTraceFile)(nil)

// stackTraceFile provides a File implementation to expose goroutine
// stacks.
type stackTraceFile struct{}

func (f *stackTraceFile) GetReader() (Reader, uint64, error) {
	buf := make([]byte, 4096)
	for {
		n := runtime.Stack(buf, true)
		if n < len(buf) {
			return NoVMO(NopCloser(bytes.NewReader(buf[:n]))), uint64(n), nil
		}
		buf = make([]byte, 2*len(buf))
	}
}

var _ Node = (*FileWrapper)(nil)

type FileWrapper struct {
	File File
}

func (file *FileWrapper) getFileState() (*fileState, error) {
	reader, size, err := file.File.GetReader()
	if err != nil {
		return nil, err
	}
	return &fileState{
		FileWrapper: file,
		reader:      reader,
		size:        size,
	}, nil
}

func (file *FileWrapper) GetFile() (io.FileWithCtx, error) {
	return file.getFileState()
}

func (file *FileWrapper) getIO() (io.NodeWithCtx, func() error, error) {
	state, err := file.getFileState()
	if err != nil {
		return nil, noop, err
	}
	return state, state.reader.Close, nil
}

func (file *FileWrapper) addConnection(flags io.Flags, channel zx.Channel) error {
	ioFile, err := file.getFileState()
	if err != nil {
		return err
	}
	stub := io.FileWithCtxStub{Impl: ioFile}
	go func() {
		defer ioFile.reader.Close()
		Serve(context.Background(), &stub, channel, ServeOptions{
			OnError: logError,
		})
	}()
	if flags&io.FlagsFlagSendRepresentation != 0 {
		proxy := io.NodeEventProxy{Channel: channel}
		return proxy.OnRepresentation(file.Representation())
	}
	return nil
}

func (file *FileWrapper) addConnectionDeprecated(flags io.OpenFlags, mode io.ModeType, req io.NodeWithCtxInterfaceRequest) error {
	ioFile, err := file.getFileState()
	if err != nil {
		return err
	}
	stub := io.FileWithCtxStub{Impl: ioFile}
	go func() {
		defer ioFile.reader.Close()
		Serve(context.Background(), &stub, req.Channel, ServeOptions{
			OnError: logError,
		})
	}()
	return respondDeprecated(flags, req, nil, ioFile)
}

var _ io.FileWithCtx = (*fileState)(nil)

type ReaderWithoutCloser interface {
	stdio.Reader
	stdio.ReaderAt
	stdio.Seeker
}

type ReaderWithoutGetVMO interface {
	ReaderWithoutCloser
	stdio.Closer
}

type Reader interface {
	ReaderWithoutGetVMO
	// GetVMO returns a pointer to the Reader's handle to its backing VMO, if the
	// Reader is backed by a VMO.
	GetVMO() *zx.VMO
}

type nopCloser struct {
	ReaderWithoutCloser
}

func (nopCloser) Close() error { return nil }

// NopCloser implements Closer for Readers that don't need closing. We can't just use io.NopCloser()
// because it doesn't compose with stdio.ReaderAt.
func NopCloser(r ReaderWithoutCloser) ReaderWithoutGetVMO {
	return nopCloser{ReaderWithoutCloser: r}
}

type noVMO struct {
	ReaderWithoutGetVMO
}

func (*noVMO) GetVMO() *zx.VMO {
	return nil
}

// NoVMO implements GetVMO for Readers that don't have a backing VMO.
func NoVMO(r ReaderWithoutGetVMO) Reader {
	return &noVMO{ReaderWithoutGetVMO: r}
}

type fileState struct {
	*FileWrapper
	reader Reader
	size   uint64
}

func (fState *fileState) DeprecatedClone(ctx fidl.Context, flags io.OpenFlags, req io.NodeWithCtxInterfaceRequest) error {
	return fState.addConnectionDeprecated(flags, 0, req)
}

func (fState *fileState) Clone(ctx fidl.Context, req unknown.CloneableWithCtxInterfaceRequest) error {
	return fState.addConnection(io.Flags(0), req.Channel)
}

func (fState *fileState) Close(fidl.Context) (unknown.CloseableCloseResult, error) {
	return unknown.CloseableCloseResultWithResponse(unknown.CloseableCloseResponse{}), fState.reader.Close()
}

func (*FileWrapper) Representation() io.Representation {
	var repr io.Representation
	repr.SetFile(io.FileInfo{})
	return repr
}

func (*FileWrapper) DescribeDeprecated() io.NodeInfoDeprecated {
	var nodeInfo io.NodeInfoDeprecated
	nodeInfo.SetFile(io.FileObject{})
	return nodeInfo
}

func (fState *fileState) Describe(fidl.Context) (io.FileInfo, error) {
	var fileInfo io.FileInfo
	return fileInfo, nil
}

func (*fileState) LinkInto(fidl.Context, zx.Event, string) (io.LinkableLinkIntoResult, error) {
	return io.LinkableLinkIntoResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*fileState) Sync(fidl.Context) (io.NodeSyncResult, error) {
	return io.NodeSyncResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (fState *fileState) DeprecatedGetAttr(fidl.Context) (int32, io.NodeAttributes, error) {
	return int32(zx.ErrOk), io.NodeAttributes{
		Mode:        uint32(fdio.VtypeFile | fdio.VtypeIRUSR),
		Id:          io.InoUnknown,
		ContentSize: fState.size,
		LinkCount:   1,
	}, nil
}

func (*fileState) DeprecatedSetAttr(fidl.Context, io.NodeAttributeFlags, io.NodeAttributes) (int32, error) {
	return int32(zx.ErrNotSupported), nil
}

func (*fileState) GetAttributes(fidl.Context, io.NodeAttributesQuery) (io.NodeGetAttributesResult, error) {
	attrs := io.NodeAttributes2{}
	attrs.ImmutableAttributes.SetProtocols(io.NodeProtocolKindsFile)
	attrs.ImmutableAttributes.SetAbilities(io.OperationsReadBytes | io.OperationsGetAttributes)
	return io.NodeGetAttributesResultWithResponse(attrs), nil
}

func (*fileState) UpdateAttributes(fidl.Context, io.MutableNodeAttributes) (io.NodeUpdateAttributesResult, error) {
	return io.NodeUpdateAttributesResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*fileState) ListExtendedAttributes(_ fidl.Context, request io.ExtendedAttributeIteratorWithCtxInterfaceRequest) error {
	if err := closeWithEpitaphIgnorePeerClosed(request.Channel, zx.ErrNotSupported); err != nil {
		logError(fmt.Errorf("fileState.ListExtendedAttributes: %w", err))
	}
	return nil
}

func (*fileState) GetExtendedAttribute(fidl.Context, []uint8) (io.NodeGetExtendedAttributeResult, error) {
	return io.NodeGetExtendedAttributeResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*fileState) SetExtendedAttribute(fidl.Context, []uint8, io.ExtendedAttributeValue, io.SetExtendedAttributeMode) (io.NodeSetExtendedAttributeResult, error) {
	return io.NodeSetExtendedAttributeResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*fileState) RemoveExtendedAttribute(fidl.Context, []uint8) (io.NodeRemoveExtendedAttributeResult, error) {
	return io.NodeRemoveExtendedAttributeResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*fileState) Allocate(fidl.Context, uint64, uint64, io.AllocateMode) (io.FileAllocateResult, error) {
	return io.FileAllocateResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*fileState) EnableVerity(fidl.Context, io.VerificationOptions) (io.FileEnableVerityResult, error) {
	return io.FileEnableVerityResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (fState *fileState) Read(_ fidl.Context, count uint64) (io.ReadableReadResult, error) {
	if l := fState.size; l < count {
		count = l
	}
	b := make([]byte, count)
	n, err := fState.reader.Read(b)
	if err != nil && err != stdio.EOF {
		return io.ReadableReadResult{}, err
	}
	b = b[:n]
	return io.ReadableReadResultWithResponse(io.ReadableReadResponse{
		Data: b,
	}), nil
}

func (fState *fileState) ReadAt(_ fidl.Context, count uint64, offset uint64) (io.FileReadAtResult, error) {
	if l := fState.size - offset; l < count {
		count = l
	}
	b := make([]byte, count)
	n, err := fState.reader.ReadAt(b, int64(offset))
	if err != nil && err != stdio.EOF {
		return io.FileReadAtResult{}, err
	}
	b = b[:n]
	return io.FileReadAtResultWithResponse(io.FileReadAtResponse{
		Data: b,
	}), nil
}

func (*fileState) Write(fidl.Context, []uint8) (io.WritableWriteResult, error) {
	return io.WritableWriteResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*fileState) WriteAt(fidl.Context, []uint8, uint64) (io.FileWriteAtResult, error) {
	return io.FileWriteAtResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (fState *fileState) Seek(_ fidl.Context, origin io.SeekOrigin, offset int64) (io.FileSeekResult, error) {
	n, err := fState.reader.Seek(offset, int(origin))
	return io.FileSeekResultWithResponse(
		io.FileSeekResponse{
			OffsetFromStart: uint64(n),
		}), err
}

func (*fileState) Resize(fidl.Context, uint64) (io.FileResizeResult, error) {
	return io.FileResizeResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*fileState) DeprecatedGetFlags(fidl.Context) (int32, io.OpenFlags, error) {
	return int32(zx.ErrNotSupported), 0, nil
}

func (*fileState) DeprecatedSetFlags(fidl.Context, io.OpenFlags) (int32, error) {
	return int32(zx.ErrNotSupported), nil
}

func (*fileState) GetFlags(fidl.Context) (io.NodeGetFlagsResult, error) {
	return io.NodeGetFlagsResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*fileState) SetFlags(fidl.Context, io.Flags) (io.NodeSetFlagsResult, error) {
	return io.NodeSetFlagsResultWithErr(int32(zx.ErrNotSupported)), nil
}

func (*fileState) QueryFilesystem(fidl.Context) (int32, *io.FilesystemInfo, error) {
	return int32(zx.ErrNotSupported), nil, nil
}

func (*fileState) Query(fidl.Context) ([]byte, error) {
	return []byte(io.FileProtocolName_), nil
}

func (fState *fileState) AdvisoryLock(fidl.Context, io.AdvisoryLockRequest) (io.AdvisoryLockingAdvisoryLockResult, error) {
	return io.AdvisoryLockingAdvisoryLockResult{}, &zx.Error{Status: zx.ErrNotSupported, Text: fmt.Sprintf("%T", fState)}
}

func (fState *fileState) GetBackingMemory(fidl.Context, io.VmoFlags) (io.FileGetBackingMemoryResult, error) {
	if vmo := fState.reader.GetVMO(); vmo != nil {
		// TODO(https://fxbug.dev/356225729): The rights on the VMO we return here should be capped at
		// the intersection of the rights in the request and those on this connection.
		h, err := vmo.Handle().Duplicate(zx.RightSameRights)
		switch err := err.(type) {
		case nil:
			return io.FileGetBackingMemoryResultWithResponse(io.FileGetBackingMemoryResponse{
				Vmo: zx.VMO(h),
			}), nil
		case *zx.Error:
			return io.FileGetBackingMemoryResultWithErr(int32(err.Status)), nil
		default:
			return io.FileGetBackingMemoryResult{}, err
		}
	}
	return io.FileGetBackingMemoryResultWithErr(int32(zx.ErrNotSupported)), nil
}

// closeWithEpitaphIgnorePeerClosed closes req with status as its epitaph. It
// returns observed errors except for peer closed.
func closeWithEpitaphIgnorePeerClosed(req zx.Channel, status zx.Status) error {
	err := CloseWithEpitaph(req, status)
	var e *zx.Error
	if errors.As(err, &e) {
		if e.Status == zx.ErrPeerClosed {
			return nil
		}
	}
	return err
}
