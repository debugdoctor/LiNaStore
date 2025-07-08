#include "linaclient.h"

template<typename T>
std::vector<T> to_vector(uint64_t value, uint8_t length, bool little_endian = true){
    std::vector<T> result(length);
    for (uint8_t i = 0; i < length; ++i) {
        if (little_endian) {
            result[i] = (value >> (i * 8)) & 0xFF;
        } else {
            result[length - 1 - i] = (value >> (i * 8)) & 0xFF;
        }
    }
    return result;
}

uint64_t to_long(std::vector<uint8_t> data, uint8_t length, bool little_endian = true)
{
    uint64_t result = 0;
    for (uint8_t i = 0; i < length; ++i) {
        if (little_endian) {
            result |= (uint64_t)((uint8_t)data[i]) << (i * 8);
        } else {
            result |= (uint64_t)((uint8_t)data[length - 1 - i]) << (i * 8);
        }
    }
    return result;
}

void LiNaClient::check_sendv(const std::vector<std::pair<const void*, size_t>>& buffers, const char* context) {
    size_t total_length = 0;
    for (const auto& buf : buffers) {
        total_length += buf.second;
    }

#ifdef _WIN32
    std::vector<WSABUF> wsa_buffers;
    wsa_buffers.reserve(buffers.size());
    for (const auto& buf : buffers) {
        WSABUF wsa_buf;
        wsa_buf.buf = (CHAR*)buf.first;
        wsa_buf.len = buf.second;
        wsa_buffers.push_back(wsa_buf);
    }
    DWORD bytesSent;
    DWORD flags = 0;
    int ret = WSASend(sock, wsa_buffers.data(), wsa_buffers.size(), &bytesSent, flags, NULL, NULL);
    if (ret == SOCKET_ERROR) {
        std::ostringstream oss;
        oss << "Winsock error: " << WSAGetLastError();
        throw LiNaClientException(std::string("Failed to sendv ") + context + " - " + oss.str());
    }
#else
    std::vector<struct iovec> iovs;
    iovs.reserve(buffers.size());
    for (const auto& buf : buffers) {
        struct iovec iov;
        iov.iov_base = const_cast<void*>(buf.first);
        iov.iov_len = buf.second;
        iovs.push_back(iov);
    }
    ssize_t bytesSent = writev(sock, iovs.data(), iovs.size());
    if (bytesSent == -1) {
        std::ostringstream oss;
        oss << "errno: " << errno;
        throw LiNaClientException(std::string("Failed to sendv ") + context + " - " + oss.str());
    }
#endif

    if (bytesSent < static_cast<ssize_t>(total_length)) {
        throw LiNaClientException(std::string("Partial sendv detected for ") + context);
    }
};

void LiNaClient::check_recv(char* buf, size_t len, const char* context)
{
    ssize_t received = recv(sock, buf, len, 0);
    if (received == -1) {
        std::ostringstream oss;
        #ifdef _WIN32
            oss << "Winsock error: " << WSAGetLastError();
        #else
            oss << "errno: " << errno;
        #endif
        throw LiNaClientException(std::string("Failed to recv ") + context + " - " + oss.str());
    } else if (received == 0) {
        throw LiNaClientException(std::string("Connection closed while receiving ") + context);
    }
}

LiNaClient::LiNaClient(std::string addr, int port)
{
    this->sock = socket(AF_INET, SOCK_STREAM, 0);
    memset(&this->server_addr, 0, sizeof(this->server_addr));
    this->server_addr.sin_family = AF_INET;
    this->server_addr.sin_addr.s_addr = inet_addr(addr.c_str());
    this->server_addr.sin_port = htons(port);
}

LiNaClient::~LiNaClient() {
    disconnect();
}
bool LiNaClient::connect(){
    return ::connect(this->sock, (struct sockaddr*)&this->server_addr, sizeof(this->server_addr)) == 0;
}

bool LiNaClient::disconnect(){
    if (sock != INVALID_SOCKET) {
        #ifdef _WIN32
            int ret = closesocket(sock);
        #else
            int ret = close(sock);
        #endif
        sock = INVALID_SOCKET;
        return ret == 0;  // Reset after closing
    }
}

bool LiNaClient::uploadFile(std::string name, std::vector<char> data, uint8_t flags)
{
    // Name copy
    if(name.empty()) {
        throw LiNaClientException("File name cannot be empty");
    }
    
    // 255 bytes padding for name
    std::vector<uint8_t> name_buf(LINA_NAME_LENGTH);
    if(name.length() > LINA_NAME_LENGTH) {
        throw LiNaClientException("File name exceeds maximum length");
    }
    memcpy(name_buf.data(), name.c_str(), name.length());

    std::vector<uint8_t> length = to_vector<uint8_t>(data.size(), 4);

    CRC32 crc32 = CRC32();
    crc32.update(name_buf);
    crc32.update(length);
    std::vector<uint8_t> data_u8(data.begin(), data.end());
    crc32.update(data_u8);

    std::vector<uint8_t> checksum = to_vector<uint8_t>(crc32.finalize(), 4);

    // Connect to LiNa server
    connect();

    std::vector<std::pair<const void*, size_t>> send_buffers;
    send_buffers.push_back({name_buf.data(), name_buf.size()});
    send_buffers.push_back({length.data(), length.size()});
    send_buffers.push_back({checksum.data(), checksum.size()});
    send_buffers.push_back({data.data(), data.size()});
    
    check_sendv(send_buffers, "file upload data");

    std::vector<char> header_buf(LINA_HEADER_LENGTH);
    check_recv(header_buf.data(), header_buf.size(), "response header");
    disconnect();

    return header_buf[0] == 0;
}

std::vector<char> LiNaClient::downloadFile(std::string name)
{
    if(name.empty()) {
        throw LiNaClientException("File name cannot be empty");
    }
    
    std::vector<uint8_t> name_buf(LINA_NAME_LENGTH);
    if(name.length() > LINA_NAME_LENGTH) {
        throw LiNaClientException("File name exceeds maximum length");
    }
    memcpy(name_buf.data(), name.c_str(), name.length());

    std::vector<uint8_t> length = to_vector<uint8_t>(0, 4);
    CRC32 crc32 = CRC32();
    crc32.update(name_buf);
    crc32.update(length);

    std::vector<uint8_t> checksum = to_vector<uint8_t>(crc32.finalize(), 4);

    connect();

    std::vector<std::pair<const void*, size_t>> send_buffers;
    send_buffers.push_back({name_buf.data(), name_buf.size()});
    send_buffers.push_back({length.data(), length.size()});
    send_buffers.push_back({checksum.data(), checksum.size()});

    check_sendv(send_buffers, "file download data");

    std::vector<char> header_buf(LINA_HEADER_LENGTH);
    check_recv(header_buf.data(), header_buf.size(), "response header");

    // Header break down
    uint16_t p = 0;
    uint8_t flags = header_buf[p++];
    std::vector<uint8_t> name_recv(header_buf.begin() + p, header_buf.begin() + p + LINA_NAME_LENGTH);
    p += LINA_NAME_LENGTH;
    std::vector<uint8_t> length_recv(header_buf.begin() + p, header_buf.begin() + p + 4);
    uint32_t length_u32 = to_long(length_recv, 4);
    p += 4;
    std::vector<uint8_t> checksum_recv(header_buf.begin() + p, header_buf.begin() + p + 4);
    p += 4;


    std::vector<char> data_recv(length_u32);
    check_recv(data_recv.data() , length_u32, "response body");
    // Disconnect
    disconnect();

    crc32.update(name_recv);
    crc32.update(length_recv);
    std::vector<uint8_t> data_u8(data_recv.begin(), data_recv.end());
    crc32.update(data_u8);

    if (crc32.finalize() != to_long(checksum_recv, 4)) {
        throw LiNaClientException("CRC32 checksum mismatch");
    }
    
    return data_recv;
}

bool LiNaClient::deleteFile(std::string name)
{
    uint8_t flags = LINA_DELETE;

    std::vector<uint8_t> name_buf(LINA_NAME_LENGTH);
    if(name.length() > LINA_NAME_LENGTH) {
        throw LiNaClientException("File name exceeds maximum length");
    }
    memcpy(name_buf.data(), name.c_str(), name.length());

    std::vector<uint8_t> length = to_vector<uint8_t>(0, 4);
    CRC32 crc32 = CRC32();
    crc32.update(name_buf);
    crc32.update(length);

    std::vector<char> checksum = to_vector<char>(crc32.finalize(), 4);

    connect();
    
    std::vector<std::pair<const void*, size_t>> send_buffers;
    send_buffers.push_back({name_buf.data(), name_buf.size()});
    send_buffers.push_back({length.data(), length.size()});
    send_buffers.push_back({checksum.data(), checksum.size()});

    check_sendv(send_buffers, "file delete data");

    std::vector<char> header_buf(LINA_HEADER_LENGTH);
    check_recv(header_buf.data(), header_buf.size(), "response header");
    // Disconnect
    disconnect();
    return header_buf[0] == 0;
}