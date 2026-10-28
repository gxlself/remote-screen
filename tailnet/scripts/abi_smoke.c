#include "rs_tailnet.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(name, condition) do { \
  if (!(condition)) { fprintf(stderr, "FAIL %s (line %d)\n", name, __LINE__); return 1; } \
  printf("ok %s\n", name); \
} while (0)

int main(void) {
  char err[256] = {0};
  char tiny = '!';
  CHECK("abi_version", rs_ts_abi_version() == RS_TS_ABI_VERSION);
  int64_t need = rs_ts_status(&tiny, 1);
  CHECK("status_size", need > 1 && tiny == '!');
  char *status = malloc((size_t)need + 1);
  CHECK("status_allocation", status != NULL);
  int64_t size = rs_ts_status(status, (size_t)need);
  CHECK("status_length", size == need);
  status[size] = '\0';
  CHECK("status_stopped", strstr(status, "\"state\":\"Stopped\"") != NULL);
  free(status);
  CHECK("events_closed", rs_ts_next_event(err, sizeof(err), 0) == RS_TS_ERR_CLOSED);
  CHECK("udp_before_start", rs_ts_dial_udp("100.64.0.1", 7400, 100, err, sizeof(err)) == RS_TS_ERR_STATE && err[0] != '\0');
  err[0] = '\0';
  CHECK("invalid_config", rs_ts_start("{", err, sizeof(err)) == RS_TS_ERR_INVALID && err[0] != '\0');
  CHECK("stop", rs_ts_stop() == RS_TS_OK);
  /* Exercise NULL, zero-capacity, and truncation at the actual C boundary. */
  CHECK("null_config", rs_ts_start(NULL, NULL, 0) == RS_TS_ERR_INVALID);
  CHECK("null_udp", rs_ts_dial_udp(NULL, 1, 0, NULL, 0) == RS_TS_ERR_INVALID);
  CHECK("null_tcp", rs_ts_dial_tcp(NULL, 1, 0, NULL, 0) == RS_TS_ERR_INVALID);
  tiny = '!';
  CHECK("error_truncation", rs_ts_start("{", &tiny, 1) == RS_TS_ERR_INVALID && tiny == '\0');
  CHECK("login_before_start", rs_ts_login(NULL, 0) == RS_TS_ERR_STATE);
  CHECK("logout_before_start", rs_ts_logout(NULL, 0) == RS_TS_ERR_STATE);
  CHECK("rebind_before_start", rs_ts_rebind() == RS_TS_ERR_STATE);
  CHECK("null_status", rs_ts_status(NULL, 0) == need);
  CHECK("events_negative_timeout", rs_ts_next_event(NULL, 0, -1) == RS_TS_ERR_CLOSED);
  CHECK("stop_idempotent", rs_ts_stop() == RS_TS_OK);
  return 0;
}
