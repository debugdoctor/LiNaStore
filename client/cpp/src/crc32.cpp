#include "crc32.h"

CRC32::CRC32() {
    crc = 0xFFFFFFFF; // Initial value for CRC32
    initTable();
}

CRC32::~CRC32() {
    // No dynamic memory to free
}

void CRC32::initTable() {
    uint8_t index = 0, bit;
    do {
        table[index] = index;
        for(bit = 8; bit; bit--) table[index] = table[index] & 1 ? (table[index] >> 1) ^ 0xEDB88320 : table[index] >> 1;
    } while(++index);
}

void CRC32::reset() {
    crc = 0xFFFFFFFF;
}

void CRC32::update(std::vector<uint8_t> data) { 
    for (uint8_t byte : data) {
        crc = (crc >> 8) ^ table[(crc & 0xFF) ^ byte];
    }
}

uint32_t CRC32::get_value() { 
    return ~crc;
}