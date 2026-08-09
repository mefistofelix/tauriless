#include "tauriless.h"

#include <stdio.h>
#include <string.h>
#include <windows.h>

static int report_error(const char *operation, int32_t status) {
  TaurilessBuffer error = {0};
  tauriless_last_error(&error);
  fprintf(stderr, "%s failed (%d): %.*s\n", operation, status,
          (int)error.len, error.data ? (const char *)error.data : "");
  tauriless_buffer_free(error.data, error.len, error.capacity);
  return status;
}

static int send_json(Tauriless *runtime, const char *json) {
  int32_t status = tauriless_send(runtime, (const uint8_t *)json, strlen(json));
  if (status != TAURILESS_OK) return report_error("send", status);
  return 0;
}

static int wait_for_result(Tauriless *runtime, int id) {
  char needle[32];
  sprintf_s(needle, sizeof(needle), "\"id\":%d", id);
  for (int i = 0; i < 120; ++i) {
    TaurilessBuffer batch = {0};
    int32_t status = tauriless_drain(runtime, &batch);
    if (status != TAURILESS_OK) return report_error("drain", status);

    int found = 0;
    int ok = 0;
    if (batch.data) {
      const char *json = (const char *)batch.data;
      printf("%.*s\n", (int)batch.len, json);
      found = strstr(json, needle) != NULL;
      ok = strstr(json, "\"ok\":true") != NULL;
    }
    tauriless_buffer_free(batch.data, batch.len, batch.capacity);
    if (found) return ok ? 0 : 20 + id;
    Sleep(16);
  }
  return 40 + id;
}

int main(void) {
  Tauriless *runtime = NULL;
  int32_t status = tauriless_create(&runtime);
  if (status != TAURILESS_OK) return report_error("create", status);

  if (send_json(runtime,
                "{\"id\":1,\"cmd\":\"plugin:app|name\",\"payload\":{}}") ||
      wait_for_result(runtime, 1))
    return 10;

  if (send_json(runtime,
                "{\"id\":2,\"cmd\":\"plugin:webview|create_webview_window\","
                "\"payload\":{\"options\":{\"label\":\"smoke\","
                "\"title\":\"Smoke\",\"url\":\"index.html\","
                "\"visible\":false}}}") ||
      wait_for_result(runtime, 2))
    return 11;

  /* Dependent commands are sent only after the create result was drained. */
  if (send_json(runtime,
                "{\"id\":3,\"cmd\":\"plugin:window|destroy\","
                "\"payload\":{\"label\":\"smoke\"}}") ||
      wait_for_result(runtime, 3))
    return 12;

  status = tauriless_destroy(runtime);
  if (status != TAURILESS_OK) return report_error("destroy", status);
  return 0;
}
