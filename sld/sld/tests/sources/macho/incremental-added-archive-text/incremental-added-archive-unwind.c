#include <unwind.h>

struct incremental_added_archive_unwind_probe {
  void *expected_ip;
  int reached_resume;
};

static _Unwind_Reason_Code incremental_added_archive_trace(
    struct _Unwind_Context *context, void *argument) {
  struct incremental_added_archive_unwind_probe *probe = argument;
  int ip_before;
  if (_Unwind_GetIPInfo(context, &ip_before) == (uintptr_t)probe->expected_ip) {
    probe->reached_resume = 1;
    return _URC_END_OF_STACK;
  }
  return _URC_NO_REASON;
}

__attribute__((noinline, used)) int
incremental_added_archive_verify_unwind(void *expected_ip) {
  if (!expected_ip) {
    return 1;
  }

  struct incremental_added_archive_unwind_probe probe = {expected_ip, 0};
  _Unwind_Backtrace(incremental_added_archive_trace, &probe);
  return probe.reached_resume;
}
