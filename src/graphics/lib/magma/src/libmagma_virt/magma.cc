// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <lib/magma/magma.h>
#include <lib/magma/util/short_macros.h>
#include <pthread.h>
#include <sys/mman.h>

#include "src/graphics/lib/magma/include/virtio/virtio_magma.h"
#include "src/graphics/lib/magma/src/libmagma_virt/virtmagma_util.h"

static pthread_once_t gOnceFlag = PTHREAD_ONCE_INIT;
static int gDefaultFd;

// Most magma interfaces get their file descriptor from a wrapped parameter (device, connection,
// etc) (or initially from the file descriptor "handle" in magma_device_import), but some interfaces
// don't have any such parameter; for those, we open the default device, and never close it.
static int get_default_fd() {
  pthread_once(&gOnceFlag, [] { gDefaultFd = open("/dev/magma0", O_RDWR); });
  assert(gDefaultFd >= 0);
  return gDefaultFd;
}

magma_status_t magma_poll(magma_poll_item_t* items, uint32_t count, uint64_t timeout_ns) {
#if VIRTMAGMA_DEBUG
  printf("%s\n", __PRETTY_FUNCTION__);
#endif

  if (count == 0)
    return MAGMA_STATUS_OK;

  magma_poll_item_t unwrapped_items[count];
  int32_t file_descriptor = -1;

  for (uint32_t i = 0; i < count; ++i) {
    unwrapped_items[i] = items[i];

    switch (items[i].type) {
      case MAGMA_POLL_TYPE_SEMAPHORE: {
        auto semaphore_wrapped = virtmagma_semaphore_t::Get(items[i].semaphore);
        unwrapped_items[i].semaphore = semaphore_wrapped->Object();

        if (file_descriptor < 0) {
          auto semaphore0_parent_wrapped = virtmagma_connection_t::Get(semaphore_wrapped->Parent());
          file_descriptor = semaphore0_parent_wrapped->Parent().fd();
        }
        break;
      }

      case MAGMA_POLL_TYPE_HANDLE: {
        // Handles are not wrapped.
        break;
      }
    }
  }

  // Ensure host compatibility with 32bit guest
  static_assert(sizeof(magma_poll_item_t) % 8 == 0);

  virtio_magma_poll_ctrl_t request{};
  virtio_magma_poll_resp_t response{};
  request.hdr.type = VIRTIO_MAGMA_CMD_POLL;
  request.items = reinterpret_cast<uintptr_t>(&unwrapped_items[0]);
  // Send byte count so kernel knows how much memory to copy
  request.count = count * sizeof(magma_poll_item_t);
  request.timeout_ns = timeout_ns;

  if (file_descriptor == -1) {
    file_descriptor = get_default_fd();
  }

  if (!virtmagma_send_command(file_descriptor, &request, sizeof(request), &response,
                              sizeof(response)))
    return MAGMA_STATUS_INTERNAL_ERROR;
  if (response.hdr.type != VIRTIO_MAGMA_RESP_POLL)
    return MAGMA_STATUS_INTERNAL_ERROR;

  magma_status_t result_return = static_cast<decltype(result_return)>(response.result_return);
  if (result_return != MAGMA_STATUS_OK)
    return result_return;

  // Update the results
  for (uint32_t i = 0; i < count; i++) {
    items[i].result = unwrapped_items[i].result;
  }

  return MAGMA_STATUS_OK;
}

magma_status_t magma_connection_execute_command(magma_connection_t connection, uint32_t context_id,
                                                struct magma_command_descriptor* descriptor) {
#if VIRTMAGMA_DEBUG
  printf("%s\n", __PRETTY_FUNCTION__);
#endif

  struct WireDescriptor {
    uint32_t resource_count;
    uint32_t command_buffer_count;
    uint32_t wait_semaphore_count;
    uint32_t signal_semaphore_count;
    uint64_t flags;
  };

  WireDescriptor wire_descriptor = {.resource_count = descriptor->resource_count,
                                    .command_buffer_count = descriptor->command_buffer_count,
                                    .wait_semaphore_count = descriptor->wait_semaphore_count,
                                    .signal_semaphore_count = descriptor->signal_semaphore_count,
                                    .flags = descriptor->flags};

  // Ensure host compatibility with 32bit guest
  static_assert(sizeof(magma_exec_command_buffer) % 8 == 0);
  static_assert(sizeof(magma_exec_resource) % 8 == 0);

  virtmagma_command_descriptor vdesc = {
      .descriptor_size = sizeof(wire_descriptor),
      .descriptor = reinterpret_cast<uintptr_t>(&wire_descriptor),
      .resource_size = sizeof(magma_exec_resource) * descriptor->resource_count,
      .resources = reinterpret_cast<uintptr_t>(descriptor->resources),
      .command_buffer_size = sizeof(magma_exec_command_buffer) * descriptor->command_buffer_count,
      .command_buffers = reinterpret_cast<uintptr_t>(descriptor->command_buffers),
      .semaphore_size = sizeof(uint64_t) *
                        (descriptor->wait_semaphore_count + descriptor->signal_semaphore_count),
      .semaphores = reinterpret_cast<uintptr_t>(descriptor->semaphore_ids)};

  virtio_magma_connection_execute_command_ctrl request{
      .hdr = {.type = VIRTIO_MAGMA_CMD_CONNECTION_EXECUTE_COMMAND}};
  virtio_magma_connection_execute_command_resp response{};

  auto connection_wrapped = virtmagma_connection_t::Get(connection);
  request.connection = reinterpret_cast<uint64_t>(connection_wrapped->Object());
  request.context_id = context_id;
  request.descriptor = reinterpret_cast<uintptr_t>(&vdesc);

  int32_t file_descriptor = connection_wrapped->Parent().fd();

  if (!virtmagma_send_command(file_descriptor, &request, sizeof(request), &response,
                              sizeof(response))) {
    assert(false);
  }
  return static_cast<magma_status_t>(response.result_return);
}

magma_status_t magma_connection_execute_inline_commands(
    magma_connection_t connection, uint32_t context_id, uint64_t command_count,
    magma_inline_command_buffer_t* command_buffers) {
#if VIRTMAGMA_DEBUG
  printf("%s\n", __PRETTY_FUNCTION__);
#endif

  virtmagma_command_descriptor commands[command_count];
  for (uint64_t i = 0; i < command_count; i++) {
    commands[i] = {
        .descriptor_size = 0,
        .descriptor = 0,
        .resource_size = 0,
        .resources = 0,
        .command_buffer_size = command_buffers[i].size,
        .command_buffers = reinterpret_cast<uintptr_t>(command_buffers[i].data),
        .semaphore_size = sizeof(uint64_t) * command_buffers[i].semaphore_count,
        .semaphores = reinterpret_cast<uintptr_t>(command_buffers[i].semaphore_ids),
    };
  }

  virtio_magma_connection_execute_inline_commands_ctrl request{
      .hdr = {.type = VIRTIO_MAGMA_CMD_CONNECTION_EXECUTE_INLINE_COMMANDS}};
  virtio_magma_connection_execute_inline_commands_resp response{};

  auto connection_wrapped = virtmagma_connection_t::Get(connection);
  request.connection = reinterpret_cast<uint64_t>(connection_wrapped->Object());
  request.context_id = context_id;
  request.command_count = command_count;
  request.command_buffers = reinterpret_cast<uintptr_t>(commands);

  int32_t file_descriptor = connection_wrapped->Parent().fd();

  if (!virtmagma_send_command(file_descriptor, &request, sizeof(request), &response,
                              sizeof(response))) {
    assert(false);
  }
  return static_cast<magma_status_t>(response.result_return);
}

magma_status_t magma_buffer_get_info(magma_buffer_t buffer, magma_buffer_info_t* info_out) {
  auto buffer_wrapped = virtmagma_buffer_t::Get(buffer);

  virtio_magma_buffer_get_info_ctrl_t request{
      .hdr = {.type = VIRTIO_MAGMA_CMD_BUFFER_GET_INFO},
      .buffer = reinterpret_cast<uint64_t>(buffer_wrapped->Object()),
      .info_out = reinterpret_cast<uintptr_t>(info_out),
  };
  virtio_magma_buffer_get_info_resp_t response{};

  auto connection_wrapped = virtmagma_connection_t::Get(buffer_wrapped->Parent());

  if (!virtmagma_send_command(connection_wrapped->Parent().fd(), &request, sizeof(request),
                              &response, sizeof(response))) {
    assert(false);
    return DRET(MAGMA_STATUS_INTERNAL_ERROR);
  }
  if (response.hdr.type != VIRTIO_MAGMA_RESP_BUFFER_GET_INFO)
    return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Wrong response header: %u", response.hdr.type);

  return static_cast<magma_status_t>(response.result_return);
}

magma_status_t magma_buffer_set_name(magma_buffer_t buffer, const char* name) {
  auto buffer_wrapped = virtmagma_buffer_t::Get(buffer);

  struct virtmagma_buffer_set_name_wrapper wrapper = {
      .name_address = reinterpret_cast<uintptr_t>(name),
      .name_size = strlen(name) + 1,  // include null terminate byte
  };

  virtio_magma_buffer_set_name_ctrl_t request{
      .hdr = {.type = VIRTIO_MAGMA_CMD_BUFFER_SET_NAME},
      .buffer = reinterpret_cast<uint64_t>(buffer_wrapped->Object()),
      .name = reinterpret_cast<uintptr_t>(&wrapper),
  };
  virtio_magma_buffer_set_name_resp_t response{};

  auto connection_wrapped = virtmagma_connection_t::Get(buffer_wrapped->Parent());

  if (!virtmagma_send_command(connection_wrapped->Parent().fd(), &request, sizeof(request),
                              &response, sizeof(response))) {
    assert(false);
    return DRET(MAGMA_STATUS_INTERNAL_ERROR);
  }
  if (response.hdr.type != VIRTIO_MAGMA_RESP_BUFFER_SET_NAME)
    return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Wrong response header: %u", response.hdr.type);

  return static_cast<magma_status_t>(response.result_return);
}

magma_status_t magma_initialize_tracing(magma_handle_t channel) {
  int fd = channel;
  close(fd);
  return MAGMA_STATUS_OK;
}

magma_status_t magma_initialize_logging(magma_handle_t channel) {
  int fd = channel;
  close(fd);
  return MAGMA_STATUS_OK;
}

magma_status_t magma_enumerate_devices(const char*, magma_handle_t, uint32_t* device_count_out,
                                       uint32_t device_path_size, char* device_paths_out) {
  if (*device_count_out < 1) {
    return MAGMA_STATUS_INVALID_ARGS;
  }

  constexpr char kPath[] = "/dev/magma0";
  constexpr size_t kPathLength = sizeof(kPath);  // includes the null terminator
  static_assert(kPathLength == 12);

  if (device_path_size < kPathLength) {
    return MAGMA_STATUS_INVALID_ARGS;
  }

  *device_count_out = 1;
  strncpy(device_paths_out, kPath, kPathLength);

  return MAGMA_STATUS_OK;
}

magma_status_t magma_device_query(magma_device_t device, uint64_t id,
                                  magma_handle_t* result_buffer_out, uint64_t* result_out) {
  auto device_wrapped = virtmagma_device_t::Get(device);
  device = device_wrapped->Object();

  int32_t file_descriptor = device_wrapped->Parent().fd();

  virtio_magma_device_query_ctrl_t request{
      .hdr = {.type = VIRTIO_MAGMA_CMD_DEVICE_QUERY}, .device = device, .id = id};
  virtio_magma_device_query_resp_t response{};

  bool success = virtmagma_send_command(file_descriptor, &request, sizeof(request), &response,
                                        sizeof(response));
  if (!success)
    return DRET(MAGMA_STATUS_INTERNAL_ERROR);

  magma_status_t status = static_cast<magma_status_t>(response.result_return);
  if (status != MAGMA_STATUS_OK)
    return DRET(status);

  if (response.hdr.type != VIRTIO_MAGMA_RESP_DEVICE_QUERY)
    return DRET(MAGMA_STATUS_INTERNAL_ERROR);

  int fd = static_cast<int>(response.result_buffer_out);
  if (fd < 0) {
    if (!result_out)
      return DRET(MAGMA_STATUS_INVALID_ARGS);

    *result_out = response.result_out;

    if (result_buffer_out)
      *result_buffer_out = -1;

    return MAGMA_STATUS_OK;
  }

  // If a buffer is present, it's an error to ignore it.
  if (!result_buffer_out) {
    close(fd);
    return DRET(MAGMA_STATUS_INVALID_ARGS);
  }

  *result_buffer_out = fd;
  return MAGMA_STATUS_OK;
}
