#ifndef LINA_CLIENT_H
#define LINA_CLIENT_H

/* Enable POSIX.1-2001 features for addrinfo, getaddrinfo, etc. */
/* Note: This must be defined before including any system headers */
#define _POSIX_C_SOURCE 200112L

#ifdef _WIN32
    #include <winsock2.h>
    #include <ws2tcpip.h>
    #include <time.h>
#else
    #include <sys/socket.h>
    #include <netinet/in.h>
    #include <unistd.h>
    #include <arpa/inet.h>
    #include <sys/uio.h>
    #include <errno.h>
    #include <time.h>
    #include <netdb.h>
    typedef int SOCKET;
    #define INVALID_SOCKET (SOCKET)(~0)
    #define SOCKET_ERROR (ssize_t)(~0)
#endif

#include <memory.h>
#include <stdbool.h>
#include "crc32.h"

#define bool uint8_t
#define true 1
#define false 0
#define MAX_MSG_LEN 4096
// Header: status(1) + ilen(1) + dlen(4) + checksum(4) = 10 bytes (before identifier)
#define LINA_HEADER_BASE_LENGTH 10

enum LiNaFlags {
    LINA_DELETE = 0xC0,
    LINA_WRITE = 0x80,
    LINA_AUTH = 0x60,
    LINA_READ = 0x40,
    LINA_COVER = 0x02,
    LINA_COMPRESS = 0x01,
    LINA_NONE = 0x00
};

typedef struct LiNaClient{
    SOCKET sock;
    struct sockaddr_in server;
    char* server_address;  // IP address or URL
    int server_port;
    // Token management
    char* session_token;
    uint64_t token_expires_at;
    // Cached credentials for auto-refresh
    char* cached_username;
    char* cached_password;
    // Auto-refresh settings
    bool auto_refresh;
    uint32_t refresh_buffer;  // Buffer time in seconds before expiration
} LiNaClient;

bool _connect(LiNaClient* client);
bool _disconnect(LiNaClient* client);

// Token management functions
bool is_token_expired(LiNaClient* client);
bool refresh_token_if_needed(LiNaClient* client, char* error_msg, size_t msg_len);
void cache_credentials(LiNaClient* client, char* username, char* password);
void clear_cached_credentials(LiNaClient* client);

typedef struct LiNaResult {
    bool status;
    union payload{
        char* data;
        char* message;
    } payload;
} LiNaResult;

typedef struct HandshakeResult {
    bool status;
    char* token;
    uint64_t expires_at;
    char* message;
} HandshakeResult;

LiNaResult uploadFile(LiNaClient* client, char* name, char* data, size_t data_len, uint8_t flags);
LiNaResult downloadFile(LiNaClient* client, char* name);
LiNaResult deleteFile(LiNaClient* client, char* name);
HandshakeResult handshake(LiNaClient* client, char* username, char* password, bool cache_credentials);
void freeResult(LiNaResult* result);
void freeHandshakeResult(HandshakeResult* result);

// Client initialization and cleanup
LiNaClient* init_client(const char* address, int port, bool auto_refresh, uint32_t refresh_buffer);
void cleanup_client(LiNaClient* client);

// Utility functions
uint8_t* to_array(uint64_t value, uint8_t length, bool little_endian);
uint64_t to_long(char* data, uint8_t length, bool little_endian);
#endif