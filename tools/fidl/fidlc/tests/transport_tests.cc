// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(TransportTests, GoodChannelTransportWithChannelTransportEnd) {
  TestLibrary library;
  library.AddFile("good/fi-0167.test.fidl");

  ASSERT_COMPILED(library);
}

TEST(TransportTests, GoodDriverTransportWithDriverTransportEnd) {
  TestLibrary library(R"FIDL(
library example;

@transport("Driver")
protocol P {
  M(resource struct{
     c client_end:P;
  }) -> (resource struct{
     s server_end:P;
  });
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(TransportTests, GoodDriverTransportWithChannelTransportEnd) {
  TestLibrary library(R"FIDL(
library example;

protocol ChannelProtocol {};

@transport("Driver")
protocol P {
  M(resource struct{
     c client_end:ChannelProtocol;
  }) -> (resource struct{
     s server_end:ChannelProtocol;
  });
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(TransportTests, GoodDriverTransportWithZirconHandle) {
  TestLibrary library(R"FIDL(
library example;

using zx;

@transport("Driver")
protocol P {
  M() -> (resource struct{
     h zx.Handle;
  });
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);
}

TEST(TransportTests, GoodSyscallTransportWithZirconHandle) {
  TestLibrary library(R"FIDL(
library example;

using zx;

@transport("Syscall")
protocol P {
  M() -> (resource struct{
     h zx.Handle;
  });
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);
}

TEST(TransportTests, GoodBanjoTransportWithZirconHandle) {
  TestLibrary library(R"FIDL(
library example;

using zx;

@transport("Banjo")
protocol P {
  M() -> (resource struct{
     h zx.Handle;
  });
};
)FIDL");
  library.UseLibraryZx();
  ASSERT_COMPILED(library);
}

TEST(TransportTests, GoodDriverTransportWithDriverHandle) {
  TestLibrary library(R"FIDL(
library example;

using fdf;

@transport("Driver")
protocol P {
  M() -> (resource struct{
     h fdf.handle;
  });
};
)FIDL");
  library.UseLibraryFdf();
  ASSERT_COMPILED(library);
}

TEST(TransportTests, BadChannelTransportWithDriverHandle) {
  TestLibrary library(R"FIDL(
library example;

using fdf;

protocol P {
  M() -> (resource struct{
     h fdf.handle;
  });
};
)FIDL");
  library.UseLibraryFdf();
  library.ExpectFail(ErrHandleUsedInIncompatibleTransport, "fdf/handle", "Channel", "protocol 'P'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TransportTests, BadChannelTransportWithDriverClientEndRequest) {
  TestLibrary library(R"FIDL(
library example;

@transport("Driver")
protocol DriverProtocol {};

protocol P {
  M(resource struct{
     c array<vector<box<resource struct{s client_end:DriverProtocol;}>>, 3>;
  });
};
)FIDL");
  library.ExpectFail(ErrTransportEndUsedInIncompatibleTransport, "Driver", "Channel",
                     "protocol 'P'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TransportTests, BadChannelTransportWithDriverServerEndResponse) {
  TestLibrary library(R"FIDL(
library example;

@transport("Driver")
protocol DriverProtocol {};

protocol P {
  M() -> (resource table{
     1: s resource union{
       1: s server_end:DriverProtocol;
     };
  });
};
)FIDL");
  library.ExpectFail(ErrTransportEndUsedInIncompatibleTransport, "Driver", "Channel",
                     "protocol 'P'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TransportTests, BadBanjoTransportWithDriverClientEndRequest) {
  TestLibrary library(R"FIDL(
library example;

@transport("Driver")
protocol DriverProtocol {};

@transport("Banjo")
protocol P {
  M(resource struct{
     s client_end:DriverProtocol;
  });
};
)FIDL");
  library.ExpectFail(ErrTransportEndUsedInIncompatibleTransport, "Driver", "Banjo", "protocol 'P'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TransportTests, BadDriverTransportWithBanjoClientEndRequest) {
  TestLibrary library(R"FIDL(
library example;

@transport("Banjo")
protocol BanjoProtocol {};

@transport("Driver")
protocol P {
  M(resource struct{
     s client_end:BanjoProtocol;
  });
};
)FIDL");
  library.ExpectFail(ErrTransportEndUsedInIncompatibleTransport, "Banjo", "Driver", "protocol 'P'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TransportTests, BadSyscallTransportWithDriverClientEndRequest) {
  TestLibrary library;
  library.AddFile("bad/fi-0118.test.fidl");
  library.ExpectFail(ErrTransportEndUsedInIncompatibleTransport, "Driver", "Syscall",
                     "protocol 'P'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TransportTests, BadSyscallTransportWithSyscallClientEndRequest) {
  TestLibrary library(R"FIDL(
library example;

@transport("Syscall")
protocol SyscallProtocol {};

@transport("Syscall")
protocol P {
  M(resource struct{
     s client_end:SyscallProtocol;
  });
};
)FIDL");
  library.ExpectFail(ErrTransportEndUsedInIncompatibleTransport, "Syscall", "Syscall",
                     "protocol 'P'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TransportTests, BadCustomHandleInZirconChannel) {
  TestLibrary library(R"FIDL(
library example;

type ObjType = strict enum : uint32 {
  NONE = 0;
};
type Rights = strict enum : uint32 {
  SAME_RIGHTS = 0;
};

resource_definition handle : uint32 {
    properties {
        subtype ObjType;
        rights Rights;
    };
};

protocol P {
  M(resource struct{
     h handle;
  });
};
)FIDL");
  library.ExpectFail(ErrHandleUsedInIncompatibleTransport, "example/handle", "Channel",
                     "protocol 'P'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TransportTests, BadDriverHandleInZirconChannel) {
  TestLibrary library;
  library.AddFile("bad/fi-0117.test.fidl");
  library.UseLibraryFdf();
  library.ExpectFail(ErrHandleUsedInIncompatibleTransport, "fdf/handle", "Channel",
                     "protocol 'Protocol'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(TransportTests, BadCannotReassignTransport) {
  TestLibrary library;
  library.AddFile("bad/fi-0167.test.fidl");

  library.ExpectFail(ErrCannotConstrainTwice, "ClientEnd");
  library.ExpectFail(ErrCannotConstrainTwice, "ServerEnd");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
