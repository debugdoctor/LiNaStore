#include "linaclient.h"
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <time.h>
#include <openssl/evp.h>
#include <openssl/rand.h>
#include <openssl/sha.h>

static void u32_to_le(uint32_t value, uint8_t out[4])
{
    out[0] = (uint8_t)(value & 0xFF);
    out[1] = (uint8_t)((value >> 8) & 0xFF);
    out[2] = (uint8_t)((value >> 16) & 0xFF);
    out[3] = (uint8_t)((value >> 24) & 0xFF);
}

static bool recv_all(SOCKET sock, void *buf, size_t len)
{
    size_t total = 0;
    while (total < len)
    {
        ssize_t n = recv(sock, (char *)buf + total, len - total, 0);
        if (n <= 0)
        {
            return false;
        }
        total += (size_t)n;
    }
    return true;
}

static bool aes256gcm_encrypt_with_token(
    const char *token,
    const uint8_t *plaintext,
    size_t plaintext_len,
    uint8_t **out,
    size_t *out_len,
    char *msg,
    size_t msg_len)
{
    if (!token || !out || !out_len)
    {
        snprintf(msg, msg_len, "Invalid parameters for encryption");
        return false;
    }

    uint8_t key[SHA256_DIGEST_LENGTH];
    SHA256((const unsigned char *)token, strlen(token), key);

    uint8_t nonce[12];
    if (RAND_bytes(nonce, (int)sizeof(nonce)) != 1)
    {
        snprintf(msg, msg_len, "RAND_bytes failed");
        return false;
    }

    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    if (!ctx)
    {
        snprintf(msg, msg_len, "EVP_CIPHER_CTX_new failed");
        return false;
    }

    uint8_t *ciphertext = (uint8_t *)malloc(plaintext_len);
    uint8_t tag[16];
    if (!ciphertext)
    {
        EVP_CIPHER_CTX_free(ctx);
        snprintf(msg, msg_len, "Memory allocation failed for ciphertext");
        return false;
    }

    int len = 0;
    int outl = 0;

    if (EVP_EncryptInit_ex(ctx, EVP_aes_256_gcm(), NULL, NULL, NULL) != 1)
    {
        free(ciphertext);
        EVP_CIPHER_CTX_free(ctx);
        snprintf(msg, msg_len, "EVP_EncryptInit_ex failed");
        return false;
    }

    if (EVP_CIPHER_CTX_ctrl(ctx, EVP_CTRL_GCM_SET_IVLEN, (int)sizeof(nonce), NULL) != 1)
    {
        free(ciphertext);
        EVP_CIPHER_CTX_free(ctx);
        snprintf(msg, msg_len, "EVP_CTRL_GCM_SET_IVLEN failed");
        return false;
    }

    if (EVP_EncryptInit_ex(ctx, NULL, NULL, key, nonce) != 1)
    {
        free(ciphertext);
        EVP_CIPHER_CTX_free(ctx);
        snprintf(msg, msg_len, "EVP_EncryptInit_ex (key/nonce) failed");
        return false;
    }

    if (plaintext_len > 0)
    {
        if (EVP_EncryptUpdate(ctx, ciphertext, &len, plaintext, (int)plaintext_len) != 1)
        {
            free(ciphertext);
            EVP_CIPHER_CTX_free(ctx);
            snprintf(msg, msg_len, "EVP_EncryptUpdate failed");
            return false;
        }
        outl += len;
    }

    if (EVP_EncryptFinal_ex(ctx, ciphertext + outl, &len) != 1)
    {
        free(ciphertext);
        EVP_CIPHER_CTX_free(ctx);
        snprintf(msg, msg_len, "EVP_EncryptFinal_ex failed");
        return false;
    }
    outl += len;

    if (EVP_CIPHER_CTX_ctrl(ctx, EVP_CTRL_GCM_GET_TAG, (int)sizeof(tag), tag) != 1)
    {
        free(ciphertext);
        EVP_CIPHER_CTX_free(ctx);
        snprintf(msg, msg_len, "EVP_CTRL_GCM_GET_TAG failed");
        return false;
    }

    EVP_CIPHER_CTX_free(ctx);

    *out_len = sizeof(nonce) + (size_t)outl + sizeof(tag);
    *out = (uint8_t *)malloc(*out_len);
    if (!*out)
    {
        free(ciphertext);
        snprintf(msg, msg_len, "Memory allocation failed for encrypted payload");
        return false;
    }

    memcpy(*out, nonce, sizeof(nonce));
    memcpy(*out + sizeof(nonce), ciphertext, (size_t)outl);
    memcpy(*out + sizeof(nonce) + (size_t)outl, tag, sizeof(tag));

    free(ciphertext);
    return true;
}

uint8_t* to_array(uint64_t value, uint8_t length, bool little_endian)
{
    uint8_t* result = (uint8_t*)malloc(length);
    for (uint8_t i = 0; i < length; i++)
    {
        if (little_endian)
        {
            result[i] = (value >> (i * 8)) & 0xFF;
        }
        else
        {
            result[length - i -1] = (value >> (i * 8)) & 0xFF;
        }
    }
    return result;
}

uint64_t to_long(char* data, uint8_t length, bool little_endian)
{
    uint64_t result = 0;
    for (uint8_t i = 0; i < length; i++)
    {
        if (little_endian)
        {
            result |= (uint64_t)((uint8_t)data[i]) << (i * 8);
        }
    }
    return result;
}

int8_t check_sendv(LiNaClient* client, const void* buffers, size_t buffer_count)
{
    #ifdef _WIN32
        // Windows implementation using WSASend
        DWORD bytesSent;
        int result = WSASend(client->sock, (WSABUF*)buffers, buffer_count, &bytesSent, 0, NULL, NULL);
        if (result == SOCKET_ERROR)
            return SOCKET_ERROR;
        
        // Calculate total expected bytes
        DWORD totalExpected = 0;
        for (size_t i = 0; i < buffer_count; i++) {
            totalExpected += ((WSABUF*)buffers)[i].len;
        }
        return bytesSent == totalExpected;
    #else
        // POSIX implementation using writev
        ssize_t result = writev(client->sock, (struct iovec*)buffers, buffer_count);
        if (result == SOCKET_ERROR)
            return SOCKET_ERROR;
             
        // Calculate total expected bytes
        size_t totalExpected = 0;
        for (size_t i = 0; i < buffer_count; i++) {
            totalExpected += ((struct iovec*)buffers)[i].iov_len;
        }
        return (size_t)result == totalExpected;
    #endif
    return true;
}


bool _connect(LiNaClient *client)
{
    if (client->sock != INVALID_SOCKET) {
        return true; // Already connected
    }
    
    client->sock = socket(AF_INET, SOCK_STREAM, 0);
    if (client->sock == INVALID_SOCKET) {
        return false;
    }
    
    int result = connect(client->sock, (struct sockaddr *)&client->server, sizeof(client->server));
    if (result != 0) {
        #ifdef _WIN32
            closesocket(client->sock);
        #else
            close(client->sock);
        #endif
        client->sock = INVALID_SOCKET;
        return false;
    }
    
    return true;
}

bool _disconnect(LiNaClient *client)
{
    if (client->sock == INVALID_SOCKET) {
        return true; // Already disconnected
    }
    
    #ifdef _WIN32
        int result = closesocket(client->sock);
    #else
        int result = close(client->sock);
    #endif
    
    client->sock = INVALID_SOCKET;
    return result == 0;
}

LiNaResult uploadFile(LiNaClient *client, char *name, char *data, size_t data_len, uint8_t flags)
{
    LiNaResult res = { .status = false };
    char *msg = (char *)malloc(MAX_MSG_LEN);
    char* name_buf = NULL;
    uint32_t dlen = 0;
    uint8_t checksum_buf[4] = {0};
    uint8_t dlen_buf[4] = {0};
    char* header_buf = NULL;
    int8_t sock_status = 0;
    uint8_t ilen = 0;
    uint8_t *encrypted = NULL;
    size_t encrypted_len = 0;
    uint8_t *payload = NULL;
    size_t payload_len = 0;
    
    if (!client || !name || !data) {
        snprintf(msg, MAX_MSG_LEN, "Invalid parameters: client, name, or data is NULL");
        res.payload.message = msg;
        return res;
    }

    // Refresh token if needed before operation
    if (!refresh_token_if_needed(client, msg, MAX_MSG_LEN)) {
        res.payload.message = msg;
        goto cleanup;
    }

    size_t name_len = strlen(name);
    if (name_len > 255)
    {
        snprintf(msg, MAX_MSG_LEN, "File name is too long: %zu > 255", name_len);
        res.payload.message = msg;
        return res;
    }

    name_buf = (char*)malloc(name_len);
    if (!name_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for name buffer");
        res.payload.message = msg;
        return res;
    }
    
    memcpy(name_buf, name, name_len);
    ilen = (uint8_t)name_len;

    if (client->session_token && client->session_token[0] != '\0')
    {
        if (!aes256gcm_encrypt_with_token(
                client->session_token,
                (const uint8_t *)data,
                data_len,
                &encrypted,
                &encrypted_len,
                msg,
                MAX_MSG_LEN))
        {
            res.payload.message = msg;
            goto cleanup;
        }

        size_t token_len = strlen(client->session_token);
        payload_len = token_len + 1 + encrypted_len;
        payload = (uint8_t *)malloc(payload_len);
        if (!payload)
        {
            snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for payload");
            res.payload.message = msg;
            goto cleanup;
        }
        memcpy(payload, client->session_token, token_len);
        payload[token_len] = '\0';
        memcpy(payload + token_len + 1, encrypted, encrypted_len);
    }
    else
    {
        payload = (uint8_t *)data;
        payload_len = data_len;
    }

    dlen = (uint32_t)payload_len;
    u32_to_le(dlen, dlen_buf);

    CRC32 crc32 = CRC32_init();
    CRC32_update(&crc32, (uint8_t*)&ilen, 1);  // ilen is 1 byte
    CRC32_update(&crc32, (uint8_t*)name_buf, ilen);
    CRC32_update(&crc32, dlen_buf, 4);
    CRC32_update(&crc32, payload, payload_len);

    u32_to_le((uint32_t)CRC32_finalize(&crc32), checksum_buf);

    // Connect to LiNa server
    if (!_connect(client)) {
        snprintf(msg, MAX_MSG_LEN, "Failed to connect to server");
        res.payload.message = msg;
        goto cleanup;
    }

    // Calculate total header length: status(1) + ilen(1) + identifier(ilen) + dlen(4) + checksum(4)
    size_t header_len = LINA_HEADER_BASE_LENGTH + ilen;
    header_buf = (char* )malloc(header_len);
    if (!header_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for header buffer");
        goto cleanup;
    }

    #ifdef _WIN32
        WSABUF buffers[6] = {
            { .len = 1, .buf = (char*)&flags },              // status
            { .len = 1, .buf = (char*)&ilen },         // ilen
            { .len = (DWORD)ilen, .buf = name_buf },    // identifier
            { .len = 4, .buf = (char*)dlen_buf },              // dlen
            { .len = 4, .buf = (char*)checksum_buf },          // checksum
            { .len = (DWORD)payload_len, .buf = (char*)payload }            // data
        };
    #else
        struct iovec buffers[6] = {
            { .iov_len = 1, .iov_base = &flags },              // status
            { .iov_len = 1, .iov_base = &ilen },         // ilen
            { .iov_len = ilen, .iov_base = name_buf },      // identifier
            { .iov_len = 4, .iov_base = dlen_buf },               // dlen
            { .iov_len = 4, .iov_base = checksum_buf },          // checksum
            { .iov_len = payload_len, .iov_base = payload }            // data
        };
    #endif

    if((sock_status = check_sendv(client, buffers, 6)) <= 0) {
        snprintf(msg, MAX_MSG_LEN, "Failed to send upload data");
        goto cleanup;
    }
    
    if((sock_status = recv(client->sock, header_buf, header_len, 0)) <= 0) {
        if (sock_status == 0) {
            snprintf(msg, MAX_MSG_LEN, "Connection closed while receiving response");
        } else {
            #ifdef _WIN32
                snprintf(msg, MAX_MSG_LEN, "Winsock error: %d", WSAGetLastError());
            #else
                snprintf(msg, MAX_MSG_LEN, "errno: %d", errno);
            #endif
        }
        goto cleanup;
    }
    
    if (header_buf[0] != 0) {
        snprintf(msg, MAX_MSG_LEN, "Server returned error code: %d", header_buf[0]);
        goto cleanup;
    }
    
    res.status = true;
    res.payload.data = NULL;

    goto cleanup;

cleanup:
    if (name_buf) free(name_buf);
    if (header_buf) free(header_buf);
    if (encrypted) free(encrypted);
    if (payload && payload != (uint8_t *)data) free(payload);
    if (!res.status) {
        _disconnect(client);
    }
    return res;
}

LiNaResult downloadFile(LiNaClient* client, char* name)
{
    LiNaResult res = { .status = false };
    char *msg = (char *)malloc(MAX_MSG_LEN);
    char* name_buf = NULL;
    uint8_t checksum_buf[4] = {0};
    uint8_t dlen_buf[4] = {0};
    char* header_buf = NULL;
    char* data_recv = NULL;
    int8_t sock_status = 0;
    uint8_t ilen = 0;
    uint32_t dlen = 0;
    uint8_t *payload = NULL;
    size_t payload_len = 0;
    
    if (!client || !name) {
        snprintf(msg, MAX_MSG_LEN, "Invalid parameters: client or name is NULL");
        res.payload.message = msg;
        goto cleanup;
    }

    // Refresh token if needed before operation
    if (!refresh_token_if_needed(client, msg, MAX_MSG_LEN)) {
        res.payload.message = msg;
        goto cleanup;
    }

    uint8_t flags = LINA_READ;
    size_t name_len = strlen(name);
    if (name_len > 255)
    {
        snprintf(msg, MAX_MSG_LEN, "File name is too long: %zu > 255", name_len);
        res.payload.message = msg;
        goto cleanup;
    }

    name_buf = (char*)malloc(name_len);
    if (!name_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for name buffer");
        res.payload.message = msg;
        goto cleanup;
    }
    
    memcpy(name_buf, name, name_len);
    ilen = (uint8_t)name_len;

    if (client->session_token && client->session_token[0] != '\0')
    {
        payload = (uint8_t *)client->session_token;
        payload_len = strlen(client->session_token);
    }

    dlen = (uint32_t)payload_len;
    u32_to_le(dlen, dlen_buf);

    CRC32 crc32 = CRC32_init();
    CRC32_update(&crc32, &ilen, 1);
    CRC32_update(&crc32, (uint8_t*)name_buf, ilen);
    CRC32_update(&crc32, dlen_buf, 4);
    if (payload_len > 0)
    {
        CRC32_update(&crc32, payload, payload_len);
    }

    u32_to_le((uint32_t)CRC32_finalize(&crc32), checksum_buf);

    // Connect to LiNa server
    if (!_connect(client)) {
        snprintf(msg, MAX_MSG_LEN, "Failed to connect to server");
        res.payload.message = msg;
        goto cleanup;
    }

    // Calculate total header length: status(1) + ilen(1) + identifier(ilen) + dlen(4) + checksum(4)
    size_t header_len = LINA_HEADER_BASE_LENGTH + ilen;
    header_buf = (char* )malloc(header_len);
    if (!header_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for header buffer");
        goto cleanup;
    }

    #ifdef _WIN32
        WSABUF buffers[6] = {
            { .len = 1, .buf = (char*)&flags },          // status
            { .len = 1, .buf = (char*)&ilen },          // ilen
            { .len = (DWORD)ilen, .buf = name_buf },   // identifier
            { .len = 4, .buf = (char*)dlen_buf },          // dlen
            { .len = 4, .buf = (char*)checksum_buf },          // checksum
            { .len = (DWORD)payload_len, .buf = (char*)payload }, // data (token)
        };
    #else
        struct iovec buffers[6] = {
            { .iov_len = 1, .iov_base = &flags },          // status
            { .iov_len = 1, .iov_base = &ilen },          // ilen
            { .iov_len = ilen, .iov_base = name_buf },   // identifier
            { .iov_len = 4, .iov_base = dlen_buf },          // dlen
            { .iov_len = 4, .iov_base = checksum_buf },          // checksum
            { .iov_len = payload_len, .iov_base = payload }, // data (token)
        };
    #endif

    if((sock_status = check_sendv(client, buffers, 6)) <= 0) {
        snprintf(msg, MAX_MSG_LEN, "Failed to send download request");
        goto cleanup;
    }
    
    // Receive header first
    if(!recv_all(client->sock, header_buf, header_len)) {
        snprintf(msg, MAX_MSG_LEN, "Connection closed while receiving response header");
        goto cleanup;
    }
    
    // Check status byte (first byte)
    if (header_buf[0] != 0) {
        snprintf(msg, MAX_MSG_LEN, "Server returned error code: %d", header_buf[0]);
        goto cleanup;
    }
    
    // Parse header: status(1) + ilen(1) + identifier(ilen) + dlen(4) + checksum(4)
    uint8_t ilen_recv = (uint8_t)header_buf[1];
    size_t dlen_offset = 2 + ilen_recv;
    if (header_len < dlen_offset + 8)
    {
        snprintf(msg, MAX_MSG_LEN, "Invalid response header length");
        goto cleanup;
    }

    dlen = (uint32_t)to_long(header_buf + dlen_offset, 4, true);
    uint32_t checksum_recv = (uint32_t)to_long(header_buf + dlen_offset + 4, 4, true);
    
    // Allocate buffer for data
    data_recv = (char*)malloc(dlen + 1);  // +1 for null terminator
    if (!data_recv) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for data buffer");
        goto cleanup;
    }
    
    // Receive data
    if (dlen > 0 && !recv_all(client->sock, data_recv, dlen)) {
        snprintf(msg, MAX_MSG_LEN, "Connection closed while receiving data");
        goto cleanup;
    }
    data_recv[dlen] = '\0';

    // Verify checksum
    CRC32 crc32_resp = CRC32_init();
    CRC32_update(&crc32_resp, &ilen_recv, 1);
    CRC32_update(&crc32_resp, (uint8_t*)header_buf + 2, ilen_recv);
    uint8_t dlen_resp_buf[4];
    u32_to_le(dlen, dlen_resp_buf);
    CRC32_update(&crc32_resp, dlen_resp_buf, 4);
    if (dlen > 0) {
        CRC32_update(&crc32_resp, (uint8_t*)data_recv, dlen);
    }

    uint32_t expected = (uint32_t)CRC32_finalize(&crc32_resp);
    if (expected != checksum_recv) {
        snprintf(msg, MAX_MSG_LEN, "Checksum verification failed (expected %u, got %u)", expected, checksum_recv);
        goto cleanup;
    }
    
    res.status = true;
    res.payload.data = data_recv;

    goto cleanup;

cleanup:
    if (name_buf) free(name_buf);
    if (header_buf) free(header_buf);
    if (!res.status) {
        if (data_recv) free(data_recv);
        _disconnect(client);
    }
    return res;
}

LiNaResult deleteFile(LiNaClient *client, char *name)
{
    LiNaResult res = { .status = false };
    char *msg = (char *)malloc(MAX_MSG_LEN);
    char* name_buf = NULL;
    uint8_t checksum_buf[4] = {0};
    uint8_t dlen_buf[4] = {0};
    char* header_buf = NULL;
    int8_t sock_status = 0;
    uint8_t ilen = 0;
    uint8_t *payload = NULL;
    size_t payload_len = 0;
    uint32_t dlen = 0;
    
    if (!client || !name) {
        snprintf(msg, MAX_MSG_LEN, "Invalid parameters: client or name is NULL");
        res.payload.message = msg;
        goto cleanup;
    }

    // Refresh token if needed before operation
    if (!refresh_token_if_needed(client, msg, MAX_MSG_LEN)) {
        res.payload.message = msg;
        goto cleanup;
    }

    uint8_t flags = LINA_DELETE;
    size_t name_len = strlen(name);
    if (name_len > 255)
    {
        snprintf(msg, MAX_MSG_LEN, "File name is too long: %zu > 255", name_len);
        res.payload.message = msg;
        goto cleanup;
    }

    name_buf = (char*)malloc(name_len);
    if (!name_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for name buffer");
        res.payload.message = msg;
        goto cleanup;
    }
    
    memcpy(name_buf, name, name_len);
    ilen = (uint8_t)name_len;

    if (client->session_token && client->session_token[0] != '\0')
    {
        payload = (uint8_t *)client->session_token;
        payload_len = strlen(client->session_token);
    }
    dlen = (uint32_t)payload_len;
    u32_to_le(dlen, dlen_buf);

    CRC32 crc32 = CRC32_init();
    CRC32_update(&crc32, &ilen, 1);
    CRC32_update(&crc32, (uint8_t*)name_buf, ilen);
    CRC32_update(&crc32, dlen_buf, 4);
    if (payload_len > 0)
    {
        CRC32_update(&crc32, payload, payload_len);
    }

    u32_to_le((uint32_t)CRC32_finalize(&crc32), checksum_buf);

    // Connect to LiNa server
    if (!_connect(client)) {
        snprintf(msg, MAX_MSG_LEN, "Failed to connect to server");
        res.payload.message = msg;
        goto cleanup;
    }

    // Calculate total header length: status(1) + ilen(1) + identifier(ilen) + dlen(4) + checksum(4)
    size_t header_len = LINA_HEADER_BASE_LENGTH + ilen;
    header_buf = (char* )malloc(header_len);
    if (!header_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for header buffer");
        goto cleanup;
    }

    #ifdef _WIN32
        WSABUF buffers[6] = {
            { .len = 1, .buf = (char*)&flags },          // status
            { .len = 1, .buf = (char*)&ilen },          // ilen
            { .len = (DWORD)ilen, .buf = name_buf },   // identifier
            { .len = 4, .buf = (char*)dlen_buf },          // dlen
            { .len = 4, .buf = (char*)checksum_buf },          // checksum
            { .len = (DWORD)payload_len, .buf = (char*)payload }, // data (token)
        };
    #else
        struct iovec buffers[6] = {
            { .iov_len = 1, .iov_base = &flags },          // status
            { .iov_len = 1, .iov_base = &ilen },          // ilen
            { .iov_len = ilen, .iov_base = name_buf },   // identifier
            { .iov_len = 4, .iov_base = dlen_buf },          // dlen
            { .iov_len = 4, .iov_base = checksum_buf },          // checksum
            { .iov_len = payload_len, .iov_base = payload }, // data (token)
        };
    #endif

    if((sock_status = check_sendv(client, buffers, 6)) <= 0) {
        snprintf(msg, MAX_MSG_LEN, "Failed to send delete request");
        goto cleanup;
    }
    
    if((sock_status = recv(client->sock, header_buf, header_len, 0)) <= 0) {
        if (sock_status == 0) {
            snprintf(msg, MAX_MSG_LEN, "Connection closed while receiving response");
        } else {
            #ifdef _WIN32
                snprintf(msg, MAX_MSG_LEN, "Winsock error: %d", WSAGetLastError());
            #else
                snprintf(msg, MAX_MSG_LEN, "errno: %d", errno);
            #endif
        }
        goto cleanup;
    }
    
    if (header_buf[0] != 0) {
        snprintf(msg, MAX_MSG_LEN, "Server returned error code: %d", header_buf[0]);
        goto cleanup;
    }
    
    res.status = true;
    res.payload.data = NULL;

    goto cleanup;

cleanup:
    if (name_buf) free(name_buf);
    if (header_buf) free(header_buf);
    if (!res.status) {
        _disconnect(client);
    }
    return res;
}

HandshakeResult handshake(LiNaClient *client, char *username, char *password, bool should_cache_credentials)
{
    HandshakeResult res = { .status = false, .token = NULL, .expires_at = 0, .message = NULL };
    
    // Cache credentials if requested
    if (should_cache_credentials) {
        cache_credentials(client, username, password);
    }
    char *msg = (char *)malloc(MAX_MSG_LEN);
    char* username_buf = NULL;
    char* password_data = NULL;
    uint32_t dlen = 0;
    uint8_t dlen_buf[4] = {0};
    uint8_t checksum_buf[4] = {0};
    char* header_buf = NULL;
    char* data_recv = NULL;
    int8_t sock_status = 0;
    uint8_t ilen = 0;
    size_t password_len = 0;
    size_t data_len = 0;
    
    if (!client || !username || !password) {
        snprintf(msg, MAX_MSG_LEN, "Invalid parameters: client, username, or password is NULL");
        res.message = msg;
        goto cleanup;
    }

    uint8_t flags = LINA_AUTH;
    size_t username_len = strlen(username);
    password_len = strlen(password);
    
    if (username_len > 255 || password_len > 255)
    {
        snprintf(msg, MAX_MSG_LEN, "Username or password is too long: %zu or %zu > 255", username_len, password_len);
        res.message = msg;
        goto cleanup;
    }

    username_buf = (char*)malloc(username_len);
    if (!username_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for username buffer");
        res.message = msg;
        goto cleanup;
    }
    
    memcpy(username_buf, username, username_len);
    ilen = (uint8_t)username_len;

    // Build data: password + '\0' (null-terminated)
    data_len = password_len + 1;
    password_data = (char*)malloc(data_len);
    if (!password_data) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for password data buffer");
        res.message = msg;
        goto cleanup;
    }
    
    memcpy(password_data, password, password_len);
    password_data[password_len] = '\0';
    
    dlen = data_len;
    u32_to_le(dlen, dlen_buf);

    CRC32 crc32 = CRC32_init();
    CRC32_update(&crc32, (uint8_t*)&ilen, 1);  // ilen is 1 byte
    CRC32_update(&crc32, (uint8_t*)username_buf, ilen);
    CRC32_update(&crc32, dlen_buf, 4);
    CRC32_update(&crc32, (uint8_t*)password_data, data_len);

    u32_to_le((uint32_t)CRC32_finalize(&crc32), checksum_buf);

    // Connect to LiNa server
    if (!_connect(client)) {
        snprintf(msg, MAX_MSG_LEN, "Failed to connect to server");
        res.message = msg;
        goto cleanup;
    }

    // Calculate total header length: status(1) + ilen(1) + identifier(ilen) + dlen(4) + checksum(4)
    // Response header has no identifier, so use LINA_HEADER_BASE_LENGTH
    size_t header_len = LINA_HEADER_BASE_LENGTH;
    header_buf = (char* )malloc(header_len);
    if (!header_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for header buffer");
        goto cleanup;
    }

    #ifdef _WIN32
        WSABUF buffers[6] = {
            { .len = 1, .buf = (char*)&flags },              // flags
            { .len = 1, .buf = (char*)&ilen },         // ilen
            { .len = (DWORD)ilen, .buf = username_buf },   // identifier (username)
            { .len = 4, .buf = (char*)dlen_buf },              // dlen
            { .len = 4, .buf = (char*)checksum_buf },          // checksum
            { .len = (DWORD)data_len, .buf = password_data }   // data (password\0)
        };
    #else
        struct iovec buffers[6] = {
            { .iov_len = 1, .iov_base = &flags },              // flags
            { .iov_len = 1, .iov_base = &ilen },         // ilen
            { .iov_len = ilen, .iov_base = username_buf },   // identifier (username)
            { .iov_len = 4, .iov_base = dlen_buf },               // dlen
            { .iov_len = 4, .iov_base = checksum_buf },          // checksum
            { .iov_len = data_len, .iov_base = password_data }   // data (password\0)
        };
    #endif

    if((sock_status = check_sendv(client, buffers, 6)) <= 0) {
        snprintf(msg, MAX_MSG_LEN, "Failed to send handshake request");
        goto cleanup;
    }
    
    // Receive response header (no identifier in response)
    if(!recv_all(client->sock, header_buf, header_len)) {
        snprintf(msg, MAX_MSG_LEN, "Connection closed while receiving response header");
        goto cleanup;
    }
    
    // Parse response header: status(1) + ilen(1) + dlen(4) + checksum(4)
    uint8_t status = header_buf[0];
    // uint8_t ilen_recv = header_buf[1];  // Not used in response
    uint32_t dlen_recv = (uint32_t)to_long(header_buf + 2, 4, true);
    // Skip checksum (bytes 6-9)
    
    // Check for error status
    if (status != 0) {
        // Read error status from data field
        if (dlen_recv > 0) {
            data_recv = (char*)malloc(dlen_recv + 1);
            if (!data_recv) {
                snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for error data buffer");
                goto cleanup;
            }
            
            size_t total_received = 0;
            while (total_received < dlen_recv) {
                ssize_t bytes = recv(client->sock, data_recv + total_received, dlen_recv - total_received, 0);
                if (bytes <= 0) {
                    snprintf(msg, MAX_MSG_LEN, "Connection closed while receiving error data");
                    goto cleanup;
                }
                total_received += bytes;
            }
            
            if (data_recv[0] == 1) {
                snprintf(msg, MAX_MSG_LEN, "Invalid password");
            } else if (data_recv[0] == 2) {
                snprintf(msg, MAX_MSG_LEN, "Authentication disabled");
            } else if (data_recv[0] == 127) {
                snprintf(msg, MAX_MSG_LEN, "Internal server error");
            } else {
                snprintf(msg, MAX_MSG_LEN, "Authentication failed with error code: %d", data_recv[0]);
            }
            goto cleanup;
        }
        snprintf(msg, MAX_MSG_LEN, "Authentication failed with status: %d", status);
        goto cleanup;
    }
    
    // Receive response data: handshakeStatus(1) + token + '\0' + expires_at
    if (dlen_recv > 0) {
        data_recv = (char*)malloc(dlen_recv + 1);
        if (!data_recv) {
            snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for response data buffer");
            goto cleanup;
        }
        
        size_t total_received = 0;
        while (total_received < dlen_recv) {
            ssize_t bytes = recv(client->sock, data_recv + total_received, dlen_recv - total_received, 0);
            if (bytes <= 0) {
                if (bytes == 0) {
                    snprintf(msg, MAX_MSG_LEN, "Connection closed while receiving response data");
                } else {
                    #ifdef _WIN32
                        snprintf(msg, MAX_MSG_LEN, "Winsock error: %d", WSAGetLastError());
                    #else
                        snprintf(msg, MAX_MSG_LEN, "errno: %d", errno);
                    #endif
                }
                goto cleanup;
            }
            total_received += bytes;
        }
        data_recv[dlen_recv] = '\0';
        
        // Parse response: handshakeStatus(1) + token + '\0' + expires_at
        uint8_t handshake_status = data_recv[0];
        
        if (handshake_status == 0) { // Success
            // Find null terminator after token
            size_t null_pos = 0;
            for (size_t i = 1; i < dlen_recv; i++) {
                if (data_recv[i] == '\0') {
                    null_pos = i;
                    break;
                }
            }
            
            if (null_pos == 0) {
                snprintf(msg, MAX_MSG_LEN, "Invalid auth response: missing null terminator");
                goto cleanup;
            }
            
            // Extract token
            size_t token_len = null_pos - 1;
            res.token = (char*)malloc(token_len + 1);
            if (!res.token) {
                snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for token");
                goto cleanup;
            }
            memcpy(res.token, data_recv + 1, token_len);
            res.token[token_len] = '\0';
            
            // Extract expires_at
            char* expires_at_str = data_recv + null_pos + 1;
            res.expires_at = (uint64_t)strtoull(expires_at_str, NULL, 10);
            
            // Store token and expiration in client
            if (client->session_token) {
                free(client->session_token);
            }
            client->session_token = (char*)malloc(token_len + 1);
            if (client->session_token) {
                memcpy(client->session_token, res.token, token_len + 1);
            }
            client->token_expires_at = res.expires_at;
            
            res.status = true;
            res.message = NULL;
        } else {
            if (handshake_status == 1) {
                snprintf(msg, MAX_MSG_LEN, "Invalid password");
            } else if (handshake_status == 2) {
                snprintf(msg, MAX_MSG_LEN, "Authentication disabled");
            } else if (handshake_status == 127) {
                snprintf(msg, MAX_MSG_LEN, "Internal server error");
            } else {
                snprintf(msg, MAX_MSG_LEN, "Handshake failed with status: %d", handshake_status);
            }
            goto cleanup;
        }
    } else {
        snprintf(msg, MAX_MSG_LEN, "Empty auth response received");
        goto cleanup;
    }
    
    // Don't disconnect after handshake - keep connection for subsequent operations
    // _disconnect(client);

cleanup:
    if (username_buf) free(username_buf);
    if (password_data) free(password_data);
    if (header_buf) free(header_buf);
    if (!res.status) {
        if (data_recv) free(data_recv);
        _disconnect(client);
        if (msg) {
            res.message = msg;
        }
    } else {
        if (msg) free(msg);
    }
    return res;
}

void freeHandshakeResult(HandshakeResult* result)
{
    if (result) {
        if (result->token) {
            free(result->token);
        }
        if (result->message) {
            free(result->message);
        }
        result->status = false;
        result->token = NULL;
        result->expires_at = 0;
        result->message = NULL;
    }
}

void freeResult(LiNaResult* result)
{
    if (result) {
        if (result->payload.data) {
            free(result->payload.data);
        }
        if (result->payload.message) {
            free(result->payload.message);
        }
    }
}

// Client initialization and cleanup
LiNaClient* init_client(const char* address, int port, bool auto_refresh, uint32_t refresh_buffer)
{
    LiNaClient* client = (LiNaClient*)malloc(sizeof(LiNaClient));
    if (!client) {
        return NULL;
    }
    
    client->sock = INVALID_SOCKET;
    client->server_address = NULL;
    client->server_port = port;
    client->session_token = NULL;
    client->token_expires_at = 0;
    client->cached_username = NULL;
    client->cached_password = NULL;
    client->auto_refresh = auto_refresh;
    client->refresh_buffer = refresh_buffer;
    
    // Initialize server address
    memset(&client->server, 0, sizeof(client->server));
    client->server.sin_family = AF_INET;
    client->server.sin_port = htons(port);
    
    // Copy address string
    if (address) {
        size_t addr_len = strlen(address);
        client->server_address = (char*)malloc(addr_len + 1);
        if (client->server_address) {
            strcpy(client->server_address, address);
        }
        
        // Try to parse as IP address first
        if (inet_pton(AF_INET, address, &client->server.sin_addr) <= 0) {
            // Not an IP address, try to resolve as hostname
            // Note: getaddrinfo is available on both Windows (ws2tcpip.h) and POSIX (netdb.h)
            struct addrinfo hints, *result;
            memset(&hints, 0, sizeof(hints));
            hints.ai_family = AF_INET;
            hints.ai_socktype = SOCK_STREAM;
            
            int ret = getaddrinfo(address, NULL, &hints, &result);
            if (ret != 0) {
                // Failed to resolve hostname, keep using the original address
                // The connection will fail later if address is invalid
            } else {
                // Use the first result
                if (result) {
                    struct sockaddr_in* addr_in = (struct sockaddr_in*)result->ai_addr;
                    client->server.sin_addr = addr_in->sin_addr;
                    freeaddrinfo(result);
                }
            }
        }
    }
    
    return client;
}

void cleanup_client(LiNaClient* client)
{
    if (client) {
        if (client->session_token) {
            free(client->session_token);
            client->session_token = NULL;
        }
        if (client->cached_username) {
            free(client->cached_username);
            client->cached_username = NULL;
        }
        if (client->cached_password) {
            // Clear password from memory for security
            size_t len = strlen(client->cached_password);
            memset(client->cached_password, 0, len);
            free(client->cached_password);
            client->cached_password = NULL;
        }
        if (client->server_address) {
            free(client->server_address);
            client->server_address = NULL;
        }
        client->token_expires_at = 0;
        
        // Disconnect if still connected
        if (client->sock != INVALID_SOCKET) {
            #ifdef _WIN32
                closesocket(client->sock);
            #else
                close(client->sock);
            #endif
            client->sock = INVALID_SOCKET;
        }
        
        // Free the client itself
        free(client);
    }
}

// Token management functions
bool is_token_expired(LiNaClient* client)
{
    if (!client || client->token_expires_at == 0) {
        return true;  // No token, treat as expired
    }
    
    #ifdef _WIN32
        time_t current_time;
        time(&current_time);
    #else
        time_t current_time = time(NULL);
    #endif
    
    uint64_t current_timestamp = (uint64_t)current_time;
    
    // Check if token is expired or will expire within refresh_buffer seconds
    if (current_timestamp >= (client->token_expires_at - client->refresh_buffer)) {
        return true;
    }
    
    return false;
}

bool refresh_token_if_needed(LiNaClient* client, char* error_msg, size_t msg_len)
{
    if (!client) {
        if (error_msg && msg_len > 0) {
            snprintf(error_msg, msg_len, "Client is NULL");
        }
        return false;
    }
    
    if (!client->auto_refresh) {
        return true;  // Auto-refresh disabled
    }

    // Auth-free mode: no token and no cached credentials means no refresh needed.
    if ((client->session_token == NULL || client->session_token[0] == '\0') &&
        !(client->cached_username && client->cached_password)) {
        return true;
    }
    
    if (is_token_expired(client)) {
        if (client->cached_username && client->cached_password) {
            // Use cached credentials to refresh
            HandshakeResult res = handshake(client, client->cached_username,
                                          client->cached_password, false);
            if (!res.status) {
                if (error_msg && msg_len > 0) {
                    snprintf(error_msg, msg_len, "Failed to refresh token: %s",
                             res.message ? res.message : "Unknown error");
                }
                freeHandshakeResult(&res);
                return false;
            }
            freeHandshakeResult(&res);
            return true;
        } else {
            if (error_msg && msg_len > 0) {
                snprintf(error_msg, msg_len,
                         "Token expired and no cached credentials available");
            }
            return false;
        }
    }
    
    return true;  // Token is still valid
}

void cache_credentials(LiNaClient* client, char* username, char* password)
{
    if (!client) {
        return;
    }
    
    // Free existing cached credentials
    if (client->cached_username) {
        free(client->cached_username);
        client->cached_username = NULL;
    }
    if (client->cached_password) {
        // Clear old password from memory
        size_t len = strlen(client->cached_password);
        memset(client->cached_password, 0, len);
        free(client->cached_password);
        client->cached_password = NULL;
    }
    
    // Cache new credentials
    if (username) {
        size_t username_len = strlen(username);
        client->cached_username = (char*)malloc(username_len + 1);
        if (client->cached_username) {
            memcpy(client->cached_username, username, username_len + 1);
        }
    }
    
    if (password) {
        size_t password_len = strlen(password);
        client->cached_password = (char*)malloc(password_len + 1);
        if (client->cached_password) {
            memcpy(client->cached_password, password, password_len + 1);
        }
    }
}

void clear_cached_credentials(LiNaClient* client)
{
    if (!client) {
        return;
    }
    
    if (client->cached_username) {
        free(client->cached_username);
        client->cached_username = NULL;
    }
    
    if (client->cached_password) {
        // Clear password from memory for security
        size_t len = strlen(client->cached_password);
        memset(client->cached_password, 0, len);
        free(client->cached_password);
        client->cached_password = NULL;
    }
}
