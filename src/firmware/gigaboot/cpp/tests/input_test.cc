// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "input.h"

#include <lib/efi/testing/mock_serial_io.h>
#include <lib/efi/testing/mock_simple_text_input.h>
#include <lib/efi/testing/mock_simple_text_output.h>
#include <lib/efi/testing/stub_boot_services.h>

#include <gtest/gtest.h>

#include "gmock/gmock.h"
#include "lib/fit/internal/result.h"

namespace gigaboot {

const efi_handle kImageHandle = reinterpret_cast<efi_handle>(0x10);

// Test fixture to set up and tear down InputReceiver test state.
class InputReceiverTest : public ::testing::Test {
 public:
  // Reset xefi global variables so state doesn't bleed between tests.
  void TearDown() override {
    memset(&xefi_global_state, 0, sizeof(xefi_global_state));
    gEfiSystemTable = nullptr;
    gEfiImageHandle = nullptr;
  }

  // Sets up the state and mock expectations for a future call to xefi_init().
  void SetupXefi(efi::MockBootServices& mock_services, efi_serial_io_protocol* serial,
                 efi_simple_text_input_protocol* text_input = nullptr,
                 efi_simple_text_output_protocol* text_output = nullptr) {
    system_table_ = efi_system_table{
        .ConIn = text_input,
        .ConOut = text_output,
        .BootServices = mock_services.services(),
    };
    gEfiSystemTable = &system_table_;
    gEfiImageHandle = kImageHandle;

    if (serial) {
      EXPECT_CALL(mock_services, LocateProtocol(::efi::MatchGuid(EFI_SERIAL_IO_PROTOCOL_GUID),
                                                ::testing::_, ::testing::_))
          .WillRepeatedly(::testing::DoAll(::testing::SetArgPointee<2>(serial),
                                           ::testing::Return(EFI_SUCCESS)));
      EXPECT_CALL(mock_services,
                  CloseProtocol(::testing::_, ::efi::MatchGuid(EFI_SERIAL_IO_PROTOCOL_GUID),
                                kImageHandle, nullptr));
    } else {
      EXPECT_CALL(mock_services, LocateProtocol(::efi::MatchGuid(EFI_SERIAL_IO_PROTOCOL_GUID),
                                                ::testing::_, ::testing::_))
          .WillRepeatedly(::testing::Return(EFI_LOAD_ERROR));
    }
  }

  static InputReceiver::Serial& serial(InputReceiver& r) { return r.serial(); }

 protected:
  efi_system_table system_table_;
  efi::MockBootServices mock_services_;
  efi::MockSerialIoProtocol mock_serial_;
  efi::MockSimpleTextInputProtocol mock_input_;
  efi::MockSimpleTextOutputProtocol mock_output_{true};
  char event_payload_;
};

TEST_F(InputReceiverTest, InitWithoutSerial) {
  SetupXefi(mock_services_, nullptr);

  InputReceiver receiver(&system_table_);
  EXPECT_EQ(receiver.system_table(), &system_table_);
  EXPECT_EQ(bool(serial(receiver)), false);
}

TEST_F(InputReceiverTest, InitWithSerial) {
  SetupXefi(mock_services_, mock_serial_.protocol());

  InputReceiver receiver(&system_table_);
  EXPECT_EQ(receiver.system_table(), &system_table_);
  EXPECT_EQ(bool(serial(receiver)), true);
  EXPECT_CALL(mock_serial_, SetAttributes).WillOnce(::testing::Return(EFI_SUCCESS));
}

TEST_F(InputReceiverTest, GetKeySerialPoll) {
  SetupXefi(mock_services_, mock_serial_.protocol(), mock_input_.protocol());

  EXPECT_CALL(mock_serial_, SetAttributes).WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_input_, ReadKeyStroke).WillRepeatedly(::testing::Return(EFI_NOT_READY));
  mock_serial_.ExpectRead("x");

  InputReceiver receiver(&system_table_);
  EXPECT_EQ('x', receiver.GetKey(zx::sec(0)));
}

TEST_F(InputReceiverTest, GetKeyInputPoll) {
  SetupXefi(mock_services_, nullptr, mock_input_.protocol());

  mock_input_.ExpectReadKeyStroke('z');

  InputReceiver receiver(&system_table_);
  EXPECT_EQ('z', receiver.GetKey(zx::sec(0)));
}

TEST_F(InputReceiverTest, GetKeyInputTakesPrecedence) {
  SetupXefi(mock_services_, mock_serial_.protocol(), mock_input_.protocol());

  EXPECT_CALL(mock_serial_, SetAttributes).WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_serial_, Read).Times(0);
  mock_input_.ExpectReadKeyStroke('z');

  InputReceiver receiver(&system_table_);
  EXPECT_EQ('z', receiver.GetKey(zx::sec(0)));
}

TEST_F(InputReceiverTest, GetKeyPollNoCharacter) {
  SetupXefi(mock_services_, mock_serial_.protocol(), mock_input_.protocol());

  EXPECT_CALL(mock_serial_, SetAttributes).WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_serial_, Read).WillOnce(::testing::Return(EFI_TIMEOUT));
  EXPECT_CALL(mock_input_, ReadKeyStroke).WillOnce(::testing::Return(EFI_NOT_READY));

  InputReceiver receiver(&system_table_);
  fit::result<efi_status, char> expected = fit::error(EFI_NOT_READY);
  EXPECT_EQ(expected, receiver.GetKey(zx::sec(0)));
}

TEST_F(InputReceiverTest, GetKeyTimer) {
  SetupXefi(mock_services_, mock_serial_.protocol(), mock_input_.protocol());

  // Mock 3 "not ready" loops, then find a character on the 4th.
  EXPECT_CALL(mock_services_, CreateEvent(EVT_TIMER, 0, nullptr, nullptr, ::testing::_))
      .WillOnce(::testing::DoAll(::testing::SetArgPointee<4>(&event_payload_),
                                 ::testing::Return(EFI_SUCCESS)));
  EXPECT_CALL(mock_services_, SetTimer(::testing::_, TimerRelative, ::testing::_))
      .WillOnce(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_services_, CheckEvent(::testing::_))
      .Times(3)
      .WillRepeatedly(::testing::Return(EFI_NOT_READY));
  EXPECT_CALL(mock_services_, CloseEvent(&event_payload_)).WillOnce(::testing::Return(EFI_SUCCESS));

  EXPECT_CALL(mock_serial_, SetAttributes).WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_serial_, Read).Times(3).WillRepeatedly(::testing::Return(EFI_TIMEOUT));

  {
    ::testing::InSequence seq;
    EXPECT_CALL(mock_input_, ReadKeyStroke)
        .Times(3)
        .WillRepeatedly(::testing::Return(EFI_NOT_READY));
    mock_input_.ExpectReadKeyStroke('z');
  }

  InputReceiver receiver(&system_table_);
  EXPECT_EQ('z', receiver.GetKey(zx::sec(100)));
}

TEST_F(InputReceiverTest, GetKeyTimeout) {
  SetupXefi(mock_services_, mock_serial_.protocol(), mock_input_.protocol());

  // Mock 2 "not ready" loops, then timeout on the 3rd.
  EXPECT_CALL(mock_services_, CreateEvent(EVT_TIMER, 0, nullptr, nullptr, ::testing::_))
      .WillOnce(::testing::DoAll(::testing::SetArgPointee<4>(&event_payload_),
                                 ::testing::Return(EFI_SUCCESS)));
  EXPECT_CALL(mock_services_, SetTimer(::testing::_, TimerRelative, ::testing::_))
      .WillOnce(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_services_, CheckEvent(::testing::_))
      .WillOnce(::testing::Return(EFI_NOT_READY))
      .WillOnce(::testing::Return(EFI_NOT_READY))
      .WillOnce(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_services_, CloseEvent(&event_payload_)).WillOnce(::testing::Return(EFI_SUCCESS));

  EXPECT_CALL(mock_serial_, SetAttributes).WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_serial_, Read).Times(3).WillRepeatedly(::testing::Return(EFI_TIMEOUT));

  EXPECT_CALL(mock_input_, ReadKeyStroke).Times(3).WillRepeatedly(::testing::Return(EFI_NOT_READY));

  InputReceiver receiver(&system_table_);
  fit::result<efi_status, char> expected = fit::error(EFI_NOT_READY);
  EXPECT_EQ(expected, receiver.GetKey(zx::sec(100)));
}

TEST_F(InputReceiverTest, SerialAttributesFailure) {
  SetupXefi(mock_services_, mock_serial_.protocol(), mock_input_.protocol());

  EXPECT_CALL(mock_serial_, SetAttributes)
      .WillOnce(::testing::Return(EFI_DEVICE_ERROR))
      .WillOnce(::testing::Return(EFI_DEVICE_ERROR));

  InputReceiver receiver(&system_table_);
  fit::result<efi_status, char> expected = fit::error(EFI_DEVICE_ERROR);
  EXPECT_EQ(expected, receiver.GetKey(zx::sec(0)));
}

TEST_F(InputReceiverTest, CreateTimerFailure) {
  SetupXefi(mock_services_, mock_serial_.protocol(), mock_input_.protocol());

  EXPECT_CALL(mock_serial_, SetAttributes).WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_services_, CreateEvent(EVT_TIMER, 0, nullptr, nullptr, ::testing::_))
      .WillOnce(::testing::Return(EFI_OUT_OF_RESOURCES));

  InputReceiver receiver(&system_table_);
  fit::result<efi_status, char> expected = fit::error(EFI_OUT_OF_RESOURCES);
  EXPECT_EQ(expected, receiver.GetKey(zx::sec(100)));
}

TEST_F(InputReceiverTest, SetTimerFailure) {
  SetupXefi(mock_services_, mock_serial_.protocol(), mock_input_.protocol());

  EXPECT_CALL(mock_serial_, SetAttributes).WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_services_, CreateEvent(EVT_TIMER, 0, nullptr, nullptr, ::testing::_))
      .WillOnce(::testing::DoAll(::testing::SetArgPointee<4>(&event_payload_),
                                 ::testing::Return(EFI_SUCCESS)));
  EXPECT_CALL(mock_services_, SetTimer(::testing::_, TimerRelative, ::testing::_))
      .WillOnce(::testing::Return(EFI_INVALID_PARAMETER));
  EXPECT_CALL(mock_services_, CloseEvent(&event_payload_)).WillOnce(::testing::Return(EFI_SUCCESS));

  InputReceiver receiver(&system_table_);
  fit::result<efi_status, char> expected = fit::error(EFI_INVALID_PARAMETER);

  EXPECT_EQ(expected, receiver.GetKey(zx::sec(100)));
}

TEST_F(InputReceiverTest, GetKeyPrompt) {
  SetupXefi(mock_services_, nullptr, mock_input_.protocol(), mock_output_.protocol());
  {
    ::testing::InSequence s;
    EXPECT_CALL(mock_output_, EnableCursor(false)).WillOnce(::testing::Return(EFI_SUCCESS));
    EXPECT_CALL(mock_output_, EnableCursor(true)).WillOnce(::testing::Return(EFI_SUCCESS));
  }
  EXPECT_CALL(mock_services_, CreateEvent(EVT_TIMER, 0, nullptr, nullptr, ::testing::_))
      .WillOnce(::testing::DoAll(::testing::SetArgPointee<4>(&event_payload_),
                                 ::testing::Return(EFI_SUCCESS)));
  EXPECT_CALL(mock_services_, SetTimer(::testing::_, ::testing::_, ::testing::_))
      .WillOnce(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_services_, CloseEvent(&event_payload_)).WillOnce(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_output_, SetCursorPosition(0, 0)).WillRepeatedly(::testing::Return(EFI_SUCCESS));

  mock_input_.ExpectReadKeyStroke('f');
  InputReceiver receiver(&system_table_);
  EXPECT_EQ(receiver.GetKeyPrompt("f", zx::sec(1)), 'f');
}

TEST_F(InputReceiverTest, GetKeyPromptWrongKey) {
  SetupXefi(mock_services_, nullptr, mock_input_.protocol(), mock_output_.protocol());
  {
    ::testing::InSequence s;
    EXPECT_CALL(mock_output_, EnableCursor(false)).WillOnce(::testing::Return(EFI_SUCCESS));
    EXPECT_CALL(mock_output_, EnableCursor(true)).WillOnce(::testing::Return(EFI_SUCCESS));
  }

  EXPECT_CALL(mock_services_, CreateEvent(EVT_TIMER, 0, nullptr, nullptr, ::testing::_))
      .WillRepeatedly(::testing::DoAll(::testing::SetArgPointee<4>(&event_payload_),
                                       ::testing::Return(EFI_SUCCESS)));
  EXPECT_CALL(mock_services_, CloseEvent(&event_payload_))
      .WillRepeatedly(::testing::Return(EFI_SUCCESS));
  mock_input_.ExpectRepeatedReadKeyStroke('z');
  EXPECT_CALL(mock_services_, CheckEvent(::testing::_))
      .WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_services_, SetTimer(::testing::_, ::testing::_, ::testing::_))
      .WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_output_, SetCursorPosition(0, 0)).WillRepeatedly(::testing::Return(EFI_SUCCESS));

  InputReceiver receiver(&system_table_);
  EXPECT_EQ(receiver.GetKeyPrompt("f", zx::sec(5)), std::nullopt);
}

TEST_F(InputReceiverTest, GetKeyPromptTimeout) {
  SetupXefi(mock_services_, nullptr, mock_input_.protocol(), mock_output_.protocol());
  {
    ::testing::InSequence s;
    EXPECT_CALL(mock_output_, EnableCursor(false)).WillOnce(::testing::Return(EFI_SUCCESS));
    EXPECT_CALL(mock_output_, EnableCursor(true)).WillOnce(::testing::Return(EFI_SUCCESS));
  }

  EXPECT_CALL(mock_input_, ReadKeyStroke).WillRepeatedly(::testing::Return(EFI_NOT_READY));
  EXPECT_CALL(mock_services_, CreateEvent(EVT_TIMER, 0, nullptr, nullptr, ::testing::_))
      .WillRepeatedly(::testing::DoAll(::testing::SetArgPointee<4>(&event_payload_),
                                       ::testing::Return(EFI_SUCCESS)));
  EXPECT_CALL(mock_services_, CheckEvent(&event_payload_))
      .WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_services_, SetTimer(::testing::_, ::testing::_, ::testing::_))
      .WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_services_, CloseEvent(&event_payload_))
      .WillRepeatedly(::testing::Return(EFI_SUCCESS));
  EXPECT_CALL(mock_output_, SetCursorPosition(0, 0)).WillRepeatedly(::testing::Return(EFI_SUCCESS));

  InputReceiver receiver(&system_table_);
  EXPECT_EQ(receiver.GetKeyPrompt("f", zx::sec(5)), std::nullopt);
}

}  // namespace gigaboot
