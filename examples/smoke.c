#include "tauriless.h"

#include <stdio.h>
#include <windows.h>

static int report_error(const char *operation, int32_t status) {
  const char *error = tauriless_last_error();
  fprintf(stderr, "%s failed (%d): %s\n", operation, status,
          error ? error : "");
  return status;
}

int main(void) {
  Tauriless *runtime = NULL;
  int32_t status = tauriless_create(&runtime);
  if (status != TAURILESS_OK) return report_error("create", status);

  const char *json =
      "{\"id\":1,\"cmd\":\"plugin:webview|create_webview_window\","
      "\"payload\":{\"options\":{\"label\":\"smoke\","
      "\"title\":\"Smoke\",\"url\":\"data:text/html,smoke\","
      "\"visible\":false}}}";
  status = tauriless_send(runtime, json);
  if (status != TAURILESS_OK) return report_error("create webview", status);

  const char *batch = tauriless_drain(runtime);
  if (!batch) return report_error("drain", TAURILESS_ERROR);
  printf("%s\n", batch);

  status = tauriless_destroy(runtime);
  if (status != TAURILESS_OK) return report_error("destroy", status);
  return 0;
}
