// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <getopt.h>
#include <lib/fit/defer.h>
#include <lib/zxdump/dump.h>
#include <lib/zxdump/elf-search.h>
#include <lib/zxdump/fd-writer.h>
#include <lib/zxdump/task.h>
#include <lib/zxdump/zstd-writer.h>
#include <zircon/assert.h>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <functional>
#include <iostream>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <variant>

#include <fbl/unique_fd.h>
#include <rapidjson/error/en.h>
#include <rapidjson/filereadstream.h>
#include <rapidjson/reader.h>
#include <rapidjson/stream.h>
#include <rapidjson/stringbuffer.h>
#include <rapidjson/writer.h>

#include "cli.h"

namespace {

using namespace std::literals;

constexpr std::string_view kOutputPrefix = "core."sv;
constexpr std::string_view kArchiveSuffix = ".a"sv;
constexpr std::string_view kZstdSuffix = ".zst"sv;

constexpr std::string_view kDefaultRemarksName = "remarks"sv;

struct Remarks {
  std::string name;
  std::unique_ptr<rapidjson::StringBuffer> data{new rapidjson::StringBuffer};
};

class Writer;  // Forward declaration.

// Command-line flags controlling the dump are parsed into this object, which
// is passed around to the methods affected by policy choices.
struct Flags {
  std::string OutputFile(zx_koid_t pid, bool outer = true, std::string_view suffix = {}) const {
    std::string filename{outer ? output_prefix : kOutputPrefix};
    filename += std::to_string(pid);
    filename += suffix;
    if (outer && zstd) {
      filename += kZstdSuffix;
    }
    return filename;
  }

  time_t Date(zxdump::Task& task) const { return record_date ? task.date() : 0; }

  std::vector<Remarks> remarks;
  std::string_view output_prefix = kOutputPrefix;
  std::unique_ptr<Writer> streaming;
  size_t limit = zxdump::DefaultLimit();
  bool dump_memory = true;
  bool collect_system = false;
  bool repeat_system = false;
  bool collect_kernel = false;
  bool repeat_kernel = false;
  bool collect_threads = true;
  bool collect_job_children = true;
  bool collect_job_processes = true;
  bool flatten_jobs = false;
  bool record_date = true;
  bool repeat_remarks = false;
  bool zstd = false;
  bool streaming_archive = false;
};

// This handles writing a single output file, and removing that output file if
// the dump is aborted before `Ok(true)` is called.
class Writer {
 public:
  using error_type = zxdump::FdWriter::error_type;

  Writer() = delete;

  Writer(fbl::unique_fd fd, std::string filename, bool zstd)
      : writer_{zstd ? WhichWriter{zxdump::ZstdWriter(std::move(fd))}
                     : WhichWriter{zxdump::FdWriter(std::move(fd))}},
        filename_{std::move(filename)} {}

  auto AccumulateFragmentsCallback() {
    return std::visit(
        [](auto& writer)
            -> fit::function<fit::result<error_type>(size_t offset, zxdump::ByteView data)> {
          return writer.AccumulateFragmentsCallback();
        },
        writer_);
  }

  auto WriteFragments() {
    return std::visit(
        [](auto& writer) -> fit::result<error_type, size_t> { return writer.WriteFragments(); },
        writer_);
  }

  auto WriteCallback() {
    return std::visit(
        [](auto& writer)
            -> fit::function<fit::result<error_type>(size_t offset, zxdump::ByteView data)> {
          return writer.WriteCallback();
        },
        writer_);
  }

  void ResetOffset() {
    std::visit([](auto& writer) { writer.ResetOffset(); }, writer_);
  }

  // Write errors from errno use the file name.
  void Error(zxdump::FdError error) {
    std::string_view fn = filename_;
    if (fn.empty()) {
      fn = "<stdout>"sv;
    }
    std::cerr << fn << ": "sv << error << std::endl;
  }

  // Called with true if the output file should be preserved at destruction.
  bool Ok(bool ok) {
    if (ok) {
      if (auto writer = std::get_if<zxdump::ZstdWriter>(&writer_)) {
        auto result = writer->Finish();
        if (result.is_error()) {
          Error(result.error_value());
          return false;
        }
      }
      filename_.clear();
    }
    return ok;
  }

  bool StartArchive() {
    auto result = zxdump::JobDump::DumpArchiveHeader(WriteCallback());
    if (result.is_error()) {
      Error(result.error_value());
    }
    return result.is_ok();
  }

  bool DumpMemberHeader(std::string_view name, size_t size, time_t mtime) {
    // File offset calculations start fresh with each member.
    ResetOffset();
    auto result = zxdump::JobDump::DumpMemberHeader(WriteCallback(), 0, name, size, mtime);
    if (result.is_error()) {
      Error(*result.error_value().dump_error_);
    }
    return result.is_ok();
  }

  ~Writer() {
    if (!filename_.empty()) {
      remove(filename_.c_str());
    }
  }

 private:
  using WhichWriter = std::variant<zxdump::FdWriter, zxdump::ZstdWriter>;

  WhichWriter writer_;
  std::string filename_;
};

size_t MemberHeaderSize() { return zxdump::JobDump::MemberHeaderSize(); }

// This is the base class of ProcessDumper and JobDumper; the object
// handles collecting and producing the dump for one process or one job.
// The Writer and Flags objects get passed in to control the details
// and of the dump and where it goes.
class DumperBase {
 public:
  // Read errors from syscalls use the PID (or job KOID).
  void Error(const zxdump::Error& error) const {
    std::cerr << koid_ << ": "sv << error << std::endl;
  }

  zx_koid_t koid() const { return koid_; }

  time_t ClockIn(const Flags& flags) { return 0; }

 protected:
  constexpr explicit DumperBase(zx_koid_t koid) : koid_(koid) {}

 private:
  zx_koid_t koid_ = ZX_KOID_INVALID;
};

// This does the Collect* calls that are common to ProcessDumper and JobDumper.
constexpr auto CollectCommon =  //
    [](const Flags& flags, bool top, auto& dumper,
       const zxdump::TaskHolder& holder) -> fit::result<zxdump::Error> {
  if (flags.collect_system && (top || flags.repeat_system)) {
    auto result = dumper.CollectSystem(holder);
    if (result.is_error()) {
      return result.take_error();
    }
  }

  if (flags.collect_kernel && (top || flags.repeat_kernel)) {
    auto result = dumper.CollectKernel();
    if (result.is_error()) {
      return result.take_error();
    }
  }

  if (top || flags.repeat_remarks) {
    for (const auto& [name, data] : flags.remarks) {
      std::string_view contents{data->GetString(), data->GetSize()};
      auto result = dumper.Remarks(name, contents);
      if (result.is_error()) {
        return result.take_error();
      }
    }
  }

  return fit::ok();
};

class ProcessDumper : public DumperBase {
 public:
  explicit ProcessDumper(zxdump::Process& process) : DumperBase{process.koid()}, dumper_{process} {}

  auto OutputFile(const Flags& flags, bool outer = true) const {
    return flags.OutputFile(koid(), outer);
  }

  time_t ClockIn(const Flags& flags) {
    time_t dump_date = flags.Date(dumper_.process());
    if (dump_date != 0) {
      dumper_.set_date(dump_date);
    }
    return dump_date;
  }

  // Phase 1: Collect underpants!
  std::optional<size_t> Collect(const Flags& flags, bool top, const zxdump::TaskHolder& holder) {
    if (flags.collect_threads) {
      auto result = dumper_.SuspendAndCollectThreads();
      if (result.is_error()) {
        Error(result.error_value());
        return std::nullopt;
      }
    }

    if (auto result = CollectCommon(flags, top, dumper_, holder); result.is_error()) {
      Error(result.error_value());
      return std::nullopt;
    }

    auto result = dumper_.CollectProcess(ChooseSegmentCallback(flags), flags.limit);
    if (result.is_error()) {
      Error(result.error_value());
      return std::nullopt;
    }

    return result.value();
  }

  // Phase 2: ???
  bool Dump(Writer& writer, const Flags& flags, const zxdump::TaskHolder& holder) {
    // File offset calculations start fresh in each ET_CORE file.
    writer.ResetOffset();

    // Now gather the accumulated header data first: not including the memory.
    // These iovecs will point into storage in the ProcessDump object itself.
    size_t total;
    if (auto result = dumper_.DumpHeaders(writer.AccumulateFragmentsCallback(), flags.limit);
        result.is_error()) {
      Error(result.error_value());
      return false;
    } else {
      total = result.value();
    }

    if (total > flags.limit) {
      writer.Error({.op_ = "not written"sv, .error_ = EFBIG});
      return false;
    }

    // All the fragments gathered above get written at once.
    if (auto result = writer.WriteFragments(); result.is_error()) {
      writer.Error(result.error_value());
      return false;
    }

    // Stream the memory out via a temporary buffer that's reused repeatedly
    // for each callback.
    if (auto memory = dumper_.DumpMemory(writer.WriteCallback(), flags.limit); memory.is_error()) {
      Error(memory.error_value());
      return false;
    }

    return true;
  }

 private:
  zxdump::SegmentCallback ChooseSegmentCallback(const Flags& flags) {
    if (!flags.dump_memory) {
      return PruneAll;
    }

    // TODO(mcgrathr): more filtering switches

    return std::bind_front(&ProcessDumper::PruneDefault, this);
  }

  static fit::result<zxdump::Error, zxdump::SegmentDisposition> PruneAll(
      zxdump::SegmentDisposition segment, const zx_info_maps_t& mapping, const zx_info_vmo_t& vmo) {
    segment.filesz = 0;
    return fit::ok(segment);
  }

  fit::result<zxdump::Error, zxdump::SegmentDisposition> PruneDefault(
      zxdump::SegmentDisposition segment, const zx_info_maps_t& mapping, const zx_info_vmo_t& vmo) {
    if (mapping.u.mapping.committed_bytes == 0 &&   // No private RAM here,
        vmo.parent_koid == ZX_KOID_INVALID &&       // and none shared,
        !(vmo.flags & ZX_INFO_VMO_PAGER_BACKED)) {  // and no backing store.
      // Since it's not pager-backed, there isn't data hidden in backing
      // store.  If we read this, it would just be zero-fill anyway.
      segment.filesz = 0;
    }

    // TODO(mcgrathr): for now, dump everything else.

    // Check for build ID.
    if (segment.filesz > 0 && zxdump::IsLikelyElfMapping(mapping)) {
      auto result = dumper_.FindBuildIdNote(mapping);
      if (result.is_error()) {
        return result.take_error();
      }
      segment.note = result.value();
      if (segment.note) {
        // TODO(mcgrathr): This could e.g. decide under some switch to truncate
        // the segment to just enough pages to include the build ID (usually
        // one).
        //
        // segment.filesz = PageAlign(note->vaddr + note->size) - mapping.base;
      }
    }

    return fit::ok(segment);
  }

  zxdump::ProcessDump dumper_;
};

// JobDumper handles dumping one job archive, either hierarchical or flattened.
class JobDumper : public DumperBase {
 public:
  explicit JobDumper(zxdump::Job& job) : DumperBase{job.koid()}, dumper_{job} {}

  auto OutputFile(const Flags& flags, bool outer = true) const {
    return flags.OutputFile(koid(), outer, kArchiveSuffix);
  }

  // The job dumper records the date of collection to use in the stub archive
  // member headers.  If the job archive is collected inside another archive,
  // this will also be the date in the member header for the nested archive.
  time_t date() const { return date_; }

  auto* operator->() { return &dumper_; }

  time_t ClockIn(const Flags& flags) {
    date_ = flags.Date(dumper_.job());
    return date_;
  }

  // Collect the job-wide data and reify the lists of children and processes.
  std::optional<size_t> Collect(const Flags& flags, bool top, const zxdump::TaskHolder& holder) {
    if (auto result = CollectCommon(flags, top, dumper_, holder); result.is_error()) {
      Error(result.error_value());
      return std::nullopt;
    }
    size_t size;
    if (auto result = dumper_.CollectJob(); result.is_error()) {
      Error(result.error_value());
      return std::nullopt;
    } else {
      size = result.value();
    }
    if (flags.collect_job_children) {
      if (auto result = dumper_.CollectChildren(); result.is_error()) {
        Error(result.error_value());
        return std::nullopt;
      } else {
        children_ = &(result.value().get());
      }
    }
    if (flags.collect_job_processes) {
      if (auto result = dumper_.CollectProcesses(); result.is_error()) {
        Error(result.error_value());
        return std::nullopt;
      } else {
        processes_ = &(result.value().get());
      }
    }
    return size;
  }

  // Dump the job archive: first dump the stub archive, and then collect and
  // dump each process and each child.
  bool Dump(Writer& writer, const Flags& flags, const zxdump::TaskHolder& holder);

 private:
  class CollectedJob;

  JobDumper(zxdump::JobDump job, zx_koid_t koid) : DumperBase{koid}, dumper_{std::move(job)} {}

  bool DumpHeaders(Writer& writer, const Flags& flags) {
    // File offset calculations start fresh in each archive.
    writer.ResetOffset();

    if (auto result = dumper_.DumpHeaders(writer.AccumulateFragmentsCallback(), date_);
        result.is_error()) {
      Error(result.error_value());
      return false;
    }

    auto result = writer.WriteFragments();
    if (result.is_error()) {
      writer.Error(result.error_value());
      return false;
    }

    return true;
  }

  static bool DumpMemberHeader(Writer& writer, std::string_view name, size_t size, time_t mtime) {
    // File offset calculations start fresh with each member.
    writer.ResetOffset();
    auto result = zxdump::JobDump::DumpMemberHeader(writer.WriteCallback(), 0, name, size, mtime);
    if (result.is_error()) {
      writer.Error(*result.error_value().dump_error_);
    }
    return result.is_ok();
  }

  zxdump::JobDump dumper_;
  zxdump::Job::JobMap* children_ = nullptr;
  zxdump::Job::ProcessMap* processes_ = nullptr;
  time_t date_ = 0;
};

// When dumping an hierarchical job archive, a CollectedJob object supports
// DeepCollect, that populates a tree of CollectedJob and CollectedProcess
// objects before the whole tree is dumped en masse.
class JobDumper::CollectedJob {
 public:
  CollectedJob(CollectedJob&&) = default;
  CollectedJob& operator=(CollectedJob&&) = default;

  explicit CollectedJob(JobDumper dumper) : dumper_(std::move(dumper)) {}

  // This is false either if the original Collect failed and this is an empty
  // object; or if any process or child collection failed so the archive is
  // still valid but just omits some processes and/or children.
  bool ok() const { return ok_; }

  // This includes the whole size of the job archive itself plus its
  // own member header as a member of its parent archive.
  size_t size_bytes() const { return MemberHeaderSize() + content_size_; }

  time_t date() const { return dumper_.date(); }

  // Returns true if the job itself was collected.
  // Later ok() indicates if any process or child collection failed.
  bool DeepCollect(const Flags& flags, const zxdump::TaskHolder& holder) {
    // Collect the job itself.
    dumper_.ClockIn(flags);
    if (auto collected_size = dumper_.Collect(flags, false, holder)) {
      content_size_ += *collected_size;

      // Collect all its processes and children.
      if (dumper_.processes_) {
        for (auto& [pid, process] : *dumper_.processes_) {
          CollectProcess(process, flags, holder);
        }
      }
      if (dumper_.children_) {
        for (auto& [koid, job] : *dumper_.children_) {
          CollectJob(job, flags, holder);
        }
      }
      return true;
    }
    ok_ = false;
    return false;
  }

  bool Dump(Writer& writer, const Flags& flags, const zxdump::TaskHolder& holder) {
    // First dump the member header for this archive as a member of its parent.
    // Then dump the "stub archive" describing the job itself.
    if (!writer.DumpMemberHeader(dumper_.OutputFile(flags, false), content_size_, date()) ||
        !dumper_.DumpHeaders(writer, flags)) {
      ok_ = false;
    } else {
      for (auto& process : processes_) {
        // Each CollectedProcess dumps its own member header and ET_CORE file.
        ok_ = process.Dump(writer, flags, holder) && ok_;
      }
      for (auto& job : children_) {
        // Recurse on each child to dump its own member header and job archive.
        ok_ = job.Dump(writer, flags, holder) && ok_;
      }
    }
    return ok_;
  }

 private:
  // At the leaves of the tree are processes still suspended after collection.
  class CollectedProcess {
   public:
    CollectedProcess(CollectedProcess&&) = default;
    CollectedProcess& operator=(CollectedProcess&&) = default;

    // Constructed with the process dumper and the result of Collect on it.
    CollectedProcess(ProcessDumper&& dumper, size_t size, time_t date)
        : dumper_(std::move(dumper)), content_size_(size), date_(date) {}

    // This includes the whole size of the ET_CORE file itself plus its
    // own member header as a member of its parent archive.
    size_t size_bytes() const { return MemberHeaderSize() + content_size_; }

    time_t date() const { return date_; }

    // Dump the member header and then the ET_CORE file contents.
    bool Dump(Writer& writer, const Flags& flags, const zxdump::TaskHolder& holder) {
      return writer.DumpMemberHeader(dumper_.OutputFile(flags, false), content_size_, date()) &&
             dumper_.Dump(writer, flags, holder);
    }

   private:
    ProcessDumper dumper_;
    size_t content_size_ = 0;
    time_t date_ = 0;
  };

  void CollectProcess(zxdump::Process& process, const Flags& flags,
                      const zxdump::TaskHolder& holder) {
    ProcessDumper dump{process};
    time_t dump_date = dump.ClockIn(flags);
    if (auto collected_size = dump.Collect(flags, false, holder)) {
      CollectedProcess core_file{std::move(dump), *collected_size, dump_date};
      content_size_ += core_file.size_bytes();
      processes_.push_back(std::move(core_file));
    } else {
      ok_ = false;
    }
  }

  void CollectJob(zxdump::Job& job, const Flags& flags, const zxdump::TaskHolder& holder) {
    CollectedJob archive{JobDumper{job}};
    if (archive.DeepCollect(flags, holder)) {
      content_size_ += archive.size_bytes();
      children_.push_back(std::move(archive));
    }
    // The job archive reports not OK if it was collected but omits some dumps.
    ok_ = archive.ok() && ok_;
  }

  JobDumper dumper_;
  std::vector<CollectedProcess> processes_;
  std::vector<CollectedJob> children_;
  size_t content_size_ = 0;
  bool ok_ = true;
};

bool JobDumper::Dump(Writer& writer, const Flags& flags, const zxdump::TaskHolder& holder) {
  if (!DumpHeaders(writer, flags)) {
    return false;
  }

  bool ok = true;
  if (processes_) {
    for (auto& [pid, process] : *processes_) {
      // Collect the process and thus discover the ET_CORE file size.
      ProcessDumper process_dump{process};
      time_t process_dump_date = process_dump.ClockIn(flags);
      if (auto collected_size = process_dump.Collect(flags, false, holder)) {
        // Dump the member header, now complete with size.
        if (!writer.DumpMemberHeader(process_dump.OutputFile(flags, false),  //
                                     *collected_size, process_dump_date)) {
          // Bail early for a write error, since later writes would fail too.
          return false;
        }
        // Now dump the member contents, the ET_CORE file for the process.
        ok = process_dump.Dump(writer, flags, holder) && ok;
      }
    }
  }

  if (children_) {
    for (auto& [koid, job] : *children_) {
      if (flags.flatten_jobs) {
        // Collect just this job first.
        JobDumper child{job};
        auto collected_job_size = child.Collect(flags, false, holder);
        ok = collected_job_size &&
             // Stream out the member header for just the stub archive alone.
             writer.DumpMemberHeader(child.OutputFile(flags, false),  //
                                     *collected_job_size, child.date()) &&
             // Now recurse to dump the stub archive followed by process and
             // child members.  Since the member header for the inner archive
             // only covers the stub archive, these become members in the outer
             // (flat) archive rather than members of the inner job archive.
             // Another inner recursion will do the same thing, so all the
             // recursions stream out a single flat archive.
             child.Dump(writer, flags, holder) && ok;
      } else {
        // Pre-collect the whole job tree and thus discover the archive size.
        // The pre-collected archive dumps its own member header first.
        CollectedJob archive{JobDumper{job}};
        ok = archive.DeepCollect(flags, holder) && archive.Dump(writer, flags, holder) && ok;
      }
    }
  }

  return ok;
}

fbl::unique_fd CreateOutputFile(const std::string& outfile) {
  fbl::unique_fd fd{open(outfile.c_str(), O_WRONLY | O_CREAT | O_TRUNC | O_EXCL, 0666)};
  if (!fd) {
    perror(outfile.c_str());
  }
  return fd;
}

// Phase 3: Profit!
template <typename Dumper>
bool WriteDump(Dumper dumper, const Flags& flags, const zxdump::TaskHolder& holder) {
  time_t date = dumper.ClockIn(flags);

  if (flags.streaming) {
    Writer& writer = *flags.streaming;
    if (auto size = dumper.Collect(flags, true, holder)) {
      if (!flags.streaming_archive ||
          writer.DumpMemberHeader(dumper.OutputFile(flags, false), *size, date)) {
        return writer.Ok(dumper.Dump(writer, flags, holder));
      }
    }
    return false;
  }

  std::string outfile = dumper.OutputFile(flags);
  fbl::unique_fd fd = CreateOutputFile(outfile);
  if (!fd) {
    return false;
  }
  Writer writer{std::move(fd), std::move(outfile), flags.zstd};
  dumper.ClockIn(flags);
  return writer.Ok(dumper.Collect(flags, true, holder) && dumper.Dump(writer, flags, holder));
}

// "Dump" a job tree by actually just making separate dumps of each process.
// We only use the JobDumper to find the processes and/or children.
bool WriteManyCoreFiles(JobDumper dumper, const Flags& flags, const zxdump::TaskHolder& holder) {
  bool ok = true;

  if (flags.collect_job_processes) {
    if (auto result = dumper->CollectProcesses(); result.is_error()) {
      dumper.Error(result.error_value());
      ok = false;
    } else {
      for (auto& [pid, process] : result.value().get()) {
        ok = WriteDump(ProcessDumper{process}, flags, holder) && ok;
      }
    }
  }

  if (flags.collect_job_children) {
    if (auto result = dumper->CollectChildren(); result.is_error()) {
      dumper.Error(result.error_value());
      ok = false;
    } else {
      for (auto& [koid, job] : result.value().get()) {
        ok = WriteManyCoreFiles(JobDumper{job}, flags, holder) && ok;
      }
    }
  }

  return ok;
}

enum class RemarksType { kText, kJson, kBinary };

std::optional<Remarks> ParseRemarks(const char* arg, RemarksType type) {
  Remarks result;

  if (const char* eq = strchr(arg, '=')) {
    result.name = {arg, static_cast<size_t>(eq - arg)};
    arg = eq + 1;
  } else {
    if (type == RemarksType::kBinary) {
      std::cerr << "--remarks-raw requires NAME= prefix" << std::endl;
      return std::nullopt;
    }
    result.name = kDefaultRemarksName;
  }

  switch (type) {
    case RemarksType::kText:
      result.name += ".txt"sv;
      break;
    case RemarksType::kJson:
      result.name += ".json"sv;
      break;
    case RemarksType::kBinary:
      break;
  }

  constexpr auto error = [](const char* filename) -> std::ostream& {
    if (filename) {
      std::cerr << filename << ": "sv;
    }
    return std::cerr;
  };

  auto empty_remarks = [error](const char* filename = nullptr) {
    error(filename) << "empty remarks not allowed"sv << std::endl;
    return std::nullopt;
  };

  auto parse_json = [error, &result](auto&& stream, const char* filename = nullptr) {
    rapidjson::Reader reader;
    rapidjson::Writer<rapidjson::StringBuffer> writer(*result.data);
    if (!reader.Parse(stream, writer)) {
      error(filename) << "cannot parse JSON at offset " << reader.GetErrorOffset() << ": "
                      << GetParseError_En(reader.GetParseErrorCode()) << std::endl;
      return false;
    }
    return true;
  };

  if (arg[0] == '@') {
    // Read the response file.
    const char* filename = &arg[1];
    FILE* f = fopen(filename, "r");
    if (!f) {
      perror(filename);
      return std::nullopt;
    }
    auto close_f = fit::defer([f]() { fclose(f); });

    if (type == RemarksType::kJson) {
      char buffer[BUFSIZ];
      rapidjson::FileReadStream stream(f, buffer, sizeof(buffer));
      if (!parse_json(stream, filename)) {
        return std::nullopt;
      }
    } else {
      int c;
      while ((c = getc(f)) != EOF) {
        result.data->Put(static_cast<char>(c));
      }
      if (ferror(f)) {
        perror(filename);
        return std::nullopt;
      }
      if (result.data->GetSize() == 0) {
        return empty_remarks(filename);
      }
    }
  } else if (type == RemarksType::kJson) {
    if (!parse_json(rapidjson::StringStream(arg))) {
      return std::nullopt;
    }
  } else {
    std::string_view data{arg};
    if (data.empty()) {
      return empty_remarks();
    }
    data.copy(result.data->Push(data.size()), data.size());
  }

  return result;
}

constexpr const char kOptString[] = "hlo:OzamtcpJjfDUsSkKr:q:B:Rd:L";
constexpr const option kLongOpts[] = {
    {"help", no_argument, nullptr, 'h'},                 //
    {"limit", required_argument, nullptr, 'l'},          //
    {"output-prefix", required_argument, nullptr, 'o'},  //
    {"streaming", no_argument, nullptr, 'O'},            //
    {"zstd", no_argument, nullptr, 'z'},                 //
    {"exclude-memory", no_argument, nullptr, 'm'},       //
    {"no-threads", no_argument, nullptr, 't'},           //
    {"no-children", no_argument, nullptr, 'c'},          //
    {"no-processes", no_argument, nullptr, 'p'},         //
    {"jobs", no_argument, nullptr, 'J'},                 //
    {"job-archive", no_argument, nullptr, 'j'},          //
    {"flat-job-archive", no_argument, nullptr, 'f'},     //
    {"no-date", no_argument, nullptr, 'D'},              //
    {"date", no_argument, nullptr, 'U'},                 //
    {"system", no_argument, nullptr, 's'},               //
    {"system-recursive", no_argument, nullptr, 'S'},     //
    {"kernel", no_argument, nullptr, 'k'},               //
    {"kernel-recursive", no_argument, nullptr, 'K'},     //
    {"remarks", required_argument, nullptr, 'r'},        //
    {"remarks-json", required_argument, nullptr, 'q'},   //
    {"remarks-raw", required_argument, nullptr, 'B'},    //
    {"remarks-recursive", no_argument, nullptr, 'R'},    //
    {"root-job", no_argument, nullptr, 'a'},             //
    {"dump-file", required_argument, nullptr, 'd'},      //
    {"live", no_argument, nullptr, 'L'},                 //
    {nullptr, no_argument, nullptr, 0},                  //
};

}  // namespace

int main(int argc, char** argv) {
  Flags flags;
  bool streaming = false;
  const char* output_argument = nullptr;
  bool allow_jobs = false;
  constexpr auto handle_process = WriteDump<ProcessDumper>;
  auto handle_job = WriteManyCoreFiles;
  CommandLineHelper cli;

  auto usage = [&](int status = EXIT_FAILURE) {
    std::cerr << "Usage: " << argv[0] << R"""( [SWITCHES...] PID...

    --help, -h                         print this message
    --output-prefix=PREFIX, -o PREFIX  write <PREFIX><PID>, not core.<PID>
    --streaming, -O                    write streaming output
    --zstd, -z                         compress output files with zstd -11
    --limit=BYTES, -l BYTES            truncate output to BYTES per process
    --exclude-memory, -M               exclude all process memory from dumps
    --no-threads, -t                   collect only memory, threads left to run
    --jobs, -J                         allow PIDs to be job KOIDs instead
    --job-archive, -j                  write job archives, not process dumps
    --flat-job-archive, -f             write flattened job archives
    --no-children, -c                  don't recurse to child jobs
    --no-processes, -p                 don't dump processes found in jobs
    --no-date, -D                      don't record dates in dumps
    --date, -U                         record dates in dumps (default)
    --system, -s                       include system-wide information
    --system-recursive, -S             ... repeated in each child dump
    --kernel, -k                       include privileged kernel information
    --kernel-recursive, -K             ... repeated in each child dump
    --remarks=REMARKS, -r REMARKS      add dump remarks (UTF-8 text)
    --remarks-json=REMARKS, -q REMARKS add dump remarks (JSON)
    --remarks-raw=REMARKS, -B REMARKS  add dump remarks (raw binary)
    --remarks-recursive, -R            repeat dump remarks in each child dump
    --root-job, -a                     dump the root job
    --dump-file=FILE, -d FILE          read a previous dump file
    --live, -L                         use live data from the running system

By default, each PID must be the KOID of a process.

With --jobs, the KOID of a job is allowed.  Each process gets a separate dump
named for its individual PID.

With --job-archive, the KOID of a job is allowed.  Each job is dumped into a
job archive named <PREFIX><KOID>.a instead of producing per-process dump files.
If child jobs are dumped they become `core.<KOID>.a` archive members that are
themselves job archives.

With --no-children, don't recurse into child jobs of a job.
With --no-process, don't dump processes within a job, only its child jobs.
Using --no-process with --jobs rather than --job-archive means no dumps are
produced from job KOIDs at all, but valid job KOIDs are ignored rather than
causing errors.

REMARKS can be `NAME=@FILE` to read the remarks from the file, or `NAME=TEXT`
to use the literal text in the argument; just `@FILE` or just `TEXT` is like
`remarks=@FILE` or `remarks=TEXT`.  All text is expected to be in UTF-8.
Text for --remarks-json must parse as valid JSON but no particular schema is
expected; it is dumped as compact canonical UTF-8 JSON text.

With `--raw-remarks` (-B), REMARKS is still `NAME=CONTENTS` or `NAME=@FILE`,
but the contents are uninterpreted binary and `NAME` is expected to have its
own suffix rather than having `.txt` or `.json` appended.  Note that since
CONTENTS cannot have embedded NUL characters, using `@FILE` for binary data is
always recommended.

Each argument is dumped synchronously before processing the next argument.
Errors dumping each process are reported and cause a failing exit status at
the end of the run, but do not prevent additional processes from being dumped.
Without --no-threads, each process is held suspended while being dumped.
Processes within a job are dumped serially.  When dumping a child job inside a
job archive, all processes inside that whole subtree are held suspended until
the whole child job archive is dumped.

With --flat-job-archive, child job archives inside a job archive are instead
"stub" job archives that only describe the job itself.  A child job's process
and (grand)child job dumps are all included directly in the outer "flat" job
archive.  In this mode, only one process is held suspended at a time.

Jobs are always dumped while they continue to run and may omit new processes
or child jobs created after the dump collection begins.  Job dumps may report
process or child job KOIDs that were never dumped if they died during
collection.

With --root-job (-a), dump the root job.  Without --no-children, that means
dumping every job on the system; and without --no-process, it means dumping
every process on the system.  Doing this without --no-threads may deadlock
essential services.  PID arguments are not allowed with --root-job unless
--no-children is also given, since they would always be redundant.

With --streaming (-O), a single contiguous output stream is written, usually to
stdout.  If --output=FILE (-o FILE) is given along with --streaming (-O), then
it names a single output file to write in place of stdout rather than a prefix.
When there is a single PID argument or just the --root-job (-a) switch, then
the output stream is a single ELF core file or a single job archive file.  When
there are multiple PID arguments, or multiple separate single-process dumps
under --jobs (-J), or a fake root job from postportem data that doesn't form a
single job tree, the output stream is a simple archive that contains the
individual ELF core file or job archive file for each PID argument as member
files with the names used by default (core.<PID> or core.<PID>.a).  Readers
treat such an archive just like the collection of separate dump files.

By default, data for dumps is drawn from the running system.  Of course, this
only works on Fuchsia.  One or more --dump-file (-d) switches can be given to
read old postmortem data instead.  This allows, for example, extracting a
subset of the jobs or processes from the original dump set; merging multiple
dumps into one job archive; adding dump remarks; changing configuration details
to dump a subset of the original postmortem data; etc.  If no --dump-file (-d)
switches are given, then --live (-L) is the default.  Using --live (-L)
explicitly in combination with dump files is allowed, but the results may be
confusing either if the dumps are not from the current running system or if a
task found in the postmortem data is also still alive on the running system.
)""";
    return status;
  };

  while (true) {
    switch (getopt_long(argc, argv, kOptString, kLongOpts, nullptr)) {
      case -1:
        // This ends the loop.  All other cases continue (or return).
        break;

      case 'D':
        flags.record_date = false;
        continue;

      case 'U':
        flags.record_date = true;
        continue;

      case 'o':
        flags.output_prefix = optarg;
        output_argument = optarg;
        continue;

      case 'O':
        streaming = true;
        continue;

      case 'l': {
        char* p;
        flags.limit = strtoul(optarg, &p, 0);
        if (*p != '\0') {
          return usage();
        }
        continue;
      }

      case 'm':
        flags.dump_memory = false;
        continue;

      case 't':
        flags.collect_threads = false;
        continue;

      case 'f':
        flags.flatten_jobs = true;
        [[fallthrough]];

      case 'j':
        handle_job = WriteDump<JobDumper>;
        [[fallthrough]];

      case 'J':
        allow_jobs = true;
        continue;

      case 'c':
        flags.collect_job_children = false;
        continue;

      case 'p':
        flags.collect_job_processes = false;
        continue;

      case 'S':
        flags.repeat_system = true;
        [[fallthrough]];

      case 's':
        flags.collect_system = true;
        continue;

      case 'K':
        flags.repeat_kernel = true;
        [[fallthrough]];

      case 'k':
        flags.collect_kernel = true;
        continue;

      case 'r':
        if (auto note = ParseRemarks(optarg, RemarksType::kText)) {
          flags.remarks.push_back(std::move(*note));
          continue;
        }
        return usage();

      case 'R':
        flags.repeat_remarks = true;
        continue;

      case 'q':
        if (auto note = ParseRemarks(optarg, RemarksType::kJson)) {
          flags.remarks.push_back(std::move(*note));
          continue;
        }
        return usage();

      case 'B':
        if (auto note = ParseRemarks(optarg, RemarksType::kBinary)) {
          flags.remarks.push_back(std::move(*note));
          continue;
        }
        return usage();

      case 'a':
        cli.RootJobArgument();
        continue;

      case 'z':
        flags.zstd = true;
        continue;

      case 'd':
        cli.DumpFileArgument(optarg);
        continue;

      case 'L':
        cli.LiveArgument();
        continue;

      case 'h':
        return usage(EXIT_SUCCESS);

      default:
        return usage();
    }
    break;
  }

  cli.KoidArguments(argc, argv, optind, allow_jobs);

  cli.NeedInfoResource(flags.collect_kernel);
  cli.NeedSystem(flags.collect_system);

  if (cli.empty() && cli.ok()) {
    return usage();
  }

  constexpr auto is_job = [](zxdump::Task& task) { return task.type() == ZX_OBJ_TYPE_JOB; };

  auto call_dumper = [&](auto&& handle_task, auto dumper) {
    cli.Ok(handle_task(std::move(dumper), flags, cli.holder()));
  };

  auto dump_one_task = [&](zxdump::Task& task) {
    if (is_job(task)) {
      auto& job = static_cast<zxdump::Job&>(task);
      call_dumper(handle_job, JobDumper{job});
    } else {
      ZX_DEBUG_ASSERT(task.type() == ZX_OBJ_TYPE_PROCESS);
      auto& process = static_cast<zxdump::Process&>(task);
      call_dumper(handle_process, ProcessDumper{process});
    }
  };

  auto tasks = cli.take_tasks();

  if (streaming) {
    // There will be just one output stream with just one writer.
    fbl::unique_fd fd;
    std::string outname;
    if (output_argument) {
      outname = output_argument;
      fd = CreateOutputFile(outname);
      if (!fd) {
        return EXIT_FAILURE;
      }
    } else {
      fd.reset(STDOUT_FILENO);
    }

    // This gets the single writer passed down to all the dumpers.
    // When not streaming, each dumper makes its own writer.
    flags.streaming = std::make_unique<Writer>(std::move(fd), std::move(outname), flags.zstd);

    // If there will be more than one separate dump, switch to "streaming
    // archive" mode.  That is, if there are multiple separate KOID arguments
    // or the sole KOID is a job but under -J rather than -j.
    flags.streaming_archive =
        tasks.size() > 1 || (is_job(tasks.front()) && handle_job == WriteManyCoreFiles);

    if (flags.streaming_archive && !flags.streaming->StartArchive()) {
      return EXIT_FAILURE;
    }
  }

  while (!tasks.empty()) {
    zxdump::Task& task = tasks.front();
    tasks.pop();
    dump_one_task(task);
  }

  return cli.exit_status();
}
