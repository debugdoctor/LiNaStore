#include "crc32.h"

CRC32 CRC32_init(){
    CRC32 ctx;
    ctx.value = 0xFFFFFFFF;
    memset(ctx.table, 0, sizeof(ctx.table));

    uint8_t index = 0, bit;
    do {
        ctx.table[index] = index;
        for(bit = 8; bit; bit--) ctx.table[index] = ctx.table[index] & 1 ? (ctx.table[index] >> 1) ^ 0xEDB88320 : ctx.table[index] >> 1;
    } while(++index);

    return ctx;
}

void CRC32_update(CRC32* crc32, uint8_t* data, size_t len) {
    uint8_t byte = 0;
    for (size_t i = 0; i < len; i++) {
        byte = data[i];
        crc32->value = (crc32->value >> 8) ^ crc32->table[(crc32->value & 0xFF) ^ byte];
    }
}

uint64_t CRC32_finalize(CRC32* crc32) {
    uint32_t result = crc32->value;
    crc32->value = 0xffffffff;
    return result;
}