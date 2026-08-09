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

/* Execute one NUL-terminated UTF-8 JSON request; queue its result for drain(). */
int32_t tauriless_send(Tauriless *runtime, const char *json);

/*
 * Pump one non-blocking GUI iteration and borrow a NUL-terminated UTF-8 JSON
 * batch. The pointer remains valid until the next drain or destroy call. A
 * NULL result indicates an error available through tauriless_last_error().
 */
const char *tauriless_drain(Tauriless *runtime);

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
