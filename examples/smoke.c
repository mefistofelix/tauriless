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

int main(void) {
  Tauriless *runtime = NULL;
  int32_t status = tauriless_create(&runtime);
  if (status != TAURILESS_OK) return report_error("create", status);

  const char *json =
      "{\"id\":1,\"cmd\":\"plugin:webview|create_webview_window\","
      "\"payload\":{\"options\":{\"label\":\"smoke\","
      "\"title\":\"Smoke\",\"url\":\"index.html\","
      "\"visible\":false}}}";
  status = tauriless_send(runtime, (const uint8_t *)json, strlen(json));
  if (status != TAURILESS_OK) return report_error("create webview", status);

  TaurilessBuffer batch = {0};
  status = tauriless_drain(runtime, &batch);
  if (status != TAURILESS_OK) return report_error("drain", status);
  printf("%.*s\n", (int)batch.len,
         batch.data ? (const char *)batch.data : "");
  tauriless_buffer_free(batch.data, batch.len, batch.capacity);

  status = tauriless_destroy(runtime);
  if (status != TAURILESS_OK) return report_error("destroy", status);
  return 0;
}
