#ifndef TAURILESS_H
#define TAURILESS_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif
typedef struct Tauriless Tauriless;

enum TaurilessStatus {
  TAURILESS_OK = 0,
  TAURILESS_INVALID_ARGUMENT = 1,
  TAURILESS_ERROR = 2,
  TAURILESS_PANIC = 3
};

/* Create and use the runtime on the host's main OS thread. */
int32_t tauriless_create(Tauriless **out);

/* Execute one NUL-terminated UTF-8 JSON request; queue its result for run(). */
int32_t tauriless_send(Tauriless *runtime, const char *json);

/*
 * Run the native GUI event loop until it becomes idle after a wake, or until
 * timeout_ms expires. timeout_ms == 0 never waits. The borrowed JSON pointer
 * remains valid until the next run or destroy call. NULL indicates an error
 * available through tauriless_last_error().
 */
const char *tauriless_run(Tauriless *runtime, uint32_t timeout_ms);

int32_t tauriless_destroy(Tauriless *runtime);

/*
 * Borrow the calling thread's most recent NUL-terminated UTF-8 error. The
 * pointer remains valid until another ABI error occurs on that thread.
 */
const char *tauriless_last_error(void);

#ifdef __cplusplus
}
#endif

#endif
