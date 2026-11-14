#include "raster_checksum.h"

#define FNV1A64_OFFSET_BASIS UINT64_C(0xcbf29ce484222325)
#define FNV1A64_PRIME UINT64_C(0x100000001b3)

RASTER_API uint64_t raster_fnv1a64(const uint8_t *data, size_t length) {
    uint64_t hash = FNV1A64_OFFSET_BASIS;
    if (data == NULL) {
        return hash;
    }
    for (size_t i = 0; i < length; ++i) {
        hash ^= (uint64_t)data[i];
        hash *= FNV1A64_PRIME;
    }
    return hash;
}

RASTER_API uint32_t raster_checksum_abi_version(void) {
    return UINT32_C(1);
}
