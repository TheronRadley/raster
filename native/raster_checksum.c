#include "raster_checksum.h"

#define FNV1A64_OFFSET_BASIS UINT64_C(0xcbf29ce484222325)
#define FNV1A64_PRIME UINT64_C(0x100000001b3)

RASTER_API uint64_t raster_fnv1a64_init(void) {
    return FNV1A64_OFFSET_BASIS;
}

RASTER_API uint64_t raster_fnv1a64_update(uint64_t state, const uint8_t *data, size_t length) {
    if (data == NULL) {
        return state;
    }
    for (size_t i = 0; i < length; ++i) {
        state ^= (uint64_t)data[i];
        state *= FNV1A64_PRIME;
    }
    return state;
}

RASTER_API uint64_t raster_fnv1a64(const uint8_t *data, size_t length) {
    return raster_fnv1a64_update(raster_fnv1a64_init(), data, length);
}

RASTER_API uint32_t raster_checksum_abi_version(void) {
    return UINT32_C(2);
}
