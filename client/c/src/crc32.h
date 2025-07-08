#ifndef CRC32_H
#define CRC32_H

#include <stdint.h>
#include <memory.h>

typedef struct CRC32
{
    uint32_t value;
    uint32_t table[256];
} CRC32;

CRC32 CRC32_init();
void CRC32_update(CRC32* crc32, uint8_t data, size_t len);
uint64_t CRC32_finalize(CRC32* crc32);
#endif