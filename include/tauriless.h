#ifndef TAURILESS_H
#define TAURILESS_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif
typedef struct Tauriless Tauriless;

typedef struct TaurilessBuffer {
  uint8_t *data;
  size_t len;
  size_t capacity;
} TaurilessBuffer;

enum TaurilessStatus {
  TAURILESS_OK = 0,
  TAURILESS_INVALID_ARGUMENT = 1,
  TAURILESS_ERROR = 2,
  TAURILESS_PANIC = 3
};

/* Create and use the runtime on the host's main OS thread. */
int32_t tauriless_create(Tauriless **out);

/* Execute one UTF-8 JSON request; its result is queued for drain(). */
int32_t tauriless_send(Tauriless *runtime, const uint8_t *json, size_t len);

/* Pump one non-blocking GUI iteration and return a UTF-8 JSON message batch. */
int32_t tauriless_drain(Tauriless *runtime, TaurilessBuffer *out);

int32_t tauriless_destroy(Tauriless *runtime);

/* Copy the calling thread's most recent error into an owned buffer. */
int32_t tauriless_last_error(TaurilessBuffer *out);

/* Free buffers returned by tauriless_drain/tauriless_last_error. */
void tauriless_buffer_free(void *data, size_t len, size_t capacity);

#ifdef __cplusplus
}
#endif

#endif
