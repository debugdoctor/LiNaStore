#include "crc32.h"
#include <cstring>

// Static members initialization
uint32_t CRC32::table[256];
bool CRC32::table_initialized = false;

CRC32::CRC32() : crc(0xFFFFFFFF) {
    if (!table_initialized) {
        initTable();
        table_initialized = true;
    }
}

void CRC32::initTable() {
    for (uint32_t i = 0; i < 256; i++) {
        uint32_t crc = i;
        for (int bit = 8; bit; bit--) {
            crc = (crc & 1) ? (crc >> 1) ^ 0xEDB88320 : crc >> 1;
        }
        table[i] = crc;
    }
}

void CRC32::reset() {
    crc = 0xFFFFFFFF;
}

// Optimized update with const reference
void CRC32::update(const std::vector<uint8_t>& data) {
    update(data.data(), data.size());
}

void CRC32::update(const uint8_t* data, size_t len) {
    if (len == 0 || data == NULL) return;
    
#if defined(__SSE4_2__) || defined(__CRC32__)
    // Use hardware CRC32 on x86/x64 with SSE4.2
    crc = crc32_hw(data, len, crc);
#elif defined(__aarch64__) && defined(__ARM_FEATURE_CRC32)
    // Use hardware CRC32 on ARM64
    crc = crc32_arm(data, len, crc);
#else
    // Software fallback - loop unrolling for better performance
    const uint8_t* ptr = data;
    const uint8_t* end = data + len;
    
    // Process 8 bytes at a time (loop unrolling)
    while (ptr + 8 <= end) {
        crc = table[(crc ^ ptr[0]) & 0xFF] ^ (crc >> 8);
        crc = table[(crc ^ ptr[1]) & 0xFF] ^ (crc >> 8);
        crc = table[(crc ^ ptr[2]) & 0xFF] ^ (crc >> 8);
        crc = table[(crc ^ ptr[3]) & 0xFF] ^ (crc >> 8);
        crc = table[(crc ^ ptr[4]) & 0xFF] ^ (crc >> 8);
        crc = table[(crc ^ ptr[5]) & 0xFF] ^ (crc >> 8);
        crc = table[(crc ^ ptr[6]) & 0xFF] ^ (crc >> 8);
        crc = table[(crc ^ ptr[7]) & 0xFF] ^ (crc >> 8);
        ptr += 8;
    }
    
    // Process remaining bytes
    while (ptr < end) {
        crc = table[(crc ^ *ptr) & 0xFF] ^ (crc >> 8);
        ptr++;
    }
#endif
}

uint32_t CRC32::finalize() {
    uint32_t result = ~crc;
    crc = 0xFFFFFFFF; // Reset for potential reuse
    return result;
}

#if defined(__SSE4_2__) || defined(__CRC32__)
#ifdef _MSC_VER
#include <nmmintrin.h>
#else
#include <x86intrin.h>
#endif

uint32_t CRC32::crc32_hw(const uint8_t* data, size_t len, uint32_t crc_init) {
    uint64_t crc = crc_init;
    const uint64_t* ptr64 = (const uint64_t*)(const void*)data;
    size_t len64 = len / 8;
    
    // Process 8 bytes at a time using hardware instruction
    for (size_t i = 0; i < len64; i++) {
        crc = _mm_crc32_u64(crc, ptr64[i]);
    }
    
    // Process remaining bytes
    const uint8_t* ptr = (const uint8_t*)(ptr64 + len64);
    size_t remaining = len - (len64 * 8);
    for (size_t i = 0; i < remaining; i++) {
        crc = _mm_crc32_u8((uint32_t)crc, ptr[i]);
    }
    
    return (uint32_t)crc;
}
#endif

#if defined(__aarch64__) && defined(__ARM_FEATURE_CRC32)
#include <arm_acle.h>

uint32_t CRC32::crc32_arm(const uint8_t* data, size_t len, uint32_t crc_init) {
    uint32_t crc = crc_init;
    const uint64_t* ptr64 = (const uint64_t*)(const void*)data;
    size_t len64 = len / 8;
    
    // Process 8 bytes at a time using ARM CRC32 instruction
    for (size_t i = 0; i < len64; i++) {
        crc = __crc32cd(crc, ptr64[i]);
    }
    
    // Process remaining 4 bytes
    const uint32_t* ptr32 = (const uint32_t*)(const void*)((const uint8_t*)ptr64 + len64 * 8);
    if (len % 8 >= 4) {
        crc = __crc32cw(crc, *ptr32);
        ptr32 = (const uint32_t*)((const uint8_t*)ptr32 + 4);
    }
    
    // Process remaining bytes
    const uint8_t* ptr = (const uint8_t*)ptr32;
    size_t remaining = len % 4;
    for (size_t i = 0; i < remaining; i++) {
        crc = __crc32cb(crc, ptr[i]);
    }
    
    return crc;
}
#endif
