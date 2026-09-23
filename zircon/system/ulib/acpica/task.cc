// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <mutex>

#include <acpica/acpi.h>
#include <fbl/intrusive_double_list.h>

/* Structures used for implementing AcpiOsExecute and
 * AcpiOsWaitEventsComplete */
struct AcpiOsTaskCtx : public fbl::DoublyLinkedListable<std::unique_ptr<AcpiOsTaskCtx>> {
  ACPI_OSD_EXEC_CALLBACK func;
  void* ctx;
};

/* Thread function for implementing AcpiOsExecute */
static int AcpiOsExecuteTask(void* arg);

/* Data used for implementing AcpiOsExecute and
 * AcpiOsWaitEventsComplete */
static struct {
  thrd_t thread;
  std::condition_variable cond;
  std::condition_variable idle_cond;
  std::mutex lock;
  bool shutdown = false;
  bool idle = true;

  fbl::DoublyLinkedList<std::unique_ptr<AcpiOsTaskCtx>> tasks;
} os_execute_state;

static ACPI_STATUS thrd_status_to_acpi_status(int status) {
  switch (status) {
    case thrd_success:
      return AE_OK;
    case thrd_nomem:
      return AE_NO_MEMORY;
    case thrd_timedout:
      return AE_TIME;
    default:
      return AE_ERROR;
  }
}

ACPI_STATUS AcpiTaskThreadStart() {
  return thrd_status_to_acpi_status(
      thrd_create_with_name(&os_execute_state.thread, AcpiOsExecuteTask, nullptr, "acpi_os_task"));
}

ACPI_STATUS AcpiTaskThreadTerminate() {
  {
    std::lock_guard lock(os_execute_state.lock);
    os_execute_state.shutdown = true;
  }
  os_execute_state.cond.notify_all();
  thrd_join(os_execute_state.thread, nullptr);
  return AE_OK;
}

static int AcpiOsExecuteTask(void* arg) {
  while (1) {
    std::unique_ptr<AcpiOsTaskCtx> task;

    {
      std::unique_lock lock(os_execute_state.lock);
      while ((task = os_execute_state.tasks.pop_front()) == nullptr) {
        os_execute_state.idle = true;
        // If anything is waiting for the queue to empty, notify it.
        os_execute_state.idle_cond.notify_one();

        // If we're waiting to shutdown, do it now that there's no more work
        if (os_execute_state.shutdown) {
          return 0;
        }

        os_execute_state.cond.wait(lock);
      }
      os_execute_state.idle = false;
    }

    task->func(task->ctx);
  }

  return 0;
}

/**
 * @brief Schedule a procedure for deferred execution.
 *
 * @param Type Type of the callback function.
 * @param Function Address of the procedure to execute.
 * @param Context A context value to be passed to the called procedure.
 *
 * @return AE_OK The procedure was successfully queued for execution.
 * @return AE_BAD_PARAMETER The Type is invalid or the Function pointer
 *         is NULL.
 */
ACPI_STATUS AcpiOsExecute(ACPI_EXECUTE_TYPE Type, ACPI_OSD_EXEC_CALLBACK Function, void* Context) {
  if (Function == NULL) {
    return AE_BAD_PARAMETER;
  }

  switch (Type) {
    case OSL_GLOBAL_LOCK_HANDLER:
    case OSL_NOTIFY_HANDLER:
    case OSL_GPE_HANDLER:
    case OSL_DEBUGGER_MAIN_THREAD:
    case OSL_DEBUGGER_EXEC_THREAD:
    case OSL_EC_POLL_HANDLER:
    case OSL_EC_BURST_HANDLER:
      break;
    default:
      return AE_BAD_PARAMETER;
  }

  fbl::AllocChecker ac;
  std::unique_ptr<AcpiOsTaskCtx> task(new (&ac) AcpiOsTaskCtx);
  if (!ac.check()) {
    return AE_NO_MEMORY;
  }
  task->func = Function;
  task->ctx = Context;

  {
    std::lock_guard lock(os_execute_state.lock);
    os_execute_state.tasks.push_back(std::move(task));
  }
  os_execute_state.cond.notify_one();

  return AE_OK;
}

/**
 * @brief Wait for completion of asynchronous events.
 *
 * This function blocks until all asynchronous events initiated by
 * AcpiOsExecute have completed.
 */
void AcpiOsWaitEventsComplete(void) {
  std::unique_lock lock(os_execute_state.lock);
  while (!os_execute_state.idle) {
    os_execute_state.idle_cond.wait(lock);
  }
}
