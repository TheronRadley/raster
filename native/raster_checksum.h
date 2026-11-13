#ifndef RASTER_CHECKSUM_H
#define RASTER_CHECKSUM_H

#include <stddef.h>
#include <stdint.h>

/*
 * Stable C ABI for Raster's verification helpers. The editor itself is Rust;
 * this ABI lets Python (ctypes) and the C++ CLI share one checksum implementation.
 * RASTER_CHECKSUM_BUILD is defined only when compiling the shared library.
 */
#if defined(_WIN32)
#  if defined(RASTER_CHECKSUM_BUILD)
#    define RASTER_API __declspec(dllexport)
#  else
#    define RASTER_API __declspec(dllimport)
#  endif
#elif defined(__GNUC__)
#  define RASTER_API __attribute__((visibility("default")))
#else
#  define RASTER_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

/* FNV-1a 64-bit over `length` bytes. A null pointer with length 0 is valid. */
RASTER_API uint64_t raster_fnv1a64(const uint8_t *data, size_t length);

/* Bump when the exported ABI changes; verification tooling checks this value. */
RASTER_API uint32_t raster_checksum_abi_version(void);

#ifdef __cplusplus
}
#endif

#endif /* RASTER_CHECKSUM_H */
