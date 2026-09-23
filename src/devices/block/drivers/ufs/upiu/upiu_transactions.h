// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_UPIU_TRANSACTIONS_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_UPIU_TRANSACTIONS_H_

#include <endian.h>

#include <optional>
#include <utility>

#include <fbl/algorithm.h>
#include <hwreg/bitfields.h>

#include "src/devices/block/drivers/ufs/transfer_command_descriptor.h"
#include "src/devices/block/drivers/ufs/transfer_request_descriptor.h"

namespace ufs {

// UPIU requires 64-bit alignment.
constexpr uint16_t kUpiuAlignment = 8;

// for test
namespace ufs_mock_device {
class TransferRequestProcessor;
class QueryRequestProcessor;
class ScsiCommandProcessor;
class UfsMockDevice;
}  // namespace ufs_mock_device

// UFS Specification Version 3.1, section 10.5 "UPIU Transactions".
enum UpiuTransactionCodes : uint8_t {
  kNopOut = 0x00,
  kCommand = 0x01,
  kDataOut = 0x02,
  kTaskManagementRequest = 0x04,
  kQueryRequest = 0x16,
  kNopIn = 0x20,
  kResponse = 0x21,
  kDataIn = 0x22,
  kTaskManagementResponse = 0x24,
  kReadyToTransfer = 0x31,
  kQueryResponse = 0x36,
  kRejectUpiu = 0x3f,
};

enum UpiuCommandSetType {
  kScsi = 0x00,
  kUfsSpecificCommandSet = 0x01,
  kVendorSpecificSet = 0x08,
};

enum UpiuHeaderResponseCode {
  kTargetSuccess = 0x00,
  kTargetFailure = 0x01,
};

// UFS Specification Version 3.1, section 10.6.2 "Basic Header Format".
struct UpiuHeader {
  // dword 0
  uint8_t trans_type = 0;
  uint8_t flags = 0;
  uint8_t lun = 0;
  uint8_t task_tag = 0;

  // dword 1
  uint8_t cmd_set_type_and_initiator_id = 0;
  uint8_t function = 0;
  uint8_t response = UpiuHeaderResponseCode::kTargetSuccess;
  uint8_t status = 0;

  // dword 2
  uint8_t ehs_length = 0;
  uint8_t device_infomation = 0;
  uint16_t data_segment_length = 0;  // (Big-endian)

  DEF_SUBFIELD(trans_type, 5, 0, trans_code);
  DEF_SUBBIT(trans_type, 6, dd);
  DEF_SUBBIT(trans_type, 7, hd);

  DEF_SUBBIT(flags, 2, cp);
  DEF_SUBFIELD(flags, 1, 0, task_attribute);

  DEF_ENUM_SUBFIELD(cmd_set_type_and_initiator_id, UpiuCommandSetType, 3, 0, command_set_type);
  DEF_SUBFIELD(cmd_set_type_and_initiator_id, 7, 4, initiator_id);

  DEF_SUBBIT(device_infomation, 0, event_alert);
} __PACKED;
static_assert(sizeof(UpiuHeader) == 12, "UpiuHeader struct must be 12 bytes");

class AbstractUpiu {
 public:
  // TODO(https://fxbug.dev/42075643): Make |AbstractUpiu| to template class for removing the |Data|
  // struct within |AbstractUpiu|. Currently each derived class must have a |UpiuHeader| as the
  // first member in its |Data| struct.
  struct Data {
    UpiuHeader header;
  } __PACKED;

  explicit AbstractUpiu() = default;
  explicit AbstractUpiu(void* data) : data_ptr_(data) {}

  virtual ~AbstractUpiu() = default;

  AbstractUpiu(AbstractUpiu&& other) noexcept
      : data_ptr_(std::exchange(other.data_ptr_, nullptr)) {}

  AbstractUpiu& operator=(AbstractUpiu&& other) noexcept {
    if (this != &other) {
      data_ptr_ = std::exchange(other.data_ptr_, nullptr);
    }
    return *this;
  }

  AbstractUpiu(const AbstractUpiu&) = default;
  AbstractUpiu& operator=(const AbstractUpiu&) = default;

  // Used to read or write the request descriptor in the UPIU.
  template <typename T = void>
  T* GetData() {
    return reinterpret_cast<T*>(data_ptr_);
  }
  UpiuHeader& GetHeader() {
    ZX_ASSERT(GetData());
    return GetData<Data>()->header;
  }

 protected:
  void SetData(void* data) { data_ptr_ = data; }

 private:
  void* data_ptr_ = nullptr;
};

template <typename RequestData, typename ResponseData>
class AbstractRequestUpiu : public AbstractUpiu {
 public:
  explicit AbstractRequestUpiu() {
    data_ = std::make_unique<RequestData>();
    SetData(data_.get());
  }

  explicit AbstractRequestUpiu(const RequestData& data) {
    data_ = std::make_unique<RequestData>(data);
    SetData(data_.get());
  }

  AbstractRequestUpiu(AbstractRequestUpiu&& other) noexcept
      : AbstractUpiu(std::move(other)), data_(std::move(other.data_)) {
    if (data_) {
      SetData(data_.get());
    }
  }

  AbstractRequestUpiu& operator=(AbstractRequestUpiu&& other) noexcept {
    if (this != &other) {
      AbstractUpiu::operator=(std::move(other));
      data_ = std::move(other.data_);
      if (data_) {
        SetData(data_.get());
      }
    }
    return *this;
  }

  AbstractRequestUpiu(const AbstractRequestUpiu&) = delete;
  AbstractRequestUpiu& operator=(const AbstractRequestUpiu&) = delete;

  ~AbstractRequestUpiu() override = default;

  // Get the direction of the data transfer to be written to the request descriptor. The
  // DataDirection determines whether the target device will read or write the system memory area
  // pointed to by the PRDT.
  virtual DataDirection GetDataDirection() const { return DataDirection::kNone; }

  // Get the offset that ResponseUpiu will be written to.
  static constexpr uint16_t GetResponseOffset() { return sizeof(RequestData); }

  // Get the length of the ResponseUpiu.
  static constexpr uint16_t GetResponseLength() { return sizeof(ResponseData); }

 private:
  std::unique_ptr<RequestData> data_ = nullptr;
};

using AbstractResponseUpiu = AbstractUpiu;

struct ResponseUpiuData {
  // dword 0 ~ 2
  UpiuHeader header;
  // dword 3
  uint32_t residual_transfer_count = 0;  // (Big-endian)

  // dword 4 ~ 6
  uint8_t reserved[16] = {0};

  // Sense Data
  uint16_t sense_data_len = 0;  // (Big-endian)
  uint8_t sense_data[18] = {0};

  // Add padding to align the kUpiuAlignment.
  uint8_t padding[4] = {0};

  DEF_SUBBIT(header.flags, 6, header_flags_o);
  DEF_SUBBIT(header.flags, 5, header_flags_u);
  DEF_SUBBIT(header.flags, 4, header_flags_d);
} __PACKED;
static_assert(sizeof(ResponseUpiuData) == 56, "ResponseUpiu struct, must be 56 bytes");
static_assert(sizeof(ResponseUpiuData) <= kMaxUtpTransferResponseSize,
              "ResponseUpiu must be kMaxUtpTransferResponseSize or less");
static_assert(sizeof(ResponseUpiuData) % kUpiuAlignment == 0, "UPIU requires 64-bit alignment");

// UFS Specification Version 3.1, section 10.7.2 "RESPONSE UPIU".
class ResponseUpiu : public AbstractResponseUpiu {
 public:
  explicit ResponseUpiu(void* data) {
    if (data) {
      response_data_ = std::make_unique<ResponseUpiuData>(*static_cast<ResponseUpiuData*>(data));
      SetData(response_data_.get());
    }
  }

  ResponseUpiu(ResponseUpiu&& other) noexcept : AbstractResponseUpiu(std::move(other)) {
    response_data_ = std::move(other.response_data_);
    if (response_data_) {
      SetData(response_data_.get());
    } else {
      SetData(nullptr);
    }
    other.SetData(nullptr);
  }

  ResponseUpiu& operator=(ResponseUpiu&& other) noexcept {
    if (this != &other) {
      AbstractResponseUpiu::operator=(std::move(other));
      response_data_ = std::move(other.response_data_);
      if (response_data_) {
        SetData(response_data_.get());
      } else {
        SetData(nullptr);
      }
      other.SetData(nullptr);
    }
    return *this;
  }

  ~ResponseUpiu() override = default;

  uint8_t* GetSenseData() { return GetData<ResponseUpiuData>()->sense_data; }

 private:
  std::unique_ptr<ResponseUpiuData> response_data_;

  // for test
  friend class ufs_mock_device::TransferRequestProcessor;
  friend class ufs_mock_device::ScsiCommandProcessor;
  friend class UfsTest;
};

struct CommandUpiuData {
  // dword 0 ~ 2
  UpiuHeader header;
  // dword 3
  uint32_t expected_data_transfer_length = 0;  // (Big-endian)

  // dword 4 ~ 7
  uint8_t cdb[16] = {0};

  DEF_SUBBIT(header.flags, 6, header_flags_r);
  DEF_SUBBIT(header.flags, 5, header_flags_w);
  DEF_SUBBIT(header.flags, 2, header_flags_cp);
  DEF_SUBFIELD(header.flags, 1, 0, header_flags_attr);
} __PACKED;
static_assert(sizeof(CommandUpiuData) == 32, "CommandUpiu struct must be 32 bytes");
static_assert(sizeof(CommandUpiuData) <= kMaxUtpTransferRequestSize,
              "CommandUpiu must be kMaxUtpTransferRequestSize or less");
static_assert(sizeof(CommandUpiuData) % kUpiuAlignment == 0, "UPIU requires 64-bit alignment");

// UFS Specification Version 3.1, section 10.7.1 "COMMAND UPIU".
class CommandUpiu : public AbstractRequestUpiu<CommandUpiuData, ResponseUpiuData> {
 public:
  explicit CommandUpiu() { GetHeader().set_trans_code(UpiuTransactionCodes::kCommand); }

  explicit CommandUpiu(UpiuCommandSetType command_set_type, DataDirection data_direction)
      : CommandUpiu() {
    GetHeader().set_command_set_type(command_set_type);

    if (data_direction == DataDirection::kDeviceToHost) {
      GetData<CommandUpiuData>()->set_header_flags_r(true);
    } else if (data_direction == DataDirection::kHostToDevice) {
      GetData<CommandUpiuData>()->set_header_flags_w(true);
    }
  }

  explicit CommandUpiu(const CommandUpiuData& data) : AbstractRequestUpiu(data) {}

  ~CommandUpiu() override = default;

  void SetExpectedDataTransferLength(uint32_t length) {
    GetData<CommandUpiuData>()->expected_data_transfer_length = htobe32(length);
  }

 private:
  // for test
  friend class ufs_mock_device::TransferRequestProcessor;
  friend class ufs_mock_device::ScsiCommandProcessor;
  friend class UfsTest;
};

enum class TaskManagementFunction : uint8_t {
  kAbortTask = 0x01,
  kAbortTaskSet = 0x02,
  kClearTaskSet = 0x04,
  kLogicalUnitReset = 0x08,
  kQueryTask = 0x80,
  kQueryTaskSet = 0x81,
};

enum class TaskManagementServiceResponse : uint8_t {
  kTaskManagementFunctionComplete = 0x00,
  kTaskManagementFunctionNotSupported = 0x04,
  kTaskManagementFunctionFailed = 0x05,
  kTaskManagementFunctionSucceeded = 0x08,
  kIncorrectLogicalUnitNumber = 0x09,
};

struct TaskManagementResponseUpiuData {
  // dword 0 ~ 2
  UpiuHeader header;
  // dword 3
  uint32_t output_param1 = 0;  // (Big-endian)
  // dword 4
  uint32_t output_param2 = 0;  // (Big-endian)
  // dword 5 ~ 7
  uint8_t reserved[12] = {0};
} __PACKED;
static_assert(sizeof(TaskManagementResponseUpiuData) == 32,
              "TaskManagementResponseUpiu struct must be 32 bytes");
static_assert(sizeof(TaskManagementResponseUpiuData) <= kMaxUtpTransferResponseSize,
              "TaskManagementResponseUpiu must be kMaxUtpTransferResponseSize or less");
static_assert(sizeof(TaskManagementResponseUpiuData) % kUpiuAlignment == 0,
              "UPIU requires 64-bit alignment");

// UFS Specification Version 3.1, section 10.7.7 "TASK MANAGEMENT RESPONSE UPIU".
class TaskManagementResponseUpiu : public AbstractResponseUpiu {
 public:
  explicit TaskManagementResponseUpiu(void* data) : AbstractResponseUpiu(data) {}

  ~TaskManagementResponseUpiu() override = default;
};

struct TaskManagementRequestUpiuData {
  // dword 0 ~ 2
  UpiuHeader header;
  // dword 3
  uint32_t input_param1 = 0;  // (Big-endian)
  // dword 4
  uint32_t input_param2 = 0;  // (Big-endian)
  // dword 5
  uint32_t input_param3 = 0;  // (Big-endian)
  // dword 6 ~ 7
  uint8_t reserved[8] = {0};
} __PACKED;
static_assert(sizeof(TaskManagementRequestUpiuData) == 32,
              "TaskManagementRequestUpiu struct must be 32 bytes");
static_assert(sizeof(TaskManagementRequestUpiuData) <= kMaxUtpTransferRequestSize,
              "TaskManagementRequestUpiu must be kMaxUtpTransferRequestSize or less");
static_assert(sizeof(TaskManagementRequestUpiuData) % kUpiuAlignment == 0,
              "UPIU requires 64-bit alignment");

// UFS Specification Version 3.1, section 10.7.6 "TASK MANAGEMENT REQUEST UPIU".
class TaskManagementRequestUpiu
    : public AbstractRequestUpiu<TaskManagementRequestUpiuData, TaskManagementResponseUpiuData> {
 public:
  explicit TaskManagementRequestUpiu(TaskManagementFunction function, uint32_t lun,
                                     uint32_t task_tag) {
    GetHeader().set_trans_code(UpiuTransactionCodes::kTaskManagementRequest);
    GetHeader().function = static_cast<uint8_t>(function);

    GetData<TaskManagementRequestUpiuData>()->input_param1 = lun;
    GetData<TaskManagementRequestUpiuData>()->input_param2 = task_tag;
  }

  ~TaskManagementRequestUpiu() override = default;
};

enum class QueryFunction {
  kStandardReadRequest = 0x01,
  kStandardWriteRequest = 0x81,
};

enum class QueryOpcode {
  kNop = 0,
  kReadDescriptor,
  kWriteDescriptor,
  kReadAttribute,
  kWriteAttribute,
  kReadFlag,
  kSetFlag,
  kClearFlag,
  kToggleFlag,
};

inline const char* QueryOpcodeToString(QueryOpcode opcode) {
  switch (opcode) {
    case QueryOpcode::kNop:
      return "Nop";
    case QueryOpcode::kReadDescriptor:
      return "Read Descriptor";
    case QueryOpcode::kWriteDescriptor:
      return "Write Descriptor";
    case QueryOpcode::kReadAttribute:
      return "Read Attribute";
    case QueryOpcode::kWriteAttribute:
      return "Write Attribute";
    case QueryOpcode::kReadFlag:
      return "Read Flag";
    case QueryOpcode::kSetFlag:
      return "Set Flag";
    case QueryOpcode::kClearFlag:
      return "Clear Flag";
    case QueryOpcode::kToggleFlag:
      return "Toggle Flag";
    default:
      return "(UNKNOWN)";
  }
}

enum class QueryResponseCode {
  kSuccess = 0x00,
  kParameterNotReadable = 0xf6,
  kParameterNotWriteable = 0xf7,
  kParameterAlreadyWritten = 0xf8,
  kInvalidLength = 0xf9,
  kInvalidValue = 0xfa,
  kInvalidSelector = 0xfb,
  kInvalidIndex = 0xfc,
  kInvalidIdn = 0xfd,
  kInvalidOpcode = 0xfe,
  kGeneralFailure = 0xff,
};

struct QueryResponseUpiuData {
  // dword 0 ~ 2
  UpiuHeader header;
  // dword 3
  uint8_t opcode = 0;
  uint8_t idn = 0;
  uint8_t index = 0;
  uint8_t selector = 0;

  // dword 4
  uint8_t reserved1[2] = {0};
  uint16_t length = 0;  // (Big-endian)

  // dword 5
  union {
    uint32_t value = 0;  // (Big-endian)
    struct {
      uint8_t reserved2[3];
      uint8_t flag_value;
    };
  } __PACKED;

  // dword 6
  uint8_t reserved3[4] = {0};

  // dword 7
  uint8_t reserved4[4] = {0};
  std::array<uint8_t, 256> command_data = {0};
} __PACKED;
static_assert(sizeof(QueryResponseUpiuData) == 288, "QueryResponseUpiu struct must be 288 bytes");
static_assert(sizeof(QueryResponseUpiuData) <= kMaxUtpTransferResponseSize,
              "QueryResponseUpiu must be kMaxUtpTransferResponseSize or less");
static_assert(sizeof(QueryResponseUpiuData) % kUpiuAlignment == 0,
              "UPIU requires 64-bit alignment");

// UFS Specification Version 3.1, section 10.7.9 "QUERY RESPONSE UPIU".
class QueryResponseUpiu : public AbstractResponseUpiu {
 public:
  explicit QueryResponseUpiu(void* data) {
    if (data) {
      response_data_ =
          std::make_unique<QueryResponseUpiuData>(*static_cast<QueryResponseUpiuData*>(data));
      SetData(response_data_.get());
    }
  }

  QueryResponseUpiu(QueryResponseUpiu&& other) noexcept : AbstractResponseUpiu(std::move(other)) {
    response_data_ = std::move(other.response_data_);
    if (response_data_) {
      SetData(response_data_.get());
    } else {
      SetData(nullptr);
    }
    other.SetData(nullptr);
  }

  QueryResponseUpiu& operator=(QueryResponseUpiu&& other) noexcept {
    if (this != &other) {
      AbstractResponseUpiu::operator=(std::move(other));
      response_data_ = std::move(other.response_data_);
      if (response_data_) {
        SetData(response_data_.get());
      } else {
        SetData(nullptr);
      }
      other.SetData(nullptr);
    }
    return *this;
  }

  ~QueryResponseUpiu() override = default;

  template <typename U>
  constexpr U& GetResponse() {
    static_assert(std::is_base_of<QueryResponseUpiu, U>::value);
    static_assert(sizeof(U) == sizeof(QueryResponseUpiu));
    return *reinterpret_cast<U*>(this);
  }

  // for test
  uint8_t GetOpcode() { return GetData<QueryResponseUpiuData>()->opcode; }
  uint8_t GetIdn() { return GetData<QueryResponseUpiuData>()->idn; }
  uint8_t GetIndex() { return GetData<QueryResponseUpiuData>()->index; }

 private:
  std::unique_ptr<QueryResponseUpiuData> response_data_;

  // for test
  friend class ufs_mock_device::TransferRequestProcessor;
  friend class ufs_mock_device::QueryRequestProcessor;
  friend class UfsTest;
};

struct QueryRequestUpiuData {
  // dword 0 ~ 2
  UpiuHeader header;
  // dword 3
  uint8_t opcode = 0;
  uint8_t idn = 0;
  uint8_t index = 0;
  uint8_t selector = 0;

  // dword 4
  uint8_t reserved1[2] = {0};
  uint16_t length = 0;  // (Big-endian)

  // dword 5
  uint32_t value = 0;  // (Big-endian)

  // dword 6
  uint8_t reserved2[4] = {0};

  // dword 7
  uint8_t reserved3[4] = {0};
  std::array<uint8_t, 256> command_data = {0};
} __PACKED;
static_assert(sizeof(QueryRequestUpiuData) == 288, "QueryRequestUpiu struct must be 288 bytes");
static_assert(sizeof(QueryRequestUpiuData) <= kMaxUtpTransferRequestSize,
              "QueryRequestUpiu must be kMaxUtpTransferRequestSize or less");
static_assert(sizeof(QueryRequestUpiuData) % kUpiuAlignment == 0, "UPIU requires 64-bit alignment");

// UFS Specification Version 3.1, section 10.7.8 "QUERY REQUEST UPIU".
class QueryRequestUpiu : public AbstractRequestUpiu<QueryRequestUpiuData, QueryResponseUpiuData> {
 public:
  explicit QueryRequestUpiu(QueryFunction query_function, QueryOpcode query_opcode, uint8_t type,
                            uint8_t index = 0) {
    GetHeader().set_trans_code(UpiuTransactionCodes::kQueryRequest);

    GetHeader().function = static_cast<uint8_t>(query_function);

    GetData<QueryRequestUpiuData>()->opcode = static_cast<uint8_t>(query_opcode);
    GetData<QueryRequestUpiuData>()->idn = type;
    GetData<QueryRequestUpiuData>()->index = index;
  }

  explicit QueryRequestUpiu(const QueryRequestUpiuData& data) : AbstractRequestUpiu(data) {}

  ~QueryRequestUpiu() override = default;

 private:
  // for test
  friend class ufs_mock_device::TransferRequestProcessor;
  friend class ufs_mock_device::QueryRequestProcessor;
  friend class UfsTest;
};

struct NopInUpiuData {
  // dword 0 ~ 2
  UpiuHeader header;
  // dword 3 ~ 7
  uint8_t reserved[20] = {0};
} __PACKED;
static_assert(sizeof(NopInUpiuData) == 32, "NopInUpiu struct must be 32 bytes");
static_assert(sizeof(NopInUpiuData) <= kMaxUtpTransferResponseSize,
              "NopInUpiu must be kMaxUtpTransferResponseSize or less");
static_assert(sizeof(NopInUpiuData) % kUpiuAlignment == 0, "UPIU requires 64-bit alignment");

// UFS Specification Version 3.1, section 10.7.12 "NOP IN UPIU".
class NopInUpiu : public AbstractResponseUpiu {
 public:
  explicit NopInUpiu(void* data) : AbstractResponseUpiu(data) {}

  ~NopInUpiu() override = default;

 private:
  // for test
  friend class ufs_mock_device::TransferRequestProcessor;
  friend class UfsTest;
};

struct NopOutUpiuData {
  // dword 0 ~ 2
  UpiuHeader header;
  // dword 3 ~ 7
  uint8_t reserved[20] = {0};
} __PACKED;
static_assert(sizeof(NopOutUpiuData) == 32, "NopOutUpiu struct must be 32 bytes");
static_assert(sizeof(NopOutUpiuData) <= kMaxUtpTransferRequestSize,
              "NopOutUpiu must be kMaxUtpTransferRequestSize or less");
static_assert(sizeof(NopOutUpiuData) % kUpiuAlignment == 0, "UPIU requires 64-bit alignment");

// UFS Specification Version 3.1, section 10.7.11 "NOP OUT UPIU".
class NopOutUpiu : public AbstractRequestUpiu<NopOutUpiuData, NopInUpiuData> {
 public:
  explicit NopOutUpiu() { GetHeader().set_trans_code(UpiuTransactionCodes::kNopOut); }

  ~NopOutUpiu() override = default;
};

// Used to avoid std::memset() on uncacheable memory.
inline void CustomMemSet(void* dest, int c, size_t count) {
  volatile uint8_t* d = static_cast<volatile uint8_t*>(dest);
  while (count--) {
    *d++ = static_cast<uint8_t>(c);
  }
}

// Used to avoid std::memcpy() on uncacheable memory.
inline void CustomMemCpy(void* dest, const void* src, size_t count) {
  volatile uint8_t* d = static_cast<volatile uint8_t*>(dest);
  const volatile uint8_t* s = static_cast<const volatile uint8_t*>(src);
  while (count--) {
    *d++ = *s++;
  }
}

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_UPIU_UPIU_TRANSACTIONS_H_
