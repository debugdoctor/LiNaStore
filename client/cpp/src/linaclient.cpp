#include "linaclient.h"
#include <ctime>
#include <cstring>
#include <openssl/evp.h>
#include <openssl/rand.h>
#include <openssl/sha.h>

static std::vector<uint8_t> sha256_bytes(const std::string &input)
{
    std::vector<uint8_t> out(SHA256_DIGEST_LENGTH);
    SHA256(reinterpret_cast<const unsigned char *>(input.data()), input.size(), out.data());
    return out;
}

static std::vector<uint8_t> aes256gcm_encrypt_with_token(const std::string &token, const std::vector<uint8_t> &plaintext)
{
    std::vector<uint8_t> key = sha256_bytes(token);

    std::vector<uint8_t> nonce(12);
    if (RAND_bytes(nonce.data(), static_cast<int>(nonce.size())) != 1)
    {
        throw LiNaClientException("Failed to generate nonce (RAND_bytes)");
    }

    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    if (!ctx)
    {
        throw LiNaClientException("Failed to create EVP_CIPHER_CTX");
    }

    std::vector<uint8_t> ciphertext(plaintext.size());
    std::vector<uint8_t> tag(16);

    int len = 0;
    int out_len = 0;

    if (EVP_EncryptInit_ex(ctx, EVP_aes_256_gcm(), nullptr, nullptr, nullptr) != 1)
    {
        EVP_CIPHER_CTX_free(ctx);
        throw LiNaClientException("EVP_EncryptInit_ex failed");
    }
    if (EVP_CIPHER_CTX_ctrl(ctx, EVP_CTRL_GCM_SET_IVLEN, static_cast<int>(nonce.size()), nullptr) != 1)
    {
        EVP_CIPHER_CTX_free(ctx);
        throw LiNaClientException("EVP_CTRL_GCM_SET_IVLEN failed");
    }
    if (EVP_EncryptInit_ex(ctx, nullptr, nullptr, key.data(), nonce.data()) != 1)
    {
        EVP_CIPHER_CTX_free(ctx);
        throw LiNaClientException("EVP_EncryptInit_ex (key/nonce) failed");
    }

    if (!plaintext.empty())
    {
        if (EVP_EncryptUpdate(ctx, ciphertext.data(), &len, plaintext.data(), static_cast<int>(plaintext.size())) != 1)
        {
            EVP_CIPHER_CTX_free(ctx);
            throw LiNaClientException("EVP_EncryptUpdate failed");
        }
        out_len += len;
    }

    if (EVP_EncryptFinal_ex(ctx, ciphertext.data() + out_len, &len) != 1)
    {
        EVP_CIPHER_CTX_free(ctx);
        throw LiNaClientException("EVP_EncryptFinal_ex failed");
    }
    out_len += len;
    ciphertext.resize(out_len);

    if (EVP_CIPHER_CTX_ctrl(ctx, EVP_CTRL_GCM_GET_TAG, static_cast<int>(tag.size()), tag.data()) != 1)
    {
        EVP_CIPHER_CTX_free(ctx);
        throw LiNaClientException("EVP_CTRL_GCM_GET_TAG failed");
    }

    EVP_CIPHER_CTX_free(ctx);

    std::vector<uint8_t> out;
    out.reserve(nonce.size() + ciphertext.size() + tag.size());
    out.insert(out.end(), nonce.begin(), nonce.end());
    out.insert(out.end(), ciphertext.begin(), ciphertext.end());
    out.insert(out.end(), tag.begin(), tag.end());
    return out;
}

template <typename T>
std::vector<T> to_vector(uint64_t value, uint8_t length, bool little_endian)
{
    std::vector<T> result(length);
    for (uint8_t i = 0; i < length; ++i)
    {
        if (little_endian)
        {
            result[i] = (value >> (i * 8)) & 0xFF;
        }
        else
        {
            result[length - 1 - i] = (value >> (i * 8)) & 0xFF;
        }
    }
    return result;
}

uint64_t to_long(std::vector<uint8_t> data, uint8_t length, bool little_endian)
{
    uint64_t result = 0;
    for (uint8_t i = 0; i < length; ++i)
    {
        if (little_endian)
        {
            result |= (uint64_t)((uint8_t)data[i]) << (i * 8);
        }
        else
        {
            result |= (uint64_t)((uint8_t)data[length - 1 - i]) << (i * 8);
        }
    }
    return result;
}

void LiNaClient::check_sendv(const std::vector<std::pair<const void *, size_t>> &buffers, const char *context)
{
    size_t total_length = 0;
    for (const auto &buf : buffers)
    {
        total_length += buf.second;
    }

#ifdef _WIN32
    std::vector<WSABUF> wsa_buffers;
    wsa_buffers.reserve(buffers.size());
    for (const auto &buf : buffers)
    {
        WSABUF wsa_buf;
        wsa_buf.buf = (CHAR *)buf.first;
        wsa_buf.len = buf.second;
        wsa_buffers.push_back(wsa_buf);
    }
    DWORD bytesSent;
    DWORD flags = 0;
    int ret = WSASend(sock, wsa_buffers.data(), wsa_buffers.size(), &bytesSent, flags, NULL, NULL);
    if (ret == SOCKET_ERROR)
    {
        std::ostringstream oss;
        oss << "Winsock error: " << WSAGetLastError();
        throw LiNaClientException(std::string("Failed to sendv ") + context + " - " + oss.str());
    }
#else
    std::vector<struct iovec> iovs;
    iovs.reserve(buffers.size());
    for (const auto &buf : buffers)
    {
        struct iovec iov;
        iov.iov_base = const_cast<void *>(buf.first);
        iov.iov_len = buf.second;
        iovs.push_back(iov);
    }
    ssize_t bytesSent = writev(sock, iovs.data(), iovs.size());
    if (bytesSent == -1)
    {
        std::ostringstream oss;
        oss << "errno: " << errno;
        throw LiNaClientException(std::string("Failed to sendv ") + context + " - " + oss.str());
    }
#endif

    if (bytesSent < static_cast<ssize_t>(total_length))
    {
        throw LiNaClientException(std::string("Partial sendv detected for ") + context);
    }
};

void LiNaClient::check_recv(char *buf, size_t len, const char *context)
{
    ssize_t received = recv(sock, buf, len, 0);
    if (received == -1)
    {
        std::ostringstream oss;
#ifdef _WIN32
        oss << "Winsock error: " << WSAGetLastError();
#else
        oss << "errno: " << errno;
#endif
        throw LiNaClientException(std::string("Failed to recv ") + context + " - " + oss.str());
    }
    else if (received == 0)
    {
        throw LiNaClientException(std::string("Connection closed while receiving ") + context);
    }
}

LiNaClient::LiNaClient(std::string address, int port, bool auto_refresh, uint32_t refresh_buffer)
    : sock(INVALID_SOCKET), server_addr(), server_address(address), session_token(), token_expires_at(0), cached_username(), cached_password(), auto_refresh(auto_refresh), refresh_buffer(refresh_buffer)
{
    memset(&this->server_addr, 0, sizeof(this->server_addr));
    this->server_addr.sin_family = AF_INET;
    this->server_addr.sin_port = htons(port);

    if (inet_pton(AF_INET, address.c_str(), &this->server_addr.sin_addr) <= 0)
    {
        // Not an IP address, try to resolve as hostname
        struct addrinfo hints, *result;
        memset(&hints, 0, sizeof(hints));
        hints.ai_family = AF_INET;
        hints.ai_socktype = SOCK_STREAM;

        int ret = getaddrinfo(address.c_str(), NULL, &hints, &result);
        if (ret != 0)
        {
            // Failed to resolve hostname, keep using the original address
            // The connection will fail later if address is invalid
        }
        else
        {
            // Use the first result
            if (result)
            {
                struct sockaddr_in *addr_in = (struct sockaddr_in *)result->ai_addr;
                this->server_addr.sin_addr = addr_in->sin_addr;
                freeaddrinfo(result);
            }
        }
    }
}

LiNaClient::~LiNaClient()
{
    disconnect();
    clearCachedCredentials();
}

bool LiNaClient::connect()
{
    if (sock != INVALID_SOCKET)
    {
        return true; // Already connected
    }

    sock = socket(AF_INET, SOCK_STREAM, 0);
    if (sock == INVALID_SOCKET)
    {
        throw LiNaClientException("Failed to create socket");
    }

    int result = ::connect(sock, (struct sockaddr *)&server_addr, sizeof(server_addr));
    if (result != 0)
    {
#ifdef _WIN32
        int error = WSAGetLastError();
        closesocket(sock);
#else
        int error = errno;
        close(sock);
#endif
        sock = INVALID_SOCKET;

        std::ostringstream oss;
        oss << "Failed to connect to server: " << error;
        throw LiNaClientException(oss.str());
    }

    return true;
}

bool LiNaClient::disconnect()
{
    if (sock != INVALID_SOCKET)
    {
#ifdef _WIN32
        int ret = closesocket(sock);
#else
        int ret = close(sock);
#endif
        sock = INVALID_SOCKET;
        return ret == 0;
    }
    return true; // Already disconnected
}

bool LiNaClient::uploadFile(std::string name, std::vector<char> data, uint8_t flags)
{
    // Refresh token if needed before operation
    refreshTokenIfNeeded();

    // Name validation
    if (name.empty())
    {
        throw LiNaClientException("File name cannot be empty");
    }

    if (data.empty())
    {
        throw LiNaClientException("File data cannot be empty");
    }

    // Variable length identifier
    if (name.length() > LINA_NAME_MAX_LENGTH)
    {
        throw LiNaClientException("File name exceeds maximum length");
    }

    uint8_t ilen = name.length();
    std::vector<uint8_t> identifier(name.begin(), name.end());

    std::vector<uint8_t> payload_data;
    if (!session_token.empty())
    {
        std::vector<uint8_t> plaintext(data.begin(), data.end());
        std::vector<uint8_t> encrypted = aes256gcm_encrypt_with_token(session_token, plaintext);

        payload_data.reserve(session_token.size() + 1 + encrypted.size());
        payload_data.insert(payload_data.end(), session_token.begin(), session_token.end());
        payload_data.push_back(0);
        payload_data.insert(payload_data.end(), encrypted.begin(), encrypted.end());
    }
    else
    {
        payload_data.assign(data.begin(), data.end());
    }

    uint32_t dlen = payload_data.size();
    std::vector<uint8_t> dlen_buf = to_vector<uint8_t>(dlen, 4);

    CRC32 crc32 = CRC32();
    crc32.update(std::vector<uint8_t>{ilen});
    crc32.update(identifier);
    crc32.update(dlen_buf);
    crc32.update(payload_data);

    std::vector<uint8_t> checksum = to_vector<uint8_t>(crc32.finalize(), 4);

    // Connect to LiNa server
    connect();

    try
    {
        std::vector<std::pair<const void *, size_t>> send_buffers;
        send_buffers.push_back({&flags, 1});                            // flags
        send_buffers.push_back({&ilen, 1});                             // ilen
        send_buffers.push_back({identifier.data(), identifier.size()}); // identifier
        send_buffers.push_back({dlen_buf.data(), dlen_buf.size()});     // dlen
        send_buffers.push_back({checksum.data(), checksum.size()});     // checksum
        send_buffers.push_back({payload_data.data(), payload_data.size()}); // data

        check_sendv(send_buffers, "file upload data");

        size_t header_len = LINA_HEADER_BASE_LENGTH + ilen;
        std::vector<char> header_buf(header_len);
        check_recv(header_buf.data(), header_buf.size(), "response header");

        if (header_buf[0] != 0)
        {
            std::ostringstream oss;
            oss << "Server returned error code: " << static_cast<int>(header_buf[0]) << " for file: " << name;
            throw LiNaClientException(oss.str());
        }

        disconnect();
        return true;
    }
    catch (...)
    {
        disconnect();
        throw;
    }
}

std::vector<char> LiNaClient::downloadFile(std::string name)
{
    // Refresh token if needed before operation
    refreshTokenIfNeeded();

    if (name.empty())
    {
        throw LiNaClientException("File name cannot be empty");
    }

    if (name.length() > LINA_NAME_MAX_LENGTH)
    {
        throw LiNaClientException("File name exceeds maximum length");
    }

    uint8_t ilen = name.length();
    std::vector<uint8_t> identifier(name.begin(), name.end());

    std::vector<uint8_t> payload_data;
    if (!session_token.empty())
    {
        payload_data.assign(session_token.begin(), session_token.end());
    }

    uint32_t dlen = payload_data.size();
    std::vector<uint8_t> dlen_buf = to_vector<uint8_t>(dlen, 4);

    CRC32 crc32_req = CRC32();
    crc32_req.update(std::vector<uint8_t>{ilen});
    crc32_req.update(identifier);
    crc32_req.update(dlen_buf);
    crc32_req.update(payload_data);

    std::vector<uint8_t> checksum = to_vector<uint8_t>(crc32_req.finalize(), 4);

    connect();

    try
    {
        uint8_t flags = LINA_READ;
        std::vector<std::pair<const void *, size_t>> send_buffers;
        send_buffers.push_back({&flags, 1});                            // flags
        send_buffers.push_back({&ilen, 1});                             // ilen
        send_buffers.push_back({identifier.data(), identifier.size()}); // identifier
        send_buffers.push_back({dlen_buf.data(), dlen_buf.size()});     // dlen
        send_buffers.push_back({checksum.data(), checksum.size()});     // checksum
        if (!payload_data.empty())
        {
            send_buffers.push_back({payload_data.data(), payload_data.size()}); // data (token)
        }

        check_sendv(send_buffers, "file download data");

        size_t header_len = LINA_HEADER_BASE_LENGTH + ilen;
        std::vector<char> header_buf(header_len);
        check_recv(header_buf.data(), header_buf.size(), "response header");

        // Check response flag first
        if (header_buf[0] != 0)
        {
            std::ostringstream oss;
            oss << "Server returned error code: " << static_cast<int>(header_buf[0]) << " for file: " << name;
            throw LiNaClientException(oss.str());
        }

        // Header break down
        uint16_t p = 1; // Skip the flag byte
        uint8_t ilen_recv = header_buf[p];
        p += 1;
        std::vector<uint8_t> identifier_recv(header_buf.begin() + p, header_buf.begin() + p + ilen_recv);
        p += ilen_recv;
        std::vector<uint8_t> dlen_recv_buf(header_buf.begin() + p, header_buf.begin() + p + 4);
        uint32_t dlen_recv = to_long(dlen_recv_buf, 4);
        p += 4;
        std::vector<uint8_t> checksum_recv(header_buf.begin() + p, header_buf.begin() + p + 4);

        if (dlen_recv > 0)
        {
            std::vector<char> data_recv(dlen_recv);
            check_recv(data_recv.data(), dlen_recv, "response body");

            // Disconnect
            disconnect();

            CRC32 crc32_resp = CRC32();
            crc32_resp.update(std::vector<uint8_t>{ilen_recv});
            crc32_resp.update(identifier_recv);
            crc32_resp.update(dlen_recv_buf);
            std::vector<uint8_t> data_u8(data_recv.begin(), data_recv.end());
            crc32_resp.update(data_u8);

            if (crc32_resp.finalize() != to_long(checksum_recv, 4))
            {
                std::ostringstream oss;
                oss << "CRC32 checksum mismatch for file: " << name;
                throw LiNaClientException(oss.str());
            }

            return data_recv;
        }
        else
        {
            disconnect();
            return std::vector<char>(); // Empty file
        }
    }
    catch (...)
    {
        disconnect();
        throw;
    }
}

bool LiNaClient::deleteFile(std::string name)
{
    // Refresh token if needed before operation
    refreshTokenIfNeeded();

    if (name.empty())
    {
        throw LiNaClientException("File name cannot be empty");
    }

    uint8_t flags = LINA_DELETE;

    if (name.length() > LINA_NAME_MAX_LENGTH)
    {
        throw LiNaClientException("File name exceeds maximum length");
    }

    uint8_t ilen = name.length();
    std::vector<uint8_t> identifier(name.begin(), name.end());

    std::vector<uint8_t> payload_data;
    if (!session_token.empty())
    {
        payload_data.assign(session_token.begin(), session_token.end());
    }

    uint32_t dlen = payload_data.size();
    std::vector<uint8_t> dlen_buf = to_vector<uint8_t>(dlen, 4);

    CRC32 crc32 = CRC32();
    crc32.update(std::vector<uint8_t>{ilen});
    crc32.update(identifier);
    crc32.update(dlen_buf);
    crc32.update(payload_data);

    std::vector<uint8_t> checksum = to_vector<uint8_t>(crc32.finalize(), 4);

    connect();

    try
    {
        std::vector<std::pair<const void *, size_t>> send_buffers;
        send_buffers.push_back({&flags, 1});                            // flags
        send_buffers.push_back({&ilen, 1});                             // ilen
        send_buffers.push_back({identifier.data(), identifier.size()}); // identifier
        send_buffers.push_back({dlen_buf.data(), dlen_buf.size()});     // dlen
        send_buffers.push_back({checksum.data(), checksum.size()});     // checksum
        if (!payload_data.empty())
        {
            send_buffers.push_back({payload_data.data(), payload_data.size()}); // data (token)
        }

        check_sendv(send_buffers, "file delete data");

        size_t header_len = LINA_HEADER_BASE_LENGTH + ilen;
        std::vector<char> header_buf(header_len);
        check_recv(header_buf.data(), header_buf.size(), "response header");

        if (header_buf[0] != 0)
        {
            std::ostringstream oss;
            oss << "Server returned error code: " << static_cast<int>(header_buf[0]) << " for file: " << name;
            throw LiNaClientException(oss.str());
        }

        // Disconnect
        disconnect();
        return true;
    }
    catch (...)
    {
        disconnect();
        throw;
    }
}

HandshakeResult LiNaClient::handshake(std::string username, std::string password, bool cache_credentials)
{
    HandshakeResult result = {.status = false, .token = "", .expires_at = 0, .message = ""};

    // Cache credentials if requested
    if (cache_credentials)
    {
        this->cacheCredentials(username, password);
    }

    // Validate inputs
    if (username.empty())
    {
        result.message = "Username cannot be empty";
        return result;
    }
    if (password.empty())
    {
        result.message = "Password cannot be empty";
        return result;
    }

    if (username.length() > LINA_NAME_MAX_LENGTH || password.length() > LINA_NAME_MAX_LENGTH)
    {
        std::ostringstream oss;
        oss << "Username or password exceeds maximum length: " << username.length() << " or " << password.length() << " > 255";
        result.message = oss.str();
        return result;
    }

    try
    {
        uint8_t flags = LINA_AUTH;
        uint8_t ilen = username.length();
        std::vector<uint8_t> identifier(username.begin(), username.end());

        // Build data: password + '\0' (null-terminated)
        std::vector<uint8_t> password_data(password.begin(), password.end());
        password_data.push_back(0); // Null terminator
        uint32_t dlen = password_data.size();
        std::vector<uint8_t> dlen_buf = to_vector<uint8_t>(dlen, 4);

        // Calculate CRC32 checksum
        CRC32 crc32 = CRC32();
        crc32.update(std::vector<uint8_t>{ilen});
        crc32.update(identifier);
        crc32.update(dlen_buf);
        crc32.update(password_data);
        std::vector<uint8_t> checksum = to_vector<uint8_t>(crc32.finalize(), 4);

        // Connect to LiNa server
        connect();

        // Send handshake request
        std::vector<std::pair<const void *, size_t>> send_buffers;
        send_buffers.push_back({&flags, 1});                                  // flags
        send_buffers.push_back({&ilen, 1});                                   // ilen
        send_buffers.push_back({identifier.data(), identifier.size()});       // identifier (username)
        send_buffers.push_back({dlen_buf.data(), dlen_buf.size()});           // dlen
        send_buffers.push_back({checksum.data(), checksum.size()});           // checksum
        send_buffers.push_back({password_data.data(), password_data.size()}); // data (password\0)

        check_sendv(send_buffers, "handshake request");

        // Receive response header (no identifier in response)
        size_t header_len = LINA_HEADER_BASE_LENGTH;
        std::vector<char> header_buf(header_len);
        check_recv(header_buf.data(), header_buf.size(), "handshake response header");

        // Parse response header: status(1) + ilen(1) + dlen(4) + checksum(4)
        uint8_t status = header_buf[0];
        // uint8_t ilen_recv = header_buf[1];
        std::vector<uint8_t> dlen_recv_buf(header_buf.begin() + 2, header_buf.begin() + 6);
        uint32_t dlen_recv = to_long(dlen_recv_buf, 4);
        // Skip checksum (bytes 6-9)

        // Check for error status
        if (status != 0)
        {
            // Read error status from data field
            if (dlen_recv > 0)
            {
                std::vector<char> error_data(dlen_recv);
                check_recv(error_data.data(), dlen_recv, "error data");

                uint8_t error_code = error_data[0];
                if (error_code == 1)
                {
                    result.message = "Invalid password";
                }
                else if (error_code == 2)
                {
                    result.message = "Authentication disabled";
                }
                else if (error_code == 127)
                {
                    result.message = "Internal server error";
                }
                else
                {
                    std::ostringstream oss;
                    oss << "Authentication failed with error code: " << static_cast<int>(error_code);
                    result.message = oss.str();
                }
            }
            else
            {
                std::ostringstream oss;
                oss << "Authentication failed with status: " << static_cast<int>(status);
                result.message = oss.str();
            }
            disconnect();
            return result;
        }

        // Receive response data: handshakeStatus(1) + token + '\0' + expires_at
        if (dlen_recv > 0)
        {
            std::vector<char> data_recv(dlen_recv);
            check_recv(data_recv.data(), dlen_recv, "handshake response data");

            // Parse response: handshakeStatus(1) + token + '\0' + expires_at
            uint8_t handshake_status = data_recv[0];

            if (handshake_status == 0)
            { // Success
                // Find null terminator after token
                size_t null_pos = 0;
                for (size_t i = 1; i < dlen_recv; i++)
                {
                    if (data_recv[i] == 0)
                    {
                        null_pos = i;
                        break;
                    }
                }

                if (null_pos == 0)
                {
                    result.message = "Invalid auth response: missing null terminator";
                    disconnect();
                    return result;
                }

                // Extract token
                result.token = std::string(data_recv.begin() + 1, data_recv.begin() + null_pos);

                // Extract expires_at
                std::string expires_at_str(data_recv.begin() + null_pos + 1, data_recv.end());
                result.expires_at = std::stoull(expires_at_str);

                // Store token and expiration in client
                this->session_token = result.token;
                this->token_expires_at = result.expires_at;

                result.status = true;
                result.message = "";

                // Don't disconnect after handshake - keep connection for subsequent operations
                // disconnect();
            }
            else
            {
                if (handshake_status == 1)
                {
                    result.message = "Invalid password";
                }
                else if (handshake_status == 2)
                {
                    result.message = "Authentication disabled";
                }
                else if (handshake_status == 127)
                {
                    result.message = "Internal server error";
                }
                else
                {
                    std::ostringstream oss;
                    oss << "Handshake failed with status: " << static_cast<int>(handshake_status);
                    result.message = oss.str();
                }
                disconnect();
            }
        }
        else
        {
            result.message = "Empty auth response received";
            disconnect();
        }
    }
    catch (LiNaClientException &e)
    {
        result.message = e.what();
        disconnect();
    }
    catch (...)
    {
        result.message = "Unknown error during handshake";
        disconnect();
    }

    return result;
}
