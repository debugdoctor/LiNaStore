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
    typedef int SOCKET;
    #define INVALID_SOCKET (SOCKET)(~0)
#endif

#define LINA_NAME_LENGTH 255
#define LINA_HEADER_LENGTH 0x108

#include <string>
#include <vector>
#include <memory>
#include <sstream>
#include "crc32.h"

class LiNaClient {
public:
    LiNaClient(std::string addr, int port);
    ~LiNaClient();

    bool uploadFile(std::string name, std::vector<char> data, uint8_t flags);
    std::vector<char> downloadFile(std::string name);
    bool deleteFile(std::string name);

    void check_sendv(const std::vector<std::pair<const void*, size_t>>& buffers, const char* context);
    void check_recv(char* buf, size_t len, const char* context);

    enum LiNaFlags {
        LINA_DELETE = 0xC0,
        LINA_WRITE = 0x80,
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
uint64_t to_long(std::vector<char> data, uint8_t length, bool little_endian = true);

#endif // LINA_CLIENT_H