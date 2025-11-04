#include "linaclient.h"

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
            result[length - i - 1] = (value >> (i * 8)) & 0xFF;
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

int8_t check_sendv(LiNaClient* client, const void* buffers, size_t buffer_count, const char* context)
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
        return result == totalExpected;
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
    uint8_t* length = NULL;
    uint8_t* checksum = NULL;
    char* header_buf = NULL;
    int8_t sock_status = 0;
    
    if (!client || !name || !data) {
        snprintf(msg, MAX_MSG_LEN, "Invalid parameters: client, name, or data is NULL");
        res.payload.message = msg;
        return res;
    }

    if (strlen(name) > LINA_NAME_LENGTH)
    {
        snprintf(msg, MAX_MSG_LEN, "File name is too long: %zu > %d", strlen(name), LINA_NAME_LENGTH);
        res.payload.message = msg;
        return res;
    }

    name_buf = (char*)malloc(LINA_NAME_LENGTH);
    if (!name_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for name buffer");
        res.payload.message = msg;
        return res;
    }
    
    memset(name_buf, 0, LINA_NAME_LENGTH);
    memcpy(name_buf, name, strlen(name));

    length = to_array(data_len, 4, true);
    if (!length) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for length buffer");
        res.payload.message = msg;
        goto cleanup;
    }

    CRC32 crc32 = CRC32_init();
    CRC32_update(&crc32, (uint8_t*)name_buf, LINA_NAME_LENGTH);
    CRC32_update(&crc32, length, 4);
    CRC32_update(&crc32, (uint8_t*)data, data_len);

    checksum = to_array(CRC32_finalize(&crc32), 4, true);
    if (!checksum) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for checksum buffer");
        res.payload.message = msg;
        goto cleanup;
    }

    // Connect to LiNa server
    if (!_connect(client)) {
        snprintf(msg, MAX_MSG_LEN, "Failed to connect to server");
        res.payload.message = msg;
        goto cleanup;
    }

    #ifdef _WIN32
        WSABUF buffers[4] = {
            { .len = 1, .buf = (char*)&flags },
            { .len = LINA_NAME_LENGTH, .buf = name_buf },
            { .len = 4, .buf = (char*)checksum },
            { .len = data_len, .buf = data }
        };
    #else
        struct iovec buffers[4] = {
            { .iov_len = 1, .iov_base = &flags },
            { .iov_len = LINA_NAME_LENGTH, .iov_base = name_buf },
            { .iov_len = 4, .iov_base = checksum },
            { .iov_len = data_len, .iov_base = data }
        };
    #endif

    if((sock_status = check_sendv(client, buffers, 4, "Upload file")) <= 0) {
        snprintf(msg, MAX_MSG_LEN, "Failed to send upload data");
        goto cleanup;
    }
    
    header_buf = (char* )malloc(LINA_HEADER_LENGTH);
    if (!header_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for header buffer");
        goto cleanup;
    }
    
    if((sock_status = recv(client->sock, header_buf, LINA_HEADER_LENGTH, 0)) <= 0) {
        if (sock_status == 0) {
            snprintf(msg, MAX_MSG_LEN, "Connection closed while receiving response");
        } else {
            #ifdef _WIN32
                snprintf(msg, MAX_MSG_LEN, "Winsock error: %d", WSAGetLastError());
            #else
                snprintf(msg, MAX_MSG_LEN, "errno: %d", errno);
            #endif
        }
        free(header_buf);
        goto cleanup;
    }
    
    if (header_buf[0] != 0) {
        snprintf(msg, MAX_MSG_LEN, "Server returned error code: %d", header_buf[0]);
        free(header_buf);
        goto cleanup;
    }
    
    free(header_buf);
    _disconnect(client);

    res.status = true;
    res.payload.data = NULL;

    goto cleanup;

cleanup:
    if (name_buf) free(name_buf);
    if (length) free(length);
    if (checksum) free(checksum);
    if (!res.status) {
        _disconnect(client);
    }
    return res;
}

LiNaResult downloadFile(LiNaClient* client, char* name) {
    LiNaResult res = { .status = false };
    char *msg = (char *)malloc(MAX_MSG_LEN);
    char* name_buf = NULL;
    uint8_t* length = NULL;
    uint8_t* checksum = NULL;
    char* header_buf = NULL;
    char* data_recv = NULL;
    
    if (!client || !name) {
        snprintf(msg, MAX_MSG_LEN, "Invalid parameters: client or name is NULL");
        res.payload.message = msg;
        return res;
    }

    uint8_t flags = LINA_READ;
    if (strlen(name) > LINA_NAME_LENGTH)
    {
        snprintf(msg, MAX_MSG_LEN, "File name is too long: %zu > %d", strlen(name), LINA_NAME_LENGTH);
        res.payload.message = msg;
        return res;
    }

    name_buf = (char*)malloc(LINA_NAME_LENGTH);
    if (!name_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for name buffer");
        res.payload.message = msg;
        return res;
    }
    
    memset(name_buf, 0, LINA_NAME_LENGTH);
    memcpy(name_buf, name, strlen(name));

    length = to_array(0, 4, true);
    if (!length) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for length buffer");
        res.payload.message = msg;
        free(name_buf);
        return res;
    }

    CRC32 crc32 = CRC32_init();
    CRC32_update(&crc32, (uint8_t*)name_buf, LINA_NAME_LENGTH);
    CRC32_update(&crc32, length, 4);

    checksum = to_array(CRC32_finalize(&crc32), 4, true);
    if (!checksum) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for checksum buffer");
        res.payload.message = msg;
        free(name_buf);
        free(length);
        return res;
    }

    int8_t sock_status = 0;
    // Connect to LiNa server
    if (!_connect(client)) {
        snprintf(msg, MAX_MSG_LEN, "Failed to connect to server");
        res.payload.message = msg;
        goto cleanup;
    }
    #ifdef _WIN32
        WSABUF buffers[3] = {
            { .len = 1, .buf = (char*)&flags },
            { .len = LINA_NAME_LENGTH, .buf = name_buf },
            { .len = 4, .buf = (char*)checksum }
        };
    #else
        // POSIX implementation using writev
        struct iovec buffers[3] = {
            { .iov_base = &flags, .iov_len = 1 },
            { .iov_base = name_buf, .iov_len = LINA_NAME_LENGTH },
            { .iov_base = checksum, .iov_len = 4 },
        };
    #endif
    if((sock_status = check_sendv(client, buffers, 3, "Download file")) <= 0) {
        snprintf(msg, MAX_MSG_LEN, "Failed to send download request");
        goto cleanup;
    }
    
    header_buf = (char* )malloc(LINA_HEADER_LENGTH);
    if (!header_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for header buffer");
        goto cleanup;
    }
    
    if(recv(client->sock, header_buf, LINA_HEADER_LENGTH, 0) <= 0) {
        if (sock_status == 0) {
            snprintf(msg, MAX_MSG_LEN, "Connection closed while receiving header");
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

    int p = 1;
    char* name_recv = header_buf + p;
    p += LINA_NAME_LENGTH;
    uint32_t length_recv = to_long(header_buf + p, 4, true);
    p += 4;
    uint32_t checksum_recv = to_long(header_buf + p, 4, true);
    
    if (length_recv > 0) {
        data_recv = (char* )malloc(length_recv + 1); // +1 for null terminator
        if (!data_recv) {
            snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for data buffer");
            goto cleanup;
        }
        
        if((sock_status = recv(client->sock, data_recv, length_recv, 0)) <= 0) {
            if (sock_status == 0) {
                snprintf(msg, MAX_MSG_LEN, "Connection closed while receiving data");
            } else {
                #ifdef _WIN32
                    snprintf(msg, MAX_MSG_LEN, "Winsock error: %d", WSAGetLastError());
                #else
                    snprintf(msg, MAX_MSG_LEN, "errno: %d", errno);
                #endif
            }
            goto cleanup;
        }
        
        if (sock_status < length_recv) {
            snprintf(msg, MAX_MSG_LEN, "Incomplete data received: %d < %u", sock_status, length_recv);
            goto cleanup;
        }
        
        data_recv[length_recv] = '\0'; // Null terminate for safety
    }

    CRC32_update(&crc32, name_recv, LINA_NAME_LENGTH);
    CRC32_update(&crc32, (uint8_t*)&length_recv, 4);
    CRC32_update(&crc32, (uint8_t*)data_recv, length_recv);

    if(CRC32_finalize(&crc32) != checksum_recv) {
        snprintf(msg, MAX_MSG_LEN, "Checksum verification failed");
        goto cleanup;
    }

    _disconnect(client);

    res.status = true;
    res.payload.data = data_recv;

    goto cleanup;

cleanup:
    if (name_buf) free(name_buf);
    if (length) free(length);
    if (checksum) free(checksum);
    if (header_buf) free(header_buf);
    if (!res.status) {
        if (data_recv) free(data_recv);
        _disconnect(client);
    }
    return res;
}

LiNaResult deleteFile(LiNaClient* client, char* name)
{
    LiNaResult res = { .status = false };
    char *msg = (char *)malloc(MAX_MSG_LEN);
    char* name_buf = NULL;
    uint8_t* length = NULL;
    uint8_t* checksum = NULL;
    char* header_buf = NULL;
    
    if (!client || !name) {
        snprintf(msg, MAX_MSG_LEN, "Invalid parameters: client or name is NULL");
        res.payload.message = msg;
        return res;
    }

    uint8_t flags = LINA_DELETE;
    if (strlen(name) > LINA_NAME_LENGTH)
    {
        snprintf(msg, MAX_MSG_LEN, "File name is too long: %zu > %d", strlen(name), LINA_NAME_LENGTH);
        res.payload.message = msg;
        return res;
    }
    
    name_buf = (char*)malloc(LINA_NAME_LENGTH);
    if (!name_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for name buffer");
        res.payload.message = msg;
        return res;
    }
    
    memset(name_buf, 0, LINA_NAME_LENGTH);
    memcpy(name_buf, name, strlen(name));

    length = to_array(0, 4, true); // Delete operation uses 0 length
    if (!length) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for length buffer");
        res.payload.message = msg;
        free(name_buf);
        return res;
    }

    CRC32 crc32 = CRC32_init();
    CRC32_update(&crc32, (uint8_t*)name_buf, LINA_NAME_LENGTH);
    CRC32_update(&crc32, length, 4);

    checksum = to_array(CRC32_finalize(&crc32), 4, true);
    if (!checksum) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for checksum buffer");
        res.payload.message = msg;
        free(name_buf);
        free(length);
        return res;
    }

    int8_t sock_status = 0;
    // Connect to LiNa server
    if (!_connect(client)) {
        snprintf(msg, MAX_MSG_LEN, "Failed to connect to server");
        res.payload.message = msg;
        goto cleanup;
    }
    #ifdef _WIN32
        WSABUF buffers[3] = {
            { .len = 1, .buf = (char*)&flags },
            { .len = LINA_NAME_LENGTH, .buf = name_buf },
            { .len = 4, .buf = (char*)checksum }
        };
    #else
        // POSIX implementation using writev
        struct iovec buffers[3] = {
            { .iov_base = &flags, .iov_len = 1 },
            { .iov_base = name_buf, .iov_len = LINA_NAME_LENGTH },
            { .iov_base = checksum, .iov_len = 4 },
        };
    #endif

    if((sock_status = check_sendv(client, buffers, 3, "Delete file")) <= 0) {
        snprintf(msg, MAX_MSG_LEN, "Failed to send delete request");
        goto cleanup;
    }
    
    header_buf = (char* )malloc(LINA_HEADER_LENGTH);
    if (!header_buf) {
        snprintf(msg, MAX_MSG_LEN, "Memory allocation failed for header buffer");
        goto cleanup;
    }
    
    if((sock_status = recv(client->sock, header_buf, LINA_HEADER_LENGTH, 0)) <= 0) {
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
    
    _disconnect(client);

    res.status = true;
    res.payload.data = NULL;

    goto cleanup;

cleanup:
    if (name_buf) free(name_buf);
    if (length) free(length);
    if (checksum) free(checksum);
    if (header_buf) free(header_buf);
    if (!res.status) {
        _disconnect(client);
    }
    return res;
}

void freeResult(LiNaResult* result) {
    if (!result) {
        return;
    }
    
    if (!result->status) {
        if (result->payload.message) {
            free(result->payload.message);
        }
    } else {
        if (result->payload.data) {
            free(result->payload.data);
        }
    }
}
