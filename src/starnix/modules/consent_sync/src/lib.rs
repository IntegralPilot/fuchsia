// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_settings as fsettings;
use starnix_core::device::DeviceOps;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::FileOps;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps};
use starnix_logging::log_error;

use starnix_uapi::device_id::DeviceId;
use starnix_uapi::error;
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use zx;

#[derive(Clone)]
struct ConsentSyncHandle(Arc<ConsentSyncFileBackend>);

impl DeviceOps for ConsentSyncHandle {
    fn open(
        &self,
        _current_task: &CurrentTask,
        _device_id: DeviceId,
        _node: &starnix_core::vfs::NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(BytesFile::new(self.clone())))
    }
}

struct ConsentSyncFileBackend {
    consent: AtomicBool,
    privacy_proxy: fsettings::PrivacySynchronousProxy,
}

impl BytesFileOps for ConsentSyncHandle {
    fn write(&self, current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        self.0.write(current_task, data)
    }

    fn read(&self, current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        self.0.read(current_task)
    }
}

impl BytesFileOps for ConsentSyncFileBackend {
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let content_str = String::from_utf8_lossy(&data);
        let trimmed_content = content_str.trim();

        let granted = match trimmed_content {
            "0" => false,
            "1" => true,
            _ => {
                log_error!(
                    "ConsentSync: Invalid value written: {:?}. Must be '0' or '1'.",
                    trimmed_content
                );
                return error!(EINVAL);
            }
        };

        let settings = fsettings::PrivacySettings {
            user_data_sharing_consent: Some(granted),
            ..Default::default()
        };

        match self.privacy_proxy.set(&settings, zx::MonotonicInstant::INFINITE) {
            Ok(Ok(())) => {
                self.consent.store(granted, Ordering::Relaxed);
                Ok(())
            }
            Ok(Err(e)) => {
                log_error!("ConsentSync: fuchsia.settings.Privacy.Set application error: {:?}", e);
                error!(EIO)
            }
            Err(e) => {
                log_error!(
                    "ConsentSync: FIDL call to fuchsia.settings.Privacy.Set failed: {:?}",
                    e
                );
                error!(EIO)
            }
        }
    }

    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let val = if self.consent.load(Ordering::Relaxed) { "1\n" } else { "0\n" };
        Ok(val.as_bytes().into())
    }
}

pub fn init(kernel: &Arc<Kernel>) {
    let registry = &kernel.device_registry;

    let privacy_proxy = fsettings::PrivacySynchronousProxy::new(
        kernel
            .connect_to_protocol_at_container_svc::<fsettings::PrivacyMarker>()
            .expect("Connected to privacy service")
            .into_channel(),
    );

    let file_backend =
        Arc::new(ConsentSyncFileBackend { consent: AtomicBool::new(false), privacy_proxy });

    let device = ConsentSyncHandle(file_backend);

    registry
        .register_dyn_device(kernel, "consent".into(), registry.objects.starnix_class(), device)
        .expect("can register consent device");
}
