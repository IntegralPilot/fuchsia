// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use ash::prelude::*;
use ash::{Device, Instance, RawPtr, vk};
use std::mem;

#[derive(Clone)]
pub struct BufferCollection {
    handle: vk::Device,
    fp: ash::fuchsia::buffer_collection::DeviceFn,
}

impl BufferCollection {
    pub fn new(instance: &Instance, device: &Device) -> Self {
        let handle = device.handle();
        let fp = ash::fuchsia::buffer_collection::DeviceFn::load(|name| unsafe {
            mem::transmute(instance.get_device_proc_addr(handle, name.as_ptr()))
        });
        Self { handle, fp }
    }

    /// <https://registry.khronos.org/vulkan/specs/1.3-extensions/man/html/vkCreateBufferCollectionFUCHSIA.html>
    pub unsafe fn create_buffer_collection(
        &self,
        create_info: &vk::BufferCollectionCreateInfoFUCHSIA<'_>,
        allocation_callbacks: Option<&vk::AllocationCallbacks<'_>>,
    ) -> VkResult<vk::BufferCollectionFUCHSIA> {
        let mut buffer_collection = unsafe { mem::zeroed() };
        unsafe {
            (self.fp.create_buffer_collection_fuchsia)(
                self.handle,
                create_info,
                allocation_callbacks.as_raw_ptr(),
                &mut buffer_collection,
            )
        }
        .result_with_success(buffer_collection)
    }

    /// <https://registry.khronos.org/vulkan/specs/1.3-extensions/man/html/vkSetBufferCollectionImageConstraintsFUCHSIA.html>
    pub unsafe fn set_buffer_collection_image_constraints(
        &self,
        collection: vk::BufferCollectionFUCHSIA,
        info: &vk::ImageConstraintsInfoFUCHSIA<'_>,
    ) -> VkResult<()> {
        unsafe {
            (self.fp.set_buffer_collection_image_constraints_fuchsia)(
                self.handle,
                collection,
                info as *const vk::ImageConstraintsInfoFUCHSIA<'_>,
            )
        }
        .result_with_success(())
    }

    /// <https://registry.khronos.org/vulkan/specs/1.3-extensions/man/html/vkSetBufferCollectionBufferConstraintsFUCHSIA.html>
    // TODO(https://fxbug.dev/42055924): remove this #[allow(dead_code)] when we subsequently upstream
    // this extension to ash.
    #[allow(dead_code)]
    pub unsafe fn set_buffer_collection_buffer_constraints(
        &self,
        collection: vk::BufferCollectionFUCHSIA,
        info: &vk::BufferConstraintsInfoFUCHSIA<'_>,
    ) -> VkResult<()> {
        unsafe {
            (self.fp.set_buffer_collection_buffer_constraints_fuchsia)(
                self.handle,
                collection,
                info as *const vk::BufferConstraintsInfoFUCHSIA<'_>,
            )
        }
        .result_with_success(())
    }

    /// <https://registry.khronos.org/vulkan/specs/1.3-extensions/man/html/vkDestroyBufferCollectionFUCHSIA.html>
    #[allow(dead_code)]
    pub unsafe fn destroy_buffer_collection(
        &self,
        collection: vk::BufferCollectionFUCHSIA,
        allocation_callbacks: Option<&vk::AllocationCallbacks<'_>>,
    ) {
        unsafe {
            (self.fp.destroy_buffer_collection_fuchsia)(
                self.handle,
                collection,
                allocation_callbacks.as_raw_ptr(),
            );
        }
    }

    /// <https://registry.khronos.org/vulkan/specs/1.3-extensions/man/html/vkGetBufferCollectionPropertiesFUCHSIA.html>
    pub unsafe fn get_buffer_collection_properties(
        &self,
        collection: vk::BufferCollectionFUCHSIA,
    ) -> VkResult<vk::BufferCollectionPropertiesFUCHSIA<'_>> {
        let mut props = vk::BufferCollectionPropertiesFUCHSIA::default();
        unsafe {
            (self.fp.get_buffer_collection_properties_fuchsia)(self.handle, collection, &mut props)
        }
        .result_with_success(props)
    }
}
