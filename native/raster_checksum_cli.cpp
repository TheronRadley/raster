#include "raster_checksum.h"

#include <array>
#include <cstdint>
#include <fstream>
#include <iomanip>
#include <iostream>

int main(int argc, char **argv) {
    if (argc != 2) {
        std::cerr << "usage: raster-checksum <file>\n";
        return 2;
    }
    std::ifstream input(argv[1], std::ios::binary);
    if (!input) {
        std::cerr << "raster-checksum: cannot open " << argv[1] << '\n';
        return 1;
    }
    std::array<char, 64 * 1024> buffer{};
    uint64_t digest = raster_fnv1a64_init();
    while (input.read(buffer.data(), static_cast<std::streamsize>(buffer.size())) || input.gcount() > 0) {
        digest = raster_fnv1a64_update(
            digest, reinterpret_cast<const uint8_t *>(buffer.data()),
            static_cast<size_t>(input.gcount()));
    }
    if (input.bad() || !input.eof()) {
        std::cerr << "raster-checksum: cannot read " << argv[1] << '\n';
        return 1;
    }
    std::cout << std::hex << std::nouppercase << std::setfill('0')
              << std::setw(16) << digest << '\n';
    if (!std::cout) {
        std::cerr << "raster-checksum: cannot write digest\n";
        return 1;
    }
    return 0;
}
