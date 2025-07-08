#ifndef LINA_CLIENT_H
#define LINA_CLIENT_H

#ifdef _WIN32
    #include <winsock2.h>
    #include <ws2tcpip.h>
#else
    #include <sys/socket.h>
    #include <netinet/in.h>
    #include <unistd.h>
    #include <arpa/inet.h>
    #include <sys/uio.h>
    #include <errno.h>
    typedef int SOCKET;
    #define INVALID_SOCKET (SOCKET)(~0)
    #define SOCKET_ERROR (ssize_t)(~0)
#endif

#include <memory.h>
#include "crc32.h"

#define bool uint8_t
#define true 1
#define false 0
#define LINA_NAME_LENGTH 255
#define MAX_MSG_LEN 255
#define LINA_HEADER_LENGTH 0x108

enum LiNaFlags {
    LINA_DELETE = 0xC0,
    LINA_WRITE = 0x80,
    LINA_READ = 0x40,
    LINA_COVER = 0x02,
    LINA_COMPRESS = 0x01,
    LINA_NONE = 0x00
};

typedef struct LiNaClient{
    SOCKET sock;
    struct sockaddr_in server;
} LiNaClient;

bool _connect(LiNaClient* client);
bool _disconnect(LiNaClient* client);

typedef struct LiNaResult {
    bool status;
    union payload{
        char* data;
        char* message;
    } payload;
} LiNaResult;

LiNaResult uploadFile(LiNaClient* client, char* name, char* data, size_t data_len, uint8_t flags);
LiNaResult downloadFile(LiNaClient* client, char* name);
LiNaResult deleteFile(LiNaClient* client, char* name);
void freeResult(LiNaResult* result);

// Utility functions
uint8_t* to_array(uint64_t value, uint8_t length, bool little_endian);
uint64_t to_long(char* data, uint8_t length, bool little_endian);
#endif