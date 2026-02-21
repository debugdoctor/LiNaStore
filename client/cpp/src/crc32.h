#ifndef CRC32_H
#define CRC32_H

#include <vector>
#include <cstdint>
#include <cstddef>

class CRC32 {
public:
    CRC32();
    
    // Destructor - compatible with both C++11 and older versions
#if __cplusplus >= 201103L
    ~CRC32() = default;
#else
    ~CRC32() {}
#endif
    
    // Optimized update methods - pass by reference to avoid copy
    void update(const std::vector<uint8_t>& data);
    void update(const uint8_t* data, size_t len);
    
    uint32_t finalize();
    
    // Reset for reuse
    void reset();

private:
    uint32_t crc;
    
    // Static table - shared across all instances
    static uint32_t table[256];
    static bool table_initialized;
    
    static void initTable();
    
    // Hardware CRC32 support (SSE4.2)
#if defined(__SSE4_2__) || defined(__CRC32__)
    static uint32_t crc32_hw(const uint8_t* data, size_t len, uint32_t crc);
#endif
    
    // ARM CRC32 support
#if defined(__aarch64__) && defined(__ARM_FEATURE_CRC32)
    static uint32_t crc32_arm(const uint8_t* data, size_t len, uint32_t crc);
#endif
};

#endif // CRC32_H
