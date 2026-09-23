// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/common/curl.h"

#include <lib/syslog/cpp/macros.h>

#include <memory>

#include "curl/multi.h"
#include "src/developer/debug/shared/message_loop.h"

namespace zxdb {

namespace {

bool global_initialized = false;

template <typename T>
void curl_easy_setopt_CHECK(CURL* handle, CURLoption option, T t) {
  auto result = curl_easy_setopt(handle, option, t);
  FX_DCHECK(result == CURLE_OK);
}

}  // namespace

// All Curl instances share one Curl::MultiHandle instance.
//
// LIFETIME SEMANTICS
// =============================
// The Curl::MultiHandle lives so long as at least one EasyHandle exists. When the last EasyHandle
// is removed, the associated MultiHandle is cleaned up and removed. The curl_multi_cleanup routine
// can issue additional callbacks to |SocketFunction|. This object must remain valid during these
// callbacks to ensure that the higher level callbacks used with the respective Curl objects are
// issued.
//
// The callbacks |TimerFunction| and |SocketFunction| are registered with libcurl below. These
// functions will never call libcurl functions directly, instead tasks are posted to the message
// loop, even in the case of a 0ms timeout given to |TimerFunction|. These callbacks are owned by
// the message loop, and while it is tempting to use reference counting to ensure that this object
// lives through all of the tasks on the message loop, it will not work due to the behavior of
// curl_multi_cleanup mentioned above.
//
// Consider the following situation to see where this goes wrong:
//
// Suppose we have a reference counted object, and the message loop is being destroyed during
// program teardown while a posted task holds a reference to this MultiHandle object, then the final
// task being destructed will trigger this class to be destructed. This causes a problem, because we
// cannot call curl_multi_cleanup from our own destructor. If we call it from our own destructor, we
// will end up in |SocketFunction| as a result libcurl's cleanup process, which will post a task to
// the message loop that increments the reference count for |this| from the destructor. When that
// task is executed, everything will work (the CURLM object isn't completely invalid until
// curl_multi_cleanup returns) until that reference counted object destructs, returning the
// reference count to 0, causing its destruction. From here, we try to call curl_multi_cleanup
// again, but libcurl catches this and will return a CURLE_RECURSIVE_API_CALL error because we've
// already begun cleanup from the original destructor.
class Curl::MultiHandle {
 public:
  static Curl::MultiHandle* GetInstance();

  // Returns true if there is an existing MultiHandle instance. If this returns true, then it is
  // guaranteed that a subsequent synchronous call to |GetInstance| will not perform a new
  // allocation.
  static bool HasInstance();

  MultiHandle();
  ~MultiHandle();

  // Adds an easy handle and starts the transfer. The ownership of the easy handle will be shared
  // by this class when the transfer is in progress.
  void AddEasyHandle(Curl* curl);

  // Must be called before destructing. This calls the curl multihandle cleanup routines, which
  // could reentrantly call |SocketFunction|, so we need to remain in a valid state until those
  // cleanup routines are completed. Once they are, this object can be destructed.
  void Cleanup();

 private:
  // Function given to CURL which it uses to inform us it would like to do IO on a socket and that
  // we should add it to our polling in the event loop.
  static int SocketFunction(CURL* easy, curl_socket_t s, int what, void* userp, void* socketp);

  // Function given to CURL which it uses to inform us it would like to receive a timer notification
  // at a given time in the future. If the callback is called twice before the timer expires it is
  // expected to re-schedule the existing timer, not make a second timer. A timeout of -1 means to
  // cancel the outstanding timer.
  static int TimerFunction(CURLM* multi, long timeout_ms, void* userp);

  // Unique MultiHandle for all |Curl| (shorthand for an "EasyHandle") instances being used for
  // downloads. This is unique for this thread and is set by calling |GetInstance|. Once all
  // EasyHandles (i.e. all |Curl| objects have been destroyed) are closed for this MultiHandle, then
  // this MultiHandle is cleaned up and deleted. The next download started by the upper layers will
  // create a new MultiHandle instance.
  static thread_local std::unique_ptr<MultiHandle> instance_;

  void ProcessResponses();

  void DrainResponses();

  CURLM* multi_handle_;

  // Manages the ownership of WatchHandles.
  std::map<curl_socket_t, debug::MessageLoop::WatchHandle> watches_;

  // Manages the ownership of easy handles to prevent them from being destructed when an async
  // transfer is in progress.
  std::map<CURL*, fxl::RefPtr<Curl>> easy_handles_;

  // Indicates whether we already have a task posted to process the messages in multi_handler_.
  bool process_pending_ = false;
};

thread_local std::unique_ptr<Curl::MultiHandle> Curl::MultiHandle::instance_ = nullptr;

bool Curl::MultiHandle::HasInstance() { return !!instance_; }

Curl::MultiHandle* Curl::MultiHandle::GetInstance() {
  if (HasInstance()) {
    return instance_.get();
  }

  instance_ = std::make_unique<Curl::MultiHandle>();

  return instance_.get();
}

void Curl::MultiHandle::AddEasyHandle(Curl* curl) {
  easy_handles_.emplace(curl->curl_, fxl::RefPtr<Curl>(curl));
  auto result = curl_multi_add_handle(multi_handle_, curl->curl_);
  FX_DCHECK(result == CURLM_OK) << curl_multi_strerror(result);

  // There's a chance that the response is available immediately in curl_multi_add_handle, which
  // could happen when the server is localhost, e.g. requesting authentication from metadata server
  // on GCE. In this case, no SocketFunction will be invoked and we have to call ProcessResponses()
  // manually.
  ProcessResponses();
}

void Curl::MultiHandle::Cleanup() {
  // Remove any existing watches. New ones may be installed by |curl_multi_cleanup|.
  watches_.clear();

  auto result = curl_multi_cleanup(multi_handle_);
  FX_DCHECK(result == CURLM_OK) << curl_multi_strerror(result);

  multi_handle_ = nullptr;
  instance_.reset();
}

void Curl::MultiHandle::ProcessResponses() {
  FX_CHECK(instance_) << __PRETTY_FUNCTION__ << " instance_ is null!";

  if (process_pending_) {
    return;
  }

  process_pending_ = true;

  // This is to ensure that we do not reentrantly call libcurl routines from within either
  // |SocketFunction| or |TimerFunction|, which is not allowed.
  debug::MessageLoop::Current()->PostTask(FROM_HERE, []() {
    FX_CHECK(instance_) << "ProcessResponses callback but instance_ is null!";
    instance_->DrainResponses();
  });
}

void Curl::MultiHandle::DrainResponses() {
  process_pending_ = false;

  int _ignore;
  while (auto info = curl_multi_info_read(multi_handle_, &_ignore)) {
    if (info->msg != CURLMSG_DONE) {
      // CURLMSG_DONE is the only value for msg, documented or otherwise, so this is mostly
      // future-proofing at writing.
      continue;
    }

    auto easy_handle_it = easy_handles_.find(info->easy_handle);
    FX_DCHECK(easy_handle_it != easy_handles_.end());
    fxl::RefPtr<Curl> curl = std::move(easy_handle_it->second);
    curl->FreeSList();
    easy_handles_.erase(easy_handle_it);

    // The document says WARNING: The data the returned pointer points to will not survive
    // calling curl_multi_cleanup, curl_multi_remove_handle or curl_easy_cleanup.
    CURLcode code = info->data.result;
    auto result = curl_multi_remove_handle(multi_handle_, info->easy_handle);
    FX_DCHECK(result == CURLM_OK);
    // info is invalid now.

    curl->multi_cb_(curl.get(), Curl::Error(code));

    // curl->multi_cb_ becomes nullptr now because fit::callback can only be called once. Since we
    // moved |curl| out of our map, it going out of scope drops our reference, possibly destructing
    // it.
  }

  // If we just deleted our last handle, then it is time to cleanup.
  if (easy_handles_.empty()) {
    FX_CHECK(this == instance_.get());
    // Note that the cleanup routines called here are allowed to call |SocketFunction| again.
    Cleanup();
    // |this| and |instance_| are no longer valid.
  }
}

int Curl::MultiHandle::SocketFunction(CURL* /*easy*/, curl_socket_t s, int what, void* /*userp*/,
                                      void* /*socketp*/) {
  FX_CHECK(instance_);

  if (what == CURL_POLL_REMOVE || what == CURL_POLL_NONE) {
    instance_->watches_.erase(s);
  } else {
    debug::MessageLoop::WatchMode mode;

    switch (what) {
      case CURL_POLL_IN:
        mode = debug::MessageLoop::WatchMode::kRead;
        break;
      case CURL_POLL_OUT:
        mode = debug::MessageLoop::WatchMode::kWrite;
        break;
      case CURL_POLL_INOUT:
        mode = debug::MessageLoop::WatchMode::kReadWrite;
        break;
      default:
        FX_NOTREACHED();
        return -1;
    }

    instance_->watches_[s] = debug::MessageLoop::Current()->WatchFD(
        mode, s, [](int fd, bool read, bool write, bool err) {
          // |instance_| should still be valid while we have an easy handle.
          FX_CHECK(instance_) << "SocketFunction callback, MultiHandle:<null>";

          int action = 0;
          if (read)
            action |= CURL_CSELECT_IN;
          if (write)
            action |= CURL_CSELECT_OUT;
          if (err)
            action |= CURL_CSELECT_ERR;

          int _ignore;
          auto result = curl_multi_socket_action(instance_->multi_handle_, fd, action, &_ignore);
          FX_DCHECK(result == CURLM_OK);

          instance_->ProcessResponses();
        });
  }

  return 0;
}

int Curl::MultiHandle::TimerFunction(CURLM* /*multi*/, long timeout_ms, void* /*userp*/) {
  FX_DCHECK(instance_);

  // A timeout_ms value of -1 passed to this callback means you should delete the timer.
  if (timeout_ms < 0) {
    return 0;
  }

  auto cb = []() {
    if (!HasInstance()) {
      return;
    }

    int _ignore;
    auto result =
        curl_multi_socket_action(instance_->multi_handle_, CURL_SOCKET_TIMEOUT, 0, &_ignore);
    FX_DCHECK(result == CURLM_OK);

    instance_->ProcessResponses();
  };

  if (timeout_ms == 0) {
    debug::MessageLoop::Current()->PostTask(FROM_HERE, std::move(cb));
  } else {
    debug::MessageLoop::Current()->PostTimer(FROM_HERE, timeout_ms, std::move(cb));
  }

  return 0;
}

Curl::MultiHandle::MultiHandle() {
  FX_DCHECK(instance_ == nullptr);

  FX_DCHECK(global_initialized);

  multi_handle_ = curl_multi_init();
  FX_DCHECK(multi_handle_);

  auto result = curl_multi_setopt(multi_handle_, CURLMOPT_SOCKETFUNCTION, SocketFunction);
  FX_DCHECK(result == CURLM_OK);
  result = curl_multi_setopt(multi_handle_, CURLMOPT_TIMERFUNCTION, TimerFunction);
  FX_DCHECK(result == CURLM_OK);
}

Curl::MultiHandle::~MultiHandle() {
  // It is possible for |easy_handles_| to be non-empty here, if for example, there is a request
  // that the server is not responding to. In that case, the MultiHandle object could be tearing
  // down from TLS destructors after the message loop, and the MultiHandle is now invalid.
  easy_handles_.clear();
  watches_.clear();

  // Since this class is owned by a thread_local, it's possible to be destructed without having
  // |Cleanup| called (e.g. during TLS destructors), using the same reasoning as above, leaving the
  // multihandle leaked. By clearing |easy_handles_| above, none of these cleanup tasks should call
  // |SocketFunction| since there are no more valid connections.
  if (multi_handle_) {
    auto result = curl_multi_cleanup(multi_handle_);
    FX_CHECK(result == CURLM_OK);
  }
}

void Curl::GlobalInit() {
  FX_DCHECK(!global_initialized);
  auto res = curl_global_init(CURL_GLOBAL_SSL);
  FX_DCHECK(!res);
  global_initialized = true;
}

void Curl::GlobalCleanup() {
  FX_DCHECK(global_initialized);
  if (Curl::MultiHandle::HasInstance()) {
    // We have a live MultiHandle which must be cleaned up first.
    Curl::MultiHandle::GetInstance()->Cleanup();
  }
  curl_global_cleanup();
  global_initialized = false;
}

Curl::Curl() {
  curl_ = curl_easy_init();
  FX_DCHECK(curl_);
}

Curl::~Curl() {
  // multi_cb_ is allowed to be not null here, e.g., a perform is in progress.
  curl_easy_cleanup(curl_);
}

void Curl::set_post_data(const std::map<std::string, std::string>& items) {
  std::string encoded;

  for (const auto& item : items) {
    if (!encoded.empty()) {
      encoded += "&";
    }

    encoded += Escape(item.first);
    encoded += "=";
    encoded += Escape(item.second);
  }

  set_post_data(encoded);
}

std::string Curl::Escape(const std::string& input) {
  // It's legal to pass a null Curl_easy to curl_easy_escape (actually Curl_convert_to_network).
  auto escaped = curl_easy_escape(nullptr, input.c_str(), static_cast<int>(input.size()));
  // std::string(nullptr) is an UB.
  if (!escaped)
    return "";
  std::string ret(escaped);
  curl_free(escaped);
  return ret;
}

void Curl::PrepareToPerform() {
  FX_DCHECK(!multi_cb_);

  // It's critical to convert the lambda into function pointer, because the lambda is only valid in
  // this scope but the function pointer is always valid even without "static".
  using FunctionType = size_t (*)(char* data, size_t size, size_t nitems, void* curl);
  FunctionType DoHeaderCallback = [](char* data, size_t size, size_t nitems, void* curl) {
    return reinterpret_cast<Curl*>(curl)->header_callback_(std::string(data, size * nitems));
  };
  FunctionType DoDataCallback = [](char* data, size_t size, size_t nitems, void* curl) {
    return reinterpret_cast<Curl*>(curl)->data_callback_(std::string(data, size * nitems));
  };

  curl_easy_setopt_CHECK(curl_, CURLOPT_HEADERFUNCTION, DoHeaderCallback);
  curl_easy_setopt_CHECK(curl_, CURLOPT_HEADERDATA, this);
  curl_easy_setopt_CHECK(curl_, CURLOPT_WRITEFUNCTION, DoDataCallback);
  curl_easy_setopt_CHECK(curl_, CURLOPT_WRITEDATA, this);

  // We don't want to set a hard timeout on the request, as the symbol file might be extremely large
  // and the downloading might take arbitrary time.
  // The default connect timeout is 300s, which is too long for today's network.
  curl_easy_setopt_CHECK(curl_, CURLOPT_CONNECTTIMEOUT, 10L);
  // Curl will install some signal handler for SIGPIPE which causes a segfault if NOSIGNAL is unset.
  curl_easy_setopt_CHECK(curl_, CURLOPT_NOSIGNAL, 1L);
  // Abort if slower than 1 bytes/sec during 10 seconds. Ideally we want a read timeout.
  // This will install a lot of timers (one for each read() call) to the message loop.
  curl_easy_setopt_CHECK(curl_, CURLOPT_LOW_SPEED_LIMIT, 1L);
  curl_easy_setopt_CHECK(curl_, CURLOPT_LOW_SPEED_TIME, 10L);

  // API documentation specifies "A long value of 1" enables this option, so we convert very
  // specifically. Why take chances on sensible behavior?
  curl_easy_setopt_CHECK(curl_, CURLOPT_NOBODY, get_body_ ? 0L : 1L);

  if (post_data_.empty()) {
    curl_easy_setopt_CHECK(curl_, CURLOPT_POST, 0);
  } else {
    curl_easy_setopt_CHECK(curl_, CURLOPT_POSTFIELDS, post_data_.data());
    curl_easy_setopt_CHECK(curl_, CURLOPT_POSTFIELDSIZE, post_data_.size());
  }

  FX_DCHECK(!slist_);
  for (const auto& header : headers_) {
    slist_ = curl_slist_append(slist_, header.c_str());
  }

  curl_easy_setopt_CHECK(curl_, CURLOPT_HTTPHEADER, slist_);
}

void Curl::FreeSList() {
  if (slist_)
    curl_slist_free_all(slist_);
  slist_ = nullptr;
}

Curl::Error Curl::Perform() {
  PrepareToPerform();
  auto ret = Error(curl_easy_perform(curl_));
  FreeSList();
  return ret;
}

void Curl::Perform(fit::callback<void(Curl*, Curl::Error)> cb) {
  PrepareToPerform();
  multi_cb_ = std::move(cb);
  MultiHandle::GetInstance()->AddEasyHandle(this);
}

long Curl::ResponseCode() {
  long ret;

  auto result = curl_easy_getinfo(curl_, CURLINFO_RESPONSE_CODE, &ret);
  FX_DCHECK(result == CURLE_OK);
  return ret;
}

}  // namespace zxdb
