#ifndef LINA_CLIENT_H
#define LINA_CLIENT_H

#ifdef _WIN32
    #include <winsock2.h>
    #include <ws2tcpip.h>
#else
    #include <sys/socket.h>
    #include <cstring>
    #include <netinet/in.h>
    #include <unistd.h>
    #include <arpa/inet.h>
    #include <sys/uio.h>
    #include <netdb.h>
    typedef int SOCKET;
    #define INVALID_SOCKET (SOCKET)(~0)
#endif

#define LINA_NAME_MAX_LENGTH 255
#define LINA_HEADER_BASE_LENGTH 10  // flags(1) + ilen(1) + dlen(4) + checksum(4)

#include <string>
#include <vector>
#include <memory>
#include <sstream>
#include "crc32.h"

// Forward declaration of HandshakeResult
struct HandshakeResult {
    bool status;
    std::string token;
    uint64_t expires_at;
    std::string message;
};

class LiNaClient {
public:
    LiNaClient(std::string address, int port, bool auto_refresh = true, uint32_t refresh_buffer = 300);
    ~LiNaClient();

    bool uploadFile(std::string name, std::vector<char> data, uint8_t flags);
    std::vector<char> downloadFile(std::string name);
    bool deleteFile(std::string name);
    HandshakeResult handshake(std::string username, std::string password, bool cache_credentials = true);

    void check_sendv(const std::vector<std::pair<const void*, size_t>>& buffers, const char* context);
    void check_recv(char* buf, size_t len, const char* context);

    // Token management functions
    bool isTokenExpired() const;
    bool refreshTokenIfNeeded();
    void cacheCredentials(std::string username, std::string password);
    void clearCachedCredentials();
    struct TokenInfo {
        bool has_token;
        bool is_expired;
        uint64_t expires_at;
        uint64_t expires_in;
        bool has_cached_credentials;
    };
    TokenInfo getTokenInfo() const;

    enum LiNaFlags {
        LINA_DELETE = 0xC0,
        LINA_WRITE = 0x80,
        LINA_AUTH = 0x60,
        LINA_READ = 0x40,
        LINA_COVER = 0x02,
        LINA_COMPRESS = 0x01,
        LINA_NONE = 0x00
    };

private:
    bool connect();
    bool disconnect();

    SOCKET sock;
    struct sockaddr_in server_addr;
    std::string server_address;  // IP address or hostname
    
    // Token management
    std::string session_token;
    uint64_t token_expires_at;
    // Cached credentials for auto-refresh
    std::string cached_username;
    std::string cached_password;
    // Auto-refresh settings
    bool auto_refresh;
    uint32_t refresh_buffer;
};

class LiNaClientException : public std::exception {
public:
    LiNaClientException(std::string message) : message(message) {}
    const char* what() const throw() { return message.c_str(); }
private:
    std::string message;
};

// Utility functions
template <typename T>
std::vector<T> to_vector(uint64_t value, uint8_t length, bool little_endian = true);
uint64_t to_long(std::vector<uint8_t> data, uint8_t length, bool little_endian = true);

#endif // LINA_CLIENT_H