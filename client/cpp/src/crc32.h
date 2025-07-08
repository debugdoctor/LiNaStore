#ifndef CRC32_H
#define CRC32_H

#include <vector>
#include <cstdint>

class CRC32 {
public:
    CRC32();
    ~CRC32();
    void update(std::vector<uint8_t> data);
    uint32_t finalize();
private:
    uint32_t crc;
    uint32_t table[256];
    void initTable();
};

#endif // CRC32_H