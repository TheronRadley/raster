#include "raster_checksum.h"

#include <cstdint>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <iterator>
#include <vector>

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
    std::vector<char> bytes((std::istreambuf_iterator<char>(input)),
                            std::istreambuf_iterator<char>());
    const uint8_t *data = reinterpret_cast<const uint8_t *>(bytes.data());
    const uint64_t digest = raster_fnv1a64(data, bytes.size());
    std::cout << std::hex << std::nouppercase << std::setfill('0')
              << std::setw(16) << digest << '\n';
    return 0;
}
