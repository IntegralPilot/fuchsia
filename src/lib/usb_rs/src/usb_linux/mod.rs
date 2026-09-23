// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod usbdevice_fs;
use futures::lock::Mutex as AsyncMutex;
use usbdevice_fs::*;
mod discovery;
pub use discovery::{enumerate_devices, find_device_by_serial, wait_for_devices};

use fuchsia_sync::Mutex;
use futures::future::poll_fn;
use futures::task::AtomicWaker;
use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::{Poll, Waker};
use std::thread;
use zerocopy::Ref;

use crate::{
    DeviceDescriptor, Endpoint, EndpointDescriptor, EndpointDirection, EndpointType, Error,
    InterfaceDescriptor, Result, USB_ENDPOINT_DIR_MASK, ZeroPacket,
};

/// The Linux sysfs folder where usb devices are stored
static SYSFS_DEVICES_DIR: &str = "/sys/bus/usb/devices";

#[derive(Debug)]
pub struct DeviceHandleInner {
    pub(crate) hdl: String,
    pub(crate) serial: std::sync::OnceLock<Option<String>>,
}

/// Represents a USB device with its major and minor numbers.
#[derive(Debug, PartialEq, Eq)]
struct DeviceId {
    major: u32,
    minor: u32,
}

impl DeviceId {
    /// Creates a new DeviceId from a given path by reading its metadata.
    fn from_path<P>(path: P) -> Result<Self, std::io::Error>
    where
        P: AsRef<std::path::Path>,
    {
        let metadata = std::fs::metadata(path)?;
        let rdev = metadata.rdev();
        let major = libc::major(rdev);
        let minor = libc::minor(rdev);
        Ok(DeviceId { major, minor })
    }
}

/// Finds the sysfs path for a given USB device ID.
///
/// It searches through `/sys/bus/usb/devices` to find a device directory
/// that contains a 'dev' file matching the major and minor numbers.
fn find_sysfs_path(device_id: &DeviceId) -> Option<std::path::PathBuf> {
    let sys_usb_path = std::path::Path::new(SYSFS_DEVICES_DIR);
    if let Ok(entries) = std::fs::read_dir(sys_usb_path) {
        for entry in entries {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_dir() {
                    let dev_file_path = path.join("dev");
                    if dev_file_path.exists() {
                        if let Ok(content) = std::fs::read_to_string(&dev_file_path) {
                            let parts: Vec<&str> = content.trim().split(':').collect();
                            if parts.len() == 2 {
                                if let (Ok(major), Ok(minor)) =
                                    (parts[0].parse::<u32>(), parts[1].parse::<u32>())
                                {
                                    if device_id.major == major && device_id.minor == minor {
                                        return Some(path);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Reads the serial number from the 'serial' file in the given sysfs device path.
/// Returns None if there is an error
fn get_serial_from_sysfs<P>(sysfs_path: P) -> Option<String>
where
    P: AsRef<std::path::Path>,
{
    let serial_path = sysfs_path.as_ref().join("serial");
    std::fs::read_to_string(serial_path).map(|s| s.trim().to_string()).ok()
}

/// Reads the negotiated speed from the 'speed' file in the given sysfs device path.
/// Returns None if there is an error
fn get_speed_from_sysfs<P>(sysfs_path: P) -> Option<crate::UsbSpeed>
where
    P: AsRef<std::path::Path>,
{
    let speed_path = sysfs_path.as_ref().join("speed");
    let speed_str = std::fs::read_to_string(speed_path).ok()?;
    Some(crate::UsbSpeed::from_sysfs_str(speed_str.trim()))
}

impl DeviceHandleInner {
    pub(crate) fn new(hdl: String) -> DeviceHandleInner {
        DeviceHandleInner { hdl, serial: std::sync::OnceLock::new() }
    }

    pub fn sysfs_path(&self) -> Option<std::path::PathBuf> {
        let devid = DeviceId::from_path(&self.hdl).ok()?;
        find_sysfs_path(&devid)
    }

    pub fn serial(&self) -> Option<String> {
        self.serial
            .get_or_init(|| {
                let sysfs_path = self.sysfs_path()?;
                get_serial_from_sysfs(&sysfs_path)
            })
            .clone()
    }

    pub fn speed(&self) -> Option<crate::UsbSpeed> {
        let sysfs_path = self.sysfs_path()?;
        get_speed_from_sysfs(&sysfs_path)
    }

    pub fn debug_name(&self) -> String {
        self.hdl.clone()
    }

    pub fn scan_interfaces(
        &self,
        urb_pool_size: usize,
        f: impl Fn(&DeviceDescriptor, &InterfaceDescriptor) -> bool,
    ) -> Result<Interface> {
        // The endpoint descriptor comes in two lengths. We only have the struct for the larger one, and
        // the assumption from C land is we'd just let the end of it hang off the end of the buffer and
        // not touch the extra fields when we don't have them. To make this vibe with Rust we'll just
        // make the buffer a touch longer so this is always safe. For the scorekeepers, this should be
        // == 2.
        const OVERRUN: usize = (USB_DT_ENDPOINT_AUDIO_SIZE - USB_DT_ENDPOINT_SIZE) as usize;

        let mut file = match OpenOptions::new().read(true).write(true).open(&self.hdl) {
            Ok(file) => file,
            Err(err) => {
                if err.kind() != std::io::ErrorKind::PermissionDenied {
                    return Err(err.into());
                }
                OpenOptions::new().read(true).open(&self.hdl)?
            }
        };
        let mut descriptor_buf = [0u8; 4096 + OVERRUN];
        // The usbdevfs API suggests that one read will always return the whole descriptor.
        let descriptor_length = file.read(&mut descriptor_buf[..4096])?;
        let mut descriptor_buf = &descriptor_buf[..descriptor_length + OVERRUN];

        let mut iface = Option::<InterfaceDescriptor>::None;
        let mut device = Option::<DeviceDescriptor>::None;

        while descriptor_buf.len() >= std::mem::size_of::<usb_descriptor_header>() + OVERRUN {
            let (header, _) = Ref::<_, usb_descriptor_header>::from_prefix(descriptor_buf)
                .map_err(|_| Error::MalformedDescriptor)?;
            let length = header.bLength as usize;
            if length > descriptor_buf.len() {
                return Err(Error::MalformedDescriptor);
            }

            if device.is_none() {
                if header.bDescriptorType == USB_DT_DEVICE as u8 {
                    let (descriptor, _) =
                        Ref::<_, usb_device_descriptor>::from_prefix(descriptor_buf)
                            .map_err(|_| Error::MalformedDescriptor)?;

                    device = Some(DeviceDescriptor {
                        vendor: u16::from_le(descriptor.idVendor),
                        product: u16::from_le(descriptor.idProduct),
                        class: descriptor.bDeviceClass,
                        subclass: descriptor.bDeviceSubClass,
                        protocol: descriptor.bDeviceProtocol,
                    });
                }
            } else if header.bDescriptorType == USB_DT_DEVICE as u8 {
                return Err(Error::MalformedDescriptor);
            }

            let device = device.as_ref().ok_or(Error::MalformedDescriptor)?;

            if header.bDescriptorType == USB_DT_ENDPOINT as u8 {
                let Some(iface) = iface.as_mut() else {
                    return Err(Error::MalformedDescriptor);
                };

                let (descriptor, _) =
                    Ref::<_, usb_endpoint_descriptor>::from_prefix(descriptor_buf)
                        .map_err(|_| Error::MalformedDescriptor)?;

                iface.add_endpoint(&*descriptor);
            } else if header.bDescriptorType == USB_DT_INTERFACE as u8 {
                let (descriptor, _) =
                    Ref::<_, usb_interface_descriptor>::from_prefix(descriptor_buf)
                        .map_err(|_| Error::MalformedDescriptor)?;

                if let Some(iface) = iface.replace(InterfaceDescriptor::from_ch9(&*descriptor)) {
                    if f(&device, &iface) {
                        return Interface::new(file, iface, None, urb_pool_size);
                    }
                }
            }

            descriptor_buf = &descriptor_buf[length..];
        }

        if let Some(iface) = iface {
            let device = device.as_ref().ok_or(Error::MalformedDescriptor)?;
            if f(&device, &iface) {
                return Interface::new(file, iface, None, urb_pool_size);
            }
        }

        Err(Error::InterfaceNotFound)
    }
}

/// Wrapper around the Linux URB, which is a structure used to communicate about a transaction to
/// the kernel.
struct Urb {
    urb: UnsafeCell<usbdevfs_urb>,
    buf: UnsafeCell<Pin<Box<[u8]>>>,
    waker: AtomicWaker,
    refs: AtomicU8,
}

impl Urb {
    /// Construct a new Urb value.
    fn new() -> Self {
        let mut buf = UnsafeCell::new(Box::pin([]));
        Urb {
            urb: UnsafeCell::new(usbdevfs_urb {
                type_: 0,
                endpoint: 0,
                status: 0,
                flags: 0,
                buffer: buf.get_mut().as_mut_ptr() as *mut libc::c_void,
                buffer_length: 0,
                actual_length: 0,
                start_frame: 0,
                __bindgen_anon_1: usbdevfs_urb__bindgen_ty_1 { stream_id: 0 },
                error_count: 0,
                signr: 0,
                usercontext: std::ptr::null_mut(),
                // Zero-sized trailing field. The allocation pattern will need to be more
                // sophisticated here if we ever need to write to this.
                iso_frame_desc: usbdevice_fs::__IncompleteArrayField::new(),
            }),
            buf,
            waker: AtomicWaker::new(),
            refs: AtomicU8::new(0),
        }
    }

    /// Populate the buffer of this URB.
    ///
    /// # Safety
    ///
    /// This mutates the buffer geometry in place without additional
    /// synchronization. It is generally only safe to call when the buffer is
    /// not in use, i.e. when it has just been allocated from the free pool.
    unsafe fn fill_buffer(&self, action: &BufferAction<'_, '_>) -> Result<()> {
        let mtu = action.mtu();
        let urb = unsafe { self.urb.get().as_mut() }.unwrap();
        let buf = unsafe { self.buf.get().as_mut() }.unwrap();

        if buf.len() < mtu {
            let new_buf = Pin::new(std::iter::repeat(0).take(mtu).collect());
            *buf = new_buf;
            urb.buffer = buf.as_mut_ptr() as *mut libc::c_void;
            urb.actual_length = 0;
        }

        let len = if let BufferAction::CopyIn(contents) = action {
            buf[..contents.len()].copy_from_slice(contents);
            contents.len()
        } else {
            mtu
        };

        urb.buffer_length = len.try_into().map_err(|_| Error::BufferTooBig(len))?;

        Ok(())
    }
}

// SAFETY: The Linux data structure which is causing these to not be auto-derived is designed to be
// shared with kernelspace concurrently.
unsafe impl Send for Urb {}
unsafe impl Sync for Urb {}

/// Thin wrapper around [`libc::ioctl`] that gives us a rusty error report.
macro_rules! ioctl {
    ($($args:tt)*) => {
        loop {
            if libc::ioctl($($args)*) < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::EINTR) {
                    break Err(err)
                }
            } else {
                break Ok(());
            }
        }
    };
}

/// Allocatable pinned Urbs.
struct UrbQueue {
    /// Indices of Urbs in [`InterfaceInner`] that are free.
    free_urbs: VecDeque<usize>,
    /// For things waiting for free Urbs.
    wakers: Vec<Waker>,
}

impl UrbQueue {
    /// Construct a new UrbQueue.
    fn new(urbs: &[Urb]) -> Self {
        let mut free_urbs = VecDeque::new();

        for idx in 0..urbs.len() {
            free_urbs.push_front(idx);
        }

        UrbQueue { free_urbs, wakers: Vec::new() }
    }
}

/// Reference to an allocated Urb that will release it when dropped.
struct UrbRef<'a>(&'a InterfaceInner, usize);

impl Drop for UrbRef<'_> {
    fn drop(&mut self) {
        self.0.cancel_urb_by_id(self.1);
        if self.refs.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.free_urb_by_id(self.1);
        }
    }
}

impl std::ops::Deref for UrbRef<'_> {
    type Target = Urb;

    fn deref(&self) -> &Self::Target {
        &self.0.urbs[self.1]
    }
}

/// Owned reference to an allocated Urb that releases it when dropped.
struct OwnedUrbRef(Arc<InterfaceInner>, usize);

impl Drop for OwnedUrbRef {
    fn drop(&mut self) {
        self.0.cancel_urb_by_id(self.1);
        if self.0.urbs[self.1].refs.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.free_urb_by_id(self.1);
        }
    }
}

impl std::ops::Deref for OwnedUrbRef {
    type Target = Urb;

    fn deref(&self) -> &Self::Target {
        &self.0.urbs[self.1]
    }
}

/// What to do with the buffer associated with a URB when submitting.
enum BufferAction<'i, 'o> {
    /// Copy data into the buffer before submission
    CopyIn(&'i [u8]),
    /// Copy data out of the buffer after submission
    CopyOut(&'o mut [u8]),
}

impl BufferAction<'_, '_> {
    /// What size of buffer should we allocate for this URB.
    fn mtu(&self) -> usize {
        match self {
            // When copying in we're doing a write. We should allocate a
            // slightly larger buffer so that future writes won't cause a
            // reallocation.
            Self::CopyIn(x) => x.len().next_power_of_two(),

            // When copying out we're doing a read. No point in reading more
            // data than the read call is prepared to receive as we'd have
            // nowhere to put it.
            Self::CopyOut(x) => x.len(),
        }
    }
}

/// Test stubs for creating a fake interface.
trait IoctlStub: Send + Sync {
    /// Stub for USBDEVFS_SUBMITURB
    fn submit(&self, fd: RawFd, urb: *mut usbdevfs_urb) -> Result<(), std::io::Error>;
    /// Stub for USBDEVFS_CLAIMINTERFACE
    fn claim_interface(&self, fd: RawFd, iface: *mut u32) -> Result<(), std::io::Error>;
    /// Stub for USBDEVFS_SETINTERFACE
    fn set_interface(
        &self,
        fd: RawFd,
        set_struct: *mut usbdevfs_setinterface,
    ) -> Result<(), std::io::Error>;
    /// Stub for USBDEVFS_REAPURB
    fn reap_urb(&self, fd: RawFd, urb_ptr: *mut *mut usbdevfs_urb) -> Result<(), std::io::Error>;
    /// Stub for USBDEVFS_DISCARDURB
    fn discard_urb(&self, fd: RawFd, urb: *mut usbdevfs_urb) -> Result<(), std::io::Error>;
    /// Stub for USBDEVFS_RELEASEINTERFACE
    fn release_interface(&self, fd: RawFd, iface: *mut u32) -> Result<(), std::io::Error>;
}

/// Internal state of [`Interface`]
pub struct InterfaceInner {
    /// The open device file for the USB device. The descriptor within will be configured to refer
    /// to the specific interface we want already.
    file: File,

    /// Queue of Urbs to allocate from.
    urb_queue: Mutex<UrbQueue>,

    /// Urbs,
    urbs: Pin<Box<[Urb]>>,

    /// Number of Urbs that are waiting to be reaped. Needs to be wide enough to fit the length of
    /// `urbs` to avoid overflow issues.
    pending_urbs: AtomicU8,

    /// Join handle for the thread which reaps our Urbs after submission,
    reaper_thread: thread::JoinHandle<()>,

    /// Stub interface for testing.
    stubs: Option<Arc<dyn IoctlStub>>,
}

impl InterfaceInner {
    /// Allocate a Urb from the pool.
    async fn alloc_urb<'a>(&'a self, action: &BufferAction<'_, '_>) -> Result<UrbRef<'a>> {
        poll_fn(move |ctx| {
            let mut queue = self.urb_queue.lock();

            if let Some(got) = queue.free_urbs.pop_back() {
                self.urbs[got].refs.store(1, Ordering::Relaxed);

                // SAFETY: We just allocated this URB. We should be the only user.
                unsafe {
                    self.urbs[got].fill_buffer(action)?;
                }
                return Poll::Ready(Ok(UrbRef(self, got)));
            }

            queue.wakers.push(ctx.waker().clone());
            Poll::Pending
        })
        .await
    }

    /// Cancel any transactions on a Urb.
    fn cancel_urb_by_id(&self, id: usize) {
        if let Some(stubs) = &self.stubs {
            let _ = stubs.discard_urb(self.file.as_raw_fd(), self.urbs[id].urb.get());
        } else {
            unsafe {
                let _ = ioctl!(self.file.as_raw_fd(), USBDEVFS_DISCARDURB, self.urbs[id].urb.get());
            }
        }
    }

    /// Free a Urb back into the pool.
    fn free_urb_by_id(&self, id: usize) {
        let mut queue = self.urb_queue.lock();
        queue.free_urbs.push_front(id);
        for waker in queue.wakers.drain(..) {
            waker.wake();
        }
    }

    /// Submit a Urb transaction.
    ///
    /// This method is async *and* returns a future. The semantics are as follows:
    ///
    /// * After first await: URB is allocated and submitted to the kernel. Data
    /// may not have come back yet. Buffer may not yet be written.
    /// * After second await: Kernel has finished with URB. Data has been
    /// written back to the buffer if applicable.
    async fn submit_urb<'a>(
        self: &'a Arc<Self>,
        address: u8,
        ty: u8,
        buffer_action: BufferAction<'_, 'a>,
        zero_packet: ZeroPacket,
    ) -> Result<impl Future<Output = Result<usize>> + use<'a>> {
        let urb = self.alloc_urb(&buffer_action).await?;
        let out = if let BufferAction::CopyOut(o) = buffer_action { Some(o) } else { None };

        {
            // SAFETY: The allocation semantics should guarantee we're the only holder of this Urb,
            // and that nobody else has handed it to the kernel. We have scoped the reference so it
            // will drop before we hand this pointer to the kernel below, and we will leak a pointer
            // to Self so our pool of Urbs will not be deallocated.
            let urb_inner = unsafe { &mut *urb.urb.get() };

            urb_inner.type_ = ty;
            urb_inner.endpoint = address;
            urb_inner.flags =
                if zero_packet == ZeroPacket::Send && (address & USB_ENDPOINT_DIR_MASK) == 0 {
                    USBDEVFS_URB_ZERO_PACKET
                } else {
                    0
                };
            urb_inner.status = -1;

            let got = urb.refs.fetch_add(1, Ordering::Relaxed);
            debug_assert!(got == 1);
        }

        let res = if let Some(stubs) = self.stubs.as_ref() {
            stubs.submit(self.file.as_raw_fd(), urb.urb.get())
        } else {
            // SAFETY: The pointer is held by the kernel until released by USBDEVFS_REAPURB later.
            // The explanation above explains why we know that pointer will last.
            unsafe { ioctl!(self.file.as_raw_fd(), USBDEVFS_SUBMITURB, urb.urb.get()) }
        };

        if let Err(e) = res {
            // Revert reference count so dropping `urb` can return it to free_urbs.
            urb.refs.fetch_sub(1, Ordering::Release);
            return Err(e.into());
        }

        // Leak self so that our Urbs will not be free'd prematurely.
        std::mem::forget(Arc::clone(self));
        self.pending_urbs.fetch_add(1, Ordering::Release);
        self.reaper_thread.thread().unpark();

        Ok(async move {
            poll_fn(|ctx| {
                urb.waker.register(ctx.waker());
                if urb.refs.load(Ordering::Acquire) != 1 { Poll::Pending } else { Poll::Ready(()) }
            })
            .await;

            // SAFETY: As above. We should have the Urb back from the kernel and it is ours alone by
            // allocation.
            let (status, actual_length) = unsafe {
                let urb_linux = &*urb.urb.get();
                (urb_linux.status, urb_linux.actual_length as usize)
            };

            if let Some(buf) = out {
                // SAFETY: As above.
                unsafe {
                    buf[..actual_length].copy_from_slice(&(&(*urb.buf.get()))[..actual_length]);
                }
            }

            if status == 0 {
                Ok(actual_length as usize)
            } else {
                Err(std::io::Error::from_raw_os_error(-status).into())
            }
        })
    }

    /// Attempt to allocate a Urb from the pool synchronously without waiting.
    fn try_alloc_urb_owned(
        self: &Arc<Self>,
        action: &BufferAction<'_, '_>,
    ) -> Result<Option<OwnedUrbRef>> {
        let got = {
            let mut queue = self.urb_queue.lock();
            queue.free_urbs.pop_back()
        };

        if let Some(got) = got {
            self.urbs[got].refs.store(1, Ordering::Relaxed);
            // Construct OwnedUrbRef before fill_buffer so that if fill_buffer fails,
            // the URB is safely returned to the free pool on drop rather than leaked.
            let urb = OwnedUrbRef(Arc::clone(self), got);

            // SAFETY: This URB was just retrieved from the free pool, ensuring its buffer is not
            // in use or in flight. Setting `refs` to 1 guarantees we are the sole user.
            unsafe {
                urb.fill_buffer(action)?;
            }
            return Ok(Some(urb));
        }

        Ok(None)
    }

    /// Submit a previously allocated Urb transaction.
    fn submit_allocated_urb_owned(
        self: &Arc<Self>,
        urb: OwnedUrbRef,
        address: u8,
        ty: u8,
        zero_packet: ZeroPacket,
    ) -> Result<impl Future<Output = Result<usize>> + Send + 'static> {
        {
            // SAFETY: As the sole holder of `OwnedUrbRef` (refs == 1), no other thread or kernel
            // access is occurring. Scoping `urb_inner` ensures the mutable reference drops before
            // the raw pointer is submitted to the kernel below.
            let urb_inner = unsafe { &mut *urb.urb.get() };

            urb_inner.type_ = ty;
            urb_inner.endpoint = address;
            urb_inner.flags =
                if zero_packet == ZeroPacket::Send && (address & USB_ENDPOINT_DIR_MASK) == 0 {
                    USBDEVFS_URB_ZERO_PACKET
                } else {
                    0
                };
            urb_inner.status = -1;

            let got = urb.refs.fetch_add(1, Ordering::Relaxed);
            debug_assert!(got == 1);
        }

        let res = if let Some(stubs) = self.stubs.as_ref() {
            stubs.submit(self.file.as_raw_fd(), urb.urb.get())
        } else {
            // SAFETY: The pointer is held by the kernel until released by USBDEVFS_REAPURB later.
            // The explanation above explains why we know that pointer will last.
            unsafe { ioctl!(self.file.as_raw_fd(), USBDEVFS_SUBMITURB, urb.urb.get()) }
        };

        if let Err(e) = res {
            // Revert reference count so dropping `urb` can return it to free_urbs.
            urb.refs.fetch_sub(1, Ordering::Release);
            return Err(e.into());
        }

        // Leak self so that our Urbs will not be free'd prematurely.
        std::mem::forget(Arc::clone(self));
        self.pending_urbs.fetch_add(1, Ordering::Release);
        self.reaper_thread.thread().unpark();

        Ok(async move {
            poll_fn(|ctx| {
                urb.waker.register(ctx.waker());
                if urb.refs.load(Ordering::Acquire) != 1 { Poll::Pending } else { Poll::Ready(()) }
            })
            .await;

            // SAFETY: The poll_fn above waited until the reaper thread observed kernel completion
            // and decremented refs back to 1. The kernel no longer accesses this URB, so reading
            // the status and actual_length fields is sound.
            let (status, actual_length) = unsafe {
                let urb_linux = &*urb.urb.get();
                (urb_linux.status, urb_linux.actual_length as usize)
            };

            if status == 0 {
                Ok(actual_length as usize)
            } else {
                Err(std::io::Error::from_raw_os_error(-status).into())
            }
        })
    }
}

/// Represents a single open USB interface.
pub struct Interface {
    /// Internal interface state.
    inner: Arc<InterfaceInner>,

    /// Descriptor for the Interface
    descriptor: InterfaceDescriptor,
}

impl Drop for Interface {
    fn drop(&mut self) -> () {
        let fd = self.inner.file.as_raw_fd();
        let mut iface = usbdevfs_setinterface {
            interface: self.descriptor.id as u32,
            altsetting: self.descriptor.alternate as u32,
        };
        if let Some(stubs) = self.inner.stubs.as_ref() {
            let _ = stubs.release_interface(fd, &mut iface.interface as *mut libc::c_uint);
        } else {
            // SAFETY: These ioctls will only reference this memory for their own runtime, during which
            // they will block this thread, preserving scope. Their arguments have been checked to match
            // the documentation.
            unsafe {
                // Cannot return error. Just swallow the result
                let _ = ioctl!(
                    fd,
                    USBDEVFS_RELEASEINTERFACE,
                    &mut iface.interface as *mut libc::c_uint
                );
            }
        }
        // Wake up the reaper thread so it can observe the Interface has
        // been dropped, allowing it to exit and release the Weak<InterfaceInner>
        // and also drop the File
        self.inner.reaper_thread.thread().unpark();
    }
}

impl Interface {
    /// Create an interface object. The `file` should be the opened block device file, and the
    /// `descriptor` should be the descriptor read out of the descriptor data. `stubs` is used for
    /// testing and should be `None` outside of unit tests.
    fn new(
        file: File,
        descriptor: InterfaceDescriptor,
        stubs: Option<Arc<dyn IoctlStub>>,
        urb_pool_size: usize,
    ) -> Result<Self> {
        let fd = file.as_raw_fd();

        let mut iface = usbdevfs_setinterface {
            interface: descriptor.id as u32,
            altsetting: descriptor.alternate as u32,
        };

        if let Some(stubs) = stubs.as_ref() {
            stubs.claim_interface(fd, &mut iface.interface as *mut libc::c_uint)?;
            if iface.altsetting != 0 {
                stubs.set_interface(fd, &mut iface as *mut usbdevfs_setinterface)?;
            }
        } else {
            // SAFETY: These ioctls will only reference this memory for their own runtime, during which
            // they will block this thread, preserving scope. Their arguments have been checked to match
            // the documentation.
            unsafe {
                ioctl!(fd, USBDEVFS_CLAIMINTERFACE, &mut iface.interface as *mut libc::c_uint)?;
                if iface.altsetting != 0 {
                    ioctl!(fd, USBDEVFS_SETINTERFACE, &mut iface as *mut usbdevfs_setinterface)?;
                }
            }
        }

        let urbs = Pin::new(
            std::iter::from_fn(|| Some(Urb::new())).take(urb_pool_size).collect::<Box<[_]>>(),
        );
        let urb_queue = Mutex::new(UrbQueue::new(&*urbs));
        let pending_urbs = AtomicU8::new(0);
        let (sender, receiver) = std::sync::mpsc::sync_channel(0);

        let reaper_thread_stubs = stubs.clone();
        let reaper_thread = thread::spawn(move || {
            let inner: std::sync::Weak<InterfaceInner> =
                receiver.recv().expect("Never got inner value!");

            while let Some(inner) = inner.upgrade() {
                if inner.pending_urbs.fetch_sub(1, Ordering::Acquire) == 0 {
                    inner.pending_urbs.fetch_add(1, Ordering::Release);
                    thread::park();
                    continue;
                }

                let mut out_ptr = std::ptr::null_mut::<usbdevfs_urb>();

                let fd = inner.file.as_raw_fd();

                // SAFETY: Before Urbs are submitted we leak a pointer to inner to protect their
                // allocations. This reclaims that leaked pointer. Because we checked `pending_urbs`
                // we know such a pointer existed. This is also how we know `fd` won't be closed
                // (and potentially reopened given how Unix file descriptors work) before the ioctl.
                unsafe { Arc::decrement_strong_count(&*inner) };

                // SAFETY: This should block until it no longer holds referencees to the data
                // we're passing it.
                let err = if let Some(stubs) = reaper_thread_stubs.as_ref() {
                    stubs.reap_urb(fd, &mut out_ptr)
                } else {
                    unsafe { ioctl!(fd, USBDEVFS_REAPURB, &mut out_ptr) }
                };

                if err.is_err() {
                    // TODO: Log something?
                    continue;
                }

                let Some((id, urb)) = inner
                    .urbs
                    .iter()
                    .enumerate()
                    .find(|(_, urb)| std::ptr::eq(urb.urb.get(), out_ptr))
                else {
                    panic!("Reap'd URB we did not sow!");
                };

                let count = urb.refs.fetch_sub(1, Ordering::AcqRel);
                urb.waker.wake();
                if count == 1 {
                    inner.free_urb_by_id(id);
                }
            }
        });

        let inner =
            Arc::new(InterfaceInner { file, urb_queue, urbs, pending_urbs, reaper_thread, stubs });
        sender.send(Arc::downgrade(&inner)).expect("Reaper thread disappeared immediately!");

        Ok(Interface { inner, descriptor })
    }

    /// Iterate all the endpoints available for this device.
    pub fn endpoints(&self) -> impl std::iter::Iterator<Item = Endpoint> + '_ {
        self.descriptor.endpoints.iter().cloned().map(|descriptor| match descriptor.ty {
            EndpointType::Bulk => {
                if descriptor.direction() == EndpointDirection::In {
                    Endpoint::BulkIn(BulkInEndpoint { inner: Arc::clone(&self.inner), descriptor })
                } else {
                    Endpoint::BulkOut(BulkOutEndpoint {
                        inner: Arc::clone(&self.inner),
                        descriptor,
                    })
                }
            }
            EndpointType::Isochronous => Endpoint::Isochronous(IsochronousEndpoint {
                inner: Arc::clone(&self.inner),
                descriptor,
            }),
            EndpointType::Interrupt => Endpoint::Interrupt(InterruptEndpoint {
                inner: Arc::clone(&self.inner),
                descriptor,
            }),
            EndpointType::Control => {
                Endpoint::Control(ControlEndpoint { inner: Arc::clone(&self.inner), descriptor })
            }
        })
    }
}

/// A bulk USB in endpoint. This is a live endpoint that can be read from.
pub struct BulkInEndpoint {
    /// Internal interface state.
    inner: Arc<InterfaceInner>,

    /// Descriptor for this endpoint.
    descriptor: EndpointDescriptor,
}

impl BulkInEndpoint {
    pub async fn read(&self, buf: &mut [u8]) -> Result<usize> {
        let fut = self
            .inner
            .submit_urb(
                self.descriptor.address,
                USBDEVFS_URB_TYPE_BULK as u8,
                BufferAction::CopyOut(buf),
                ZeroPacket::DoNotSend,
            )
            .await?;

        fut.await
    }

    /// Stream data from this endpoint. This will be considerably more efficient
    /// than reading single transactions with `read`.
    ///
    /// `buffer_size` is the maximum transaction the reader should prepare for.
    /// A transaction larger than this will cause an error.
    ///
    /// `buffer_count` is how many buffers we should offer to the kernel at once
    /// for receiving transactions. Larger is faster but increases memory
    /// consumpton.
    pub fn to_stream(
        self,
        buffer_size: usize,
        buffer_count: usize,
    ) -> impl futures::Stream<Item = Result<Vec<u8>>> {
        let mut workers = Vec::with_capacity(buffer_count);
        let out = Arc::new(Mutex::new(VecDeque::with_capacity(buffer_count)));
        // We assign each worker a number, and the order of the numbers
        // indicates the order the URBs were submitted in. We then enforce that
        // we deliver the results of the reads to the user in that order.
        //
        // It might be possible to prove this is unnecessary given the order we
        // poll in and the semantics of Rust futures but it would be brittle to
        // rely on those things for correctness.
        let sequence_counter = Arc::new(AsyncMutex::new(0usize));
        let mut next_expected = 0usize;
        let this = Arc::new(self);
        futures::stream::poll_fn(move |ctx| {
            if workers.is_empty() {
                for _ in 0..buffer_count {
                    let this = Arc::clone(&this);
                    let sequence_counter = Arc::clone(&sequence_counter);
                    let out = Arc::clone(&out);
                    workers.push(Box::pin(async move {
                        let mut buf = vec![0u8; buffer_size];
                        loop {
                            let (fut, sequence_number) = {
                                let mut sequence_counter = sequence_counter.lock().await;
                                let fut = this
                                    .inner
                                    .submit_urb(
                                        this.descriptor.address,
                                        USBDEVFS_URB_TYPE_BULK as u8,
                                        BufferAction::CopyOut(&mut buf),
                                        ZeroPacket::DoNotSend,
                                    )
                                    .await?;
                                let sequence_number = *sequence_counter;
                                *sequence_counter = sequence_counter.wrapping_add(1);
                                (fut, sequence_number)
                            };

                            let size = fut.await?;

                            out.lock().push_back((buf[..size].to_vec(), sequence_number));
                        }
                    }));
                }
            }

            for worker in &mut workers {
                let res: Poll<Result<std::convert::Infallible>> = worker.as_mut().poll(ctx);
                let Poll::Pending = res?;
            }

            let got = {
                let mut out = out.lock();
                let idx = out.iter().enumerate().find(|x| x.1.1 == next_expected).map(|x| x.0);

                idx.and_then(|idx| out.remove(idx))
            };

            if let Some(got) = got {
                next_expected = next_expected.wrapping_add(1);
                Poll::Ready(Some(Ok(got.0)))
            } else {
                // One of the workers *must* have registered a waker, so we can
                // just return pending.
                Poll::Pending
            }
        })
    }
}

/// A bulk USB out endpoint. This is a live endpoint that can be written to.
pub struct BulkOutEndpoint {
    /// Internal interface state.
    inner: Arc<InterfaceInner>,

    /// Descriptor for this endpoint.
    descriptor: EndpointDescriptor,
}

impl BulkOutEndpoint {
    /// Write data to this endpoint.
    pub async fn write(&self, buf: &[u8], zero_packet: ZeroPacket) -> Result<()> {
        self.write_defer_wait(buf, zero_packet).await?.await
    }

    /// Submit a write request but don't wait for the response right away. This
    /// function is async *and* returns a future, so the full return value is
    /// produced by awaiting *twice*. The first await submits the request to the
    /// USB stack, the second waits for acknowledgement that the request has
    /// been sent.
    pub async fn write_defer_wait<'s>(
        &'s self,
        buf: &[u8],
        zero_packet: ZeroPacket,
    ) -> Result<impl Future<Output = Result<()>> + use<'s>> {
        let fut = self
            .inner
            .submit_urb(
                self.descriptor.address,
                USBDEVFS_URB_TYPE_BULK as u8,
                BufferAction::CopyIn(buf),
                zero_packet,
            )
            .await?;
        let len = buf.len();

        Ok(async move {
            fut.await.and_then(|x| if x == len { Ok(()) } else { Err(Error::ShortWrite(len, x)) })
        })
    }

    /// Attempt to submit a write request immediately using an available URB from the pool.
    /// Returns `Ok(None)` if no URB is currently free in the pool.
    ///
    /// Copies `buf` directly into the allocated URB and submits it to the kernel.
    /// The returned future completes when the kernel reaps the URB.
    pub fn try_write_defer_wait(
        &self,
        buf: &[u8],
        zero_packet: ZeroPacket,
    ) -> Result<Option<impl Future<Output = Result<()>> + Send + 'static>> {
        let Some(urb) = self.inner.try_alloc_urb_owned(&BufferAction::CopyIn(buf))? else {
            return Ok(None);
        };
        let len = buf.len();
        let fut = self.inner.submit_allocated_urb_owned(
            urb,
            self.descriptor.address,
            USBDEVFS_URB_TYPE_BULK as u8,
            zero_packet,
        )?;
        Ok(Some(async move {
            fut.await.and_then(|x| if x == len { Ok(()) } else { Err(Error::ShortWrite(len, x)) })
        }))
    }
}

#[allow(unused)]
pub struct IsochronousEndpoint {
    /// Internal interface state.
    inner: Arc<InterfaceInner>,

    /// Descriptor for this endpoint.
    descriptor: EndpointDescriptor,
}

#[allow(unused)]
pub struct InterruptEndpoint {
    /// Internal interface state.
    inner: Arc<InterfaceInner>,

    /// Descriptor for this endpoint.
    descriptor: EndpointDescriptor,
}

#[allow(unused)]
pub struct ControlEndpoint {
    /// Internal interface state.
    inner: Arc<InterfaceInner>,

    /// Descriptor for this endpoint.
    descriptor: EndpointDescriptor,
}

impl InterfaceDescriptor {
    /// Turn a raw ch9 interface descriptor into a more rusty [`InterfaceDescriptor`]
    fn from_ch9(descriptor: &usb_interface_descriptor) -> Self {
        InterfaceDescriptor {
            id: descriptor.bInterfaceNumber,
            class: descriptor.bInterfaceClass,
            subclass: descriptor.bInterfaceSubClass,
            protocol: descriptor.bInterfaceProtocol,
            alternate: descriptor.bAlternateSetting,
            endpoints: Vec::new(),
        }
    }

    /// Add an endpoint to this descriptor's information given the raw ch9 endpoint descriptor.
    fn add_endpoint(&mut self, endpoint: &usb_endpoint_descriptor) {
        let ty = endpoint.bmAttributes as u32 & USB_ENDPOINT_XFERTYPE_MASK;
        let ty = match ty {
            USB_ENDPOINT_XFER_BULK => EndpointType::Bulk,
            USB_ENDPOINT_XFER_INT => EndpointType::Interrupt,
            USB_ENDPOINT_XFER_ISOC => EndpointType::Isochronous,
            USB_ENDPOINT_XFER_CONTROL => EndpointType::Control,
            _ => unreachable!("All bit patterns should be covered!"),
        };

        let address = endpoint.bEndpointAddress;

        self.endpoints.push(EndpointDescriptor { ty, address })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::USB_ENDPOINT_DIR_MASK;
    use crate::bulk_interface::BulkInterface;
    use futures::StreamExt;
    use futures::io::AsyncWriteExt;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, AtomicUsize};
    use std::sync::mpsc::{Receiver, Sender, channel};

    #[derive(Clone)]
    enum EndpointBuffer {
        Data(VecDeque<Box<[u8]>>),
        WaitingReaders(VecDeque<*mut usbdevfs_urb>),
    }

    impl Default for EndpointBuffer {
        fn default() -> Self {
            EndpointBuffer::Data(VecDeque::new())
        }
    }

    fn write_from_target_with_urb(
        data: &[u8],
        urb_ptr: *mut usbdevfs_urb,
        reap_sender: &Sender<*mut usbdevfs_urb>,
    ) -> bool {
        // SAFETY: The crate under test should never pass us an invalid pointer.
        let urb = unsafe { urb_ptr.as_mut().unwrap() };

        let status = if usize::try_from(urb.buffer_length).unwrap() < data.len() {
            -libc::E2BIG
        } else {
            // SAFETY: The crate under test should never pass us an invalid pointer.
            unsafe {
                std::slice::from_raw_parts_mut(urb.buffer.cast::<u8>(), data.len())
                    .copy_from_slice(&data);
            }

            urb.actual_length = data.len().try_into().unwrap();
            0
        };
        urb.status = status;
        reap_sender.send(urb_ptr).unwrap();
        status == 0
    }

    struct FakeDev {
        descriptor: InterfaceDescriptor,
        reap_receiver: Receiver<*mut usbdevfs_urb>,
        reap_sender: Sender<*mut usbdevfs_urb>,
        claimed: AtomicU8,
        released: AtomicU8,
        set: AtomicU8,
        endpoint_buffers: Mutex<HashMap<u8, EndpointBuffer>>,
        max_submit_size: AtomicUsize,
        max_in_flight: AtomicUsize,
        current_in_flight: AtomicUsize,
        reap_delay_ms: AtomicU64,
    }

    impl FakeDev {
        fn times_claimed(&self) -> u8 {
            self.claimed.load(Ordering::Relaxed)
        }
        fn times_released(&self) -> u8 {
            self.released.load(Ordering::Relaxed)
        }
        fn times_interface_set(&self) -> u8 {
            self.set.load(Ordering::Relaxed)
        }
        fn endpoint_read_from_target(&self, address: u8) -> Option<Box<[u8]>> {
            let mut buffers = self.endpoint_buffers.lock();
            buffers.get_mut(&address).and_then(|x| {
                let EndpointBuffer::Data(x) = x else {
                    panic!("Target read from buffer with host reads waiting");
                };
                x.pop_back()
            })
        }
        fn endpoint_read_all_from_target(&self, address: u8) -> Vec<u8> {
            let mut buffers = self.endpoint_buffers.lock();
            let Some(buffer) = buffers.get_mut(&address) else {
                return Vec::new();
            };
            let EndpointBuffer::Data(data) = buffer else {
                panic!("Target read from buffer with host reads waiting");
            };
            let mut result = Vec::new();
            while let Some(chunk) = data.pop_front() {
                result.extend_from_slice(&chunk);
            }
            result
        }
        fn endpoint_write_from_target(&self, address: u8, data: &[u8]) {
            let mut buffers = self.endpoint_buffers.lock();
            let buffer = buffers.entry(address).or_default();
            let queue = match buffer {
                EndpointBuffer::Data(data) => data,
                EndpointBuffer::WaitingReaders(readers) => {
                    while let Some(urb_ptr) = readers.pop_front() {
                        self.current_in_flight.fetch_add(1, Ordering::Relaxed);
                        if write_from_target_with_urb(data, urb_ptr, &self.reap_sender) {
                            return;
                        }
                    }
                    *buffer = EndpointBuffer::Data(VecDeque::new());
                    let EndpointBuffer::Data(got) = buffer else {
                        unreachable!();
                    };
                    got
                }
            };

            queue.push_back(Box::from(data));
        }
    }

    struct FakeUSBEnv {
        fake_devs: Mutex<HashMap<RawFd, Arc<FakeDev>>>,
    }

    impl FakeUSBEnv {
        fn new_fake_dev_file(&self, descriptor: InterfaceDescriptor) -> File {
            let ret = tempfile::tempfile().unwrap();
            let (reap_sender, reap_receiver) = channel();
            let fake_dev = FakeDev {
                descriptor,
                reap_sender,
                reap_receiver,
                claimed: AtomicU8::new(0),
                released: AtomicU8::new(0),
                set: AtomicU8::new(0),
                endpoint_buffers: Mutex::new(HashMap::new()),
                max_submit_size: AtomicUsize::new(usize::MAX),
                max_in_flight: AtomicUsize::new(usize::MAX),
                current_in_flight: AtomicUsize::new(0),
                reap_delay_ms: AtomicU64::new(0),
            };
            self.fake_devs.lock().insert(ret.as_raw_fd(), Arc::new(fake_dev));
            ret
        }

        fn new_fake_dev_with_pool_size(
            self: &Arc<Self>,
            descriptor: InterfaceDescriptor,
            pool_size: usize,
        ) -> Interface {
            Interface::new(
                self.new_fake_dev_file(descriptor.clone()),
                descriptor,
                Some(Arc::clone(self) as Arc<dyn IoctlStub>),
                pool_size,
            )
            .unwrap()
        }

        fn new_fake_dev(self: &Arc<Self>, descriptor: InterfaceDescriptor) -> Interface {
            self.new_fake_dev_with_pool_size(descriptor, 32)
        }

        fn new() -> Arc<Self> {
            Arc::new(FakeUSBEnv { fake_devs: Mutex::new(HashMap::new()) })
        }

        fn get_dev(&self, fd: RawFd) -> Result<Arc<FakeDev>, std::io::Error> {
            self.fake_devs
                .lock()
                .get(&fd)
                .map(Arc::clone)
                .ok_or_else(|| std::io::Error::from_raw_os_error(libc::ENODEV))
        }
    }

    // SAFETY: It's just complaining about the urb pointers. It's fine for the same reasons it's
    // always fine.
    unsafe impl Send for FakeUSBEnv {}
    unsafe impl Sync for FakeUSBEnv {}

    impl IoctlStub for FakeUSBEnv {
        fn claim_interface(&self, fd: RawFd, iface: *mut u32) -> Result<(), std::io::Error> {
            // SAFETY: The crate under test should never pass us an invalid pointer.
            let iface = unsafe { *iface };
            let dev = self.get_dev(fd)?;
            if dev.descriptor.id as u32 != iface {
                return Err(std::io::Error::from_raw_os_error(libc::ENODEV));
            }
            let _ = dev.claimed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn release_interface(&self, fd: RawFd, iface: *mut u32) -> Result<(), std::io::Error> {
            // SAFETY: The crate under test should never pass us an invalid pointer.
            let iface = unsafe { *iface };
            let dev = self.get_dev(fd)?;
            if dev.descriptor.id as u32 != iface {
                return Err(std::io::Error::from_raw_os_error(libc::ENODEV));
            }
            let _ = dev.released.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn reap_urb(
            &self,
            fd: std::os::fd::RawFd,
            urb_ptr: *mut *mut usbdevfs_urb,
        ) -> Result<(), std::io::Error> {
            let dev = self.get_dev(fd)?;
            let urb_ptr = unsafe { urb_ptr.as_mut().unwrap() };

            let delay_ms = dev.reap_delay_ms.load(Ordering::Relaxed);
            if delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            }

            *urb_ptr = dev.reap_receiver.recv().unwrap();
            dev.current_in_flight.fetch_sub(1, Ordering::Relaxed);
            Ok(())
        }

        fn set_interface(
            &self,
            fd: std::os::fd::RawFd,
            set_struct: *mut usbdevfs_setinterface,
        ) -> Result<(), std::io::Error> {
            // SAFETY: The crate under test should never pass us an invalid pointer.
            let set_struct = unsafe { *set_struct };
            let dev = self.get_dev(fd)?;
            if dev.descriptor.id as u32 != set_struct.interface {
                return Err(std::io::Error::from_raw_os_error(libc::ENODEV));
            }
            if dev.descriptor.alternate as u32 != set_struct.altsetting {
                return Err(std::io::Error::from_raw_os_error(libc::ENODEV));
            }
            let _ = dev.set.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }

        fn submit(
            &self,
            fd: std::os::fd::RawFd,
            urb: *mut usbdevfs_urb,
        ) -> Result<(), std::io::Error> {
            let urb_ptr = urb;
            // SAFETY: The crate under test should never pass us an invalid pointer.
            let urb = unsafe { urb.as_mut().unwrap() };
            let dev = self.get_dev(fd)?;

            let max_size = dev.max_submit_size.load(Ordering::Relaxed);
            if (urb.buffer_length as usize) > max_size {
                return Err(std::io::Error::from_raw_os_error(libc::ENOMEM));
            }

            let max_in_flight = dev.max_in_flight.load(Ordering::Relaxed);
            if dev.current_in_flight.load(Ordering::Relaxed) >= max_in_flight {
                return Err(std::io::Error::from_raw_os_error(libc::ENOMEM));
            }

            assert_eq!(urb.type_, USBDEVFS_URB_TYPE_BULK as u8);
            let endpoint =
                dev.descriptor.endpoints.iter().find(|x| x.address == urb.endpoint).unwrap();

            let mut buffers = dev.endpoint_buffers.lock();
            let buffer = buffers.entry(endpoint.address).or_default();

            match endpoint.direction() {
                EndpointDirection::Out => {
                    assert!(urb.flags == 0 || urb.flags == USBDEVFS_URB_ZERO_PACKET);
                    // SAFETY: The crate under test should never pass us an invalid pointer.
                    let data = unsafe {
                        std::slice::from_raw_parts(
                            urb.buffer.cast::<u8>(),
                            urb.buffer_length.try_into().unwrap(),
                        )
                    };
                    let EndpointBuffer::Data(buffer) = buffer else {
                        panic!("Readers waiting on Out endpoint")
                    };
                    buffer.push_back(Box::from(data));
                    urb.actual_length = urb.buffer_length;
                    urb.status = 0;
                    dev.current_in_flight.fetch_add(1, Ordering::Relaxed);
                    dev.reap_sender.send(urb_ptr).unwrap();
                }
                EndpointDirection::In => {
                    assert_eq!(urb.flags, 0);
                    let waiting_readers = match buffer {
                        EndpointBuffer::WaitingReaders(readers) => readers,
                        EndpointBuffer::Data(queue) => {
                            if let Some(data) = queue.front() {
                                if write_from_target_with_urb(data, urb_ptr, &dev.reap_sender) {
                                    let _ = queue.pop_front();
                                }
                                dev.current_in_flight.fetch_add(1, Ordering::Relaxed);
                                return Ok(());
                            }
                            *buffer = EndpointBuffer::WaitingReaders(VecDeque::new());
                            let EndpointBuffer::WaitingReaders(got) = buffer else {
                                unreachable!()
                            };
                            got
                        }
                    };
                    waiting_readers.push_back(urb_ptr);
                }
            }
            Ok(())
        }

        fn discard_urb(&self, fd: RawFd, urb: *mut usbdevfs_urb) -> Result<(), std::io::Error> {
            // SAFETY: The crate under test should never pass us an invalid pointer.
            let urb = unsafe { urb.as_mut().unwrap() };
            let dev = self.get_dev(fd)?;

            assert_eq!(urb.type_, USBDEVFS_URB_TYPE_BULK as u8);
            let endpoint =
                dev.descriptor.endpoints.iter().find(|x| x.address == urb.endpoint).unwrap();

            let mut buffers = dev.endpoint_buffers.lock();
            let buffer = buffers.entry(endpoint.address).or_default();

            match endpoint.direction() {
                EndpointDirection::Out => Err(std::io::Error::from_raw_os_error(libc::ENOENT)),
                EndpointDirection::In => {
                    let EndpointBuffer::WaitingReaders(readers) = buffer else {
                        return Err(std::io::Error::from_raw_os_error(libc::ENOENT));
                    };
                    let mut found = false;

                    readers.retain(|x| {
                        if std::ptr::addr_eq(*x, urb) {
                            found = true;
                            false
                        } else {
                            true
                        }
                    });

                    if found {
                        Ok(())
                    } else {
                        Err(std::io::Error::from_raw_os_error(libc::ENOENT))
                    }
                }
            }
        }
    }

    #[fuchsia::test]
    async fn simple_bulk() {
        let env = FakeUSBEnv::new();
        let descriptor = InterfaceDescriptor {
            id: 0,
            class: 0xff,
            subclass: 0x42,
            protocol: 22,
            alternate: 64,
            endpoints: vec![
                EndpointDescriptor { ty: EndpointType::Bulk, address: 0 | USB_ENDPOINT_DIR_MASK },
                EndpointDescriptor { ty: EndpointType::Bulk, address: 1 },
            ],
        };
        let iface = env.new_fake_dev(descriptor);
        let fd = iface.inner.file.as_raw_fd();
        assert_eq!(1, env.get_dev(fd).unwrap().times_claimed());
        assert_eq!(1, env.get_dev(fd).unwrap().times_interface_set());

        let mut eps = iface.endpoints().collect::<Vec<_>>();
        assert_eq!(2, eps.len());
        let a = eps.pop().unwrap();
        let b = eps.pop().unwrap();

        let (i, o) = match (a, b) {
            (Endpoint::BulkIn(a), Endpoint::BulkOut(b)) => (a, b),
            (Endpoint::BulkOut(a), Endpoint::BulkIn(b)) => (b, a),
            _ => panic!("Wrong endpoint types!"),
        };

        assert_eq!(0 | USB_ENDPOINT_DIR_MASK, i.descriptor.address);
        assert_eq!(1, o.descriptor.address);

        env.get_dev(fd).unwrap().endpoint_write_from_target(0 | USB_ENDPOINT_DIR_MASK, b"Wango!");
        let mut buf = [0u8; 8];
        let len = i.read(&mut buf).await.unwrap();
        assert_eq!(b"Wango!".len(), len);
        assert_eq!(b"Wango!", &buf[..len]);

        o.write(b"Bango!", ZeroPacket::Send).await.unwrap();
        let data = env.get_dev(fd).unwrap().endpoint_read_from_target(1).unwrap();
        assert_eq!(b"Bango!", &*data);

        o.write(b"Bongo!", ZeroPacket::DoNotSend).await.unwrap();
        let data = env.get_dev(fd).unwrap().endpoint_read_from_target(1).unwrap();
        assert_eq!(b"Bongo!", &*data);

        assert_eq!(1, env.get_dev(fd).unwrap().times_claimed());
        assert_eq!(1, env.get_dev(fd).unwrap().times_interface_set());

        drop(iface);
        assert_eq!(1, env.get_dev(fd).unwrap().times_released());
    }

    #[fuchsia::test]
    async fn stumpy_read() {
        let env = FakeUSBEnv::new();
        let descriptor = InterfaceDescriptor {
            id: 0,
            class: 0xff,
            subclass: 0x42,
            protocol: 22,
            alternate: 64,
            endpoints: vec![EndpointDescriptor {
                ty: EndpointType::Bulk,
                address: 0 | USB_ENDPOINT_DIR_MASK,
            }],
        };
        let iface = env.new_fake_dev(descriptor);
        let fd = iface.inner.file.as_raw_fd();
        assert_eq!(1, env.get_dev(fd).unwrap().times_claimed());
        assert_eq!(1, env.get_dev(fd).unwrap().times_interface_set());

        let mut eps = iface.endpoints().collect::<Vec<_>>();
        assert_eq!(1, eps.len());

        let i = match eps.pop().unwrap() {
            Endpoint::BulkIn(a) => a,
            _ => panic!("Wrong endpoint type!"),
        };

        assert_eq!(0 | USB_ENDPOINT_DIR_MASK, i.descriptor.address);

        env.get_dev(fd).unwrap().endpoint_write_from_target(0 | USB_ENDPOINT_DIR_MASK, b"Wango!");

        let mut buf = [0u8; 2];
        let err = i.read(&mut buf).await.unwrap_err();
        let err = match err {
            Error::IOError(err) => err,
            other => panic!("Unexpected error! {other:?}"),
        };
        assert_eq!(Some(libc::E2BIG), err.raw_os_error());

        let mut buf = [0u8; 8];
        let len = i.read(&mut buf).await.unwrap();
        assert_eq!(b"Wango!".len(), len);
        assert_eq!(b"Wango!", &buf[..len]);

        assert_eq!(1, env.get_dev(fd).unwrap().times_claimed());
        assert_eq!(1, env.get_dev(fd).unwrap().times_interface_set());

        drop(iface);
        assert_eq!(1, env.get_dev(fd).unwrap().times_released());
    }

    #[fuchsia::test]
    async fn stream_bulk() {
        let env = FakeUSBEnv::new();
        let descriptor = InterfaceDescriptor {
            id: 0,
            class: 0xff,
            subclass: 0x42,
            protocol: 22,
            alternate: 64,
            endpoints: vec![
                EndpointDescriptor { ty: EndpointType::Bulk, address: 0 | USB_ENDPOINT_DIR_MASK },
                EndpointDescriptor { ty: EndpointType::Bulk, address: 1 },
            ],
        };
        let iface = env.new_fake_dev(descriptor);
        let fd = iface.inner.file.as_raw_fd();
        assert_eq!(1, env.get_dev(fd).unwrap().times_claimed());
        assert_eq!(1, env.get_dev(fd).unwrap().times_interface_set());

        let mut eps = iface.endpoints().collect::<Vec<_>>();
        assert_eq!(2, eps.len());
        let a = eps.pop().unwrap();
        let b = eps.pop().unwrap();

        let (i, o) = match (a, b) {
            (Endpoint::BulkIn(a), Endpoint::BulkOut(b)) => (a, b),
            (Endpoint::BulkOut(a), Endpoint::BulkIn(b)) => (b, a),
            _ => panic!("Wrong endpoint types!"),
        };

        assert_eq!(0 | USB_ENDPOINT_DIR_MASK, i.descriptor.address);
        assert_eq!(1, o.descriptor.address);

        let i = i.to_stream(8, 8);

        env.get_dev(fd).unwrap().endpoint_write_from_target(0 | USB_ENDPOINT_DIR_MASK, b"Wango!");
        env.get_dev(fd).unwrap().endpoint_write_from_target(0 | USB_ENDPOINT_DIR_MASK, b"Bango!");

        let results = i.take(2).collect::<Vec<_>>().await;
        let results = results.into_iter().collect::<Result<Vec<_>>>().unwrap();
        assert_eq!(2, results.len());
        assert_eq!(b"Wango!", results[0].as_slice());
        assert_eq!(b"Bango!", results[1].as_slice());

        assert_eq!(1, env.get_dev(fd).unwrap().times_claimed());
        assert_eq!(1, env.get_dev(fd).unwrap().times_interface_set());
    }

    #[fuchsia::test]
    async fn bulk_interface_drains_in_flight_on_enomem() {
        let env = FakeUSBEnv::new();
        let descriptor = InterfaceDescriptor {
            id: 0,
            class: 0xff,
            subclass: 0x42,
            protocol: 22,
            alternate: 64,
            endpoints: vec![
                EndpointDescriptor { ty: EndpointType::Bulk, address: 0 | USB_ENDPOINT_DIR_MASK },
                EndpointDescriptor { ty: EndpointType::Bulk, address: 1 },
            ],
        };
        let iface = env.new_fake_dev(descriptor);
        let fd = iface.inner.file.as_raw_fd();
        let dev = env.get_dev(fd).unwrap();

        // Simulate host kernel usbfs memory limit that can hold at most 3 in-flight URBs
        dev.max_in_flight.store(3, Ordering::Relaxed);
        dev.reap_delay_ms.store(5, Ordering::Relaxed);

        let mut bulk = BulkInterface::new(iface);

        // Payload of 6 * 512 KiB chunks (exceeding max_in_flight of 3)
        let payload: Vec<u8> = (0..6 * 512 * 1024).map(|i| (i % 241) as u8).collect();

        bulk.write_all(&payload)
            .await
            .expect("Write should succeed by throttling and draining in-flight URBs on ENOMEM");

        let received = dev.endpoint_read_all_from_target(1);
        assert_eq!(received.len(), payload.len());
        assert_eq!(received, payload);
    }

    #[fuchsia::test]
    async fn bulk_interface_fails_cleanly_on_fatal_enomem() {
        let env = FakeUSBEnv::new();
        let descriptor = InterfaceDescriptor {
            id: 0,
            class: 0xff,
            subclass: 0x42,
            protocol: 22,
            alternate: 64,
            endpoints: vec![EndpointDescriptor { ty: EndpointType::Bulk, address: 1 }],
        };
        let iface = env.new_fake_dev(descriptor);
        let fd = iface.inner.file.as_raw_fd();
        let dev = env.get_dev(fd).unwrap();

        // Simulate host kernel where even 0 in-flight URBs cannot be submitted
        dev.max_in_flight.store(0, Ordering::Relaxed);

        let mut bulk = BulkInterface::new(iface);
        let payload = vec![0x42u8; 1024];

        let err = bulk.write_all(&payload).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
        assert!(
            err.to_string().contains("Linux usbfs_memory_mb exhausted"),
            "Error message did not contain expected hint: {err}"
        );
    }

    #[fuchsia::test]
    async fn submit_urb_cleans_up_on_submission_error() {
        let env = FakeUSBEnv::new();
        let descriptor = InterfaceDescriptor {
            id: 0,
            class: 0xff,
            subclass: 0x42,
            protocol: 22,
            alternate: 64,
            endpoints: vec![EndpointDescriptor { ty: EndpointType::Bulk, address: 1 }],
        };
        let iface = env.new_fake_dev(descriptor);
        let fd = iface.inner.file.as_raw_fd();
        let dev = env.get_dev(fd).unwrap();

        let mut eps = iface.endpoints().collect::<Vec<_>>();
        let o = match eps.pop().unwrap() {
            Endpoint::BulkOut(o) => o,
            _ => panic!("Expected BulkOut endpoint"),
        };

        // Set max submit size to 10 bytes
        dev.max_submit_size.store(10, Ordering::Relaxed);

        // Attempt writing 20 bytes -> should fail with ENOMEM
        let err = o.write(b"01234567890123456789", ZeroPacket::DoNotSend).await.unwrap_err();
        match err {
            Error::IOError(e) => assert_eq!(e.raw_os_error(), Some(libc::ENOMEM)),
            other => panic!("Expected IOError(ENOMEM), got: {other:?}"),
        }

        // Verify pending_urbs is 0
        assert_eq!(iface.inner.pending_urbs.load(Ordering::Relaxed), 0);

        // Verify that the URB was returned to the pool and we can successfully write small data
        o.write(b"hello", ZeroPacket::DoNotSend)
            .await
            .expect("Subsequent valid write should succeed");
        assert_eq!(iface.inner.pending_urbs.load(Ordering::Relaxed), 0);
        assert_eq!(dev.endpoint_read_from_target(1).unwrap().as_ref(), b"hello");
    }

    #[fuchsia::test]
    async fn try_write_defer_wait_cleans_up_on_submission_error() {
        let env = FakeUSBEnv::new();
        let descriptor = InterfaceDescriptor {
            id: 0,
            class: 0xff,
            subclass: 0x42,
            protocol: 22,
            alternate: 64,
            endpoints: vec![EndpointDescriptor { ty: EndpointType::Bulk, address: 1 }],
        };
        let iface = env.new_fake_dev(descriptor);
        let fd = iface.inner.file.as_raw_fd();
        let dev = env.get_dev(fd).unwrap();

        let mut eps = iface.endpoints().collect::<Vec<_>>();
        let o = match eps.pop().unwrap() {
            Endpoint::BulkOut(o) => o,
            _ => panic!("Expected BulkOut endpoint"),
        };

        dev.max_submit_size.store(10, Ordering::Relaxed);

        // Attempt try_write_defer_wait with 20 bytes -> should fail with ENOMEM
        let err = match o.try_write_defer_wait(b"01234567890123456789", ZeroPacket::DoNotSend) {
            Err(e) => e,
            Ok(_) => panic!("Expected Err, got Ok"),
        };
        match err {
            Error::IOError(e) => assert_eq!(e.raw_os_error(), Some(libc::ENOMEM)),
            other => panic!("Expected IOError(ENOMEM), got: {other:?}"),
        }

        assert_eq!(iface.inner.pending_urbs.load(Ordering::Relaxed), 0);

        let wait_fut = o
            .try_write_defer_wait(b"hello", ZeroPacket::DoNotSend)
            .unwrap()
            .expect("URB should be available in pool");
        wait_fut.await.expect("Write should complete");
        assert_eq!(iface.inner.pending_urbs.load(Ordering::Relaxed), 0);
        assert_eq!(dev.endpoint_read_from_target(1).unwrap().as_ref(), b"hello");
    }
}
