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
        int result = WSASend(client->sock, (WSABUF*)buffers, 4, &bytesSent, 0, NULL, NULL);
        if (result == SOCKET_ERROR) 
            return SOCKET_ERROR;
        return bytesSent == (1 + LINA_NAME_LENGTH + 4 + ((WSABUF*)buffers)[buffer_count - 1].len);
    #else
        // POSIX implementation using writev
        ssize_t result = writev(client->sock, (struct iovec*)buffers, 4);
        if (result == SOCKET_ERROR) 
            return SOCKET_ERROR;
        return result == (1 + LINA_NAME_LENGTH + 4 + ((struct iovec*)buffers)[buffer_count - 1].iov_len);
    #endif
    return true;
}



bool _connect(LiNaClient *client)
{
    return connect(&client->sock, (struct sockaddr *)&client->sock, sizeof(client->sock));
}

bool _disconnect(LiNaClient *client)
{
#ifdef _WIN32
    return closesocket(client->sock) == 0;
#else
    return close(client->sock) == 0;
#endif
}

LiNaResult uploadFile(LiNaClient *client, char *name, char *data, size_t data_len, uint8_t flags)
{
    LiNaResult res = { .status = false };
    char *msg = (char *)malloc(MAX_MSG_LEN);

    if (strlen(name) > LINA_NAME_LENGTH)
    {
        strcpy(msg, "File name is too long");
        res.payload.message = &msg;
        return res;
    }

    char* name_buf = (char*)malloc(LINA_NAME_LENGTH);
    // \0 is added by strncpy
    if (strlen(name) > LINA_NAME_LENGTH)
    {
        strcpy(msg, "File name is too long");
        res.payload.message = &msg;
        return res;
    }
    memcpy(name_buf, name, strlen(name));

    uint8_t* length = to_array(data_len, 4, true);

    CRC32 crc32 = CRC32_init();
    CRC32_update(&crc32, (uint8_t*)name_buf, LINA_NAME_LENGTH);
    CRC32_update(&crc32, length, 4);
    CRC32_update(&crc32, (uint8_t*)data, data_len);

    uint8_t* checksum = to_array(CRC32_finalize(&crc32), 4, true);

    int8_t sock_status = 0;
    // Connect to LiNa server
    __connect(client);

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

    if((sock_status = check_sendv(client, buffers, 4, "Upload file")) <= 0)
        goto error;
    
    char* header_buf = (char* )malloc(LINA_HEADER_LENGTH);
    if((sock_status = recv(client->sock, header_buf, LINA_HEADER_LENGTH, 0)) <= 0)
        goto error;
    __disconnect(client);

    free(name_buf);
    free(length);
    
    res.status = true;
    res.payload.data = NULL;

    return res;

error:
    __disconnect(client);
    if(sock_status == 0)
        #ifdef _WIN32
            snprintf(msg, MAX_MSG_LEN, "Winsock error: %d", WSAGetLastError());
        #else
            snprintf(msg, MAX_MSG_LEN, "errno: %d", errno);
        #endif
    else if(sock_status == -1)
        snprintf(msg, MAX_MSG_LEN, "Socket closed unexpectedly");
    res.payload.message = &msg;
    return res;
}

LiNaResult downloadFile(LiNaClient* client, char* name) {
    LiNaResult res = { .status = false };
    char *msg = (char *)malloc(MAX_MSG_LEN);

    uint8_t flags = LINA_READ;
    if (strlen(name) > LINA_NAME_LENGTH)
    {
        strcpy(msg, "File name is too long");
        res.payload.message = &msg;
        return res;
    }

    char* name_buf = (char*)malloc(LINA_NAME_LENGTH);
    // \0 is added by strncpy
    if (strlen(name) > LINA_NAME_LENGTH)
    {
        strcpy(msg, "File name is too long");
        res.payload.message = &msg;
        return res;
    }
    memcpy(name_buf, name, strlen(name));

    uint8_t* length = to_array(0, 4, true);

    CRC32 crc32 = CRC32_init();
    CRC32_update(&crc32, (uint8_t*)name_buf, LINA_NAME_LENGTH);
    CRC32_update(&crc32, length, 4);

    uint8_t* checksum = to_array(CRC32_finalize(&crc32), 4, true);

    int8_t sock_status = 0;
    // Connect to LiNa server
    __connect(client);
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
    if((sock_status = check_sendv(client, buffers, 4, "Upload file")) <= 0)
        goto error;
    
    char* header_buf = (char* )malloc(LINA_HEADER_LENGTH);
    if(recv(client->sock, header_buf, LINA_HEADER_LENGTH, 0) <= 0)
        goto error;

    int p = 1;
    char* name_recv = header_buf + p;
    p += LINA_NAME_LENGTH;
    uint32_t length_recv = to_long(header_buf + p, 4, true);
    p += 4;
    uint32_t checksum_recv = to_long(header_buf + p, 4, true);
    
    char* data_recv = (char* )malloc(length_recv);
    if((sock_status = recv(client->sock, data_recv, length_recv, 0)) <= 0)
        goto error;

    CRC32_update(&crc32, name_recv, LINA_NAME_LENGTH);
    CRC32_update(&crc32, length_recv, 4);
    CRC32_update(&crc32, data_recv, length_recv);

    if(CRC32_finalize(&crc32) != checksum_recv)
        goto error;

    __disconnect(client);

    free(name_buf);
    free(length);

    res.status = true;
    res.payload.data = data_recv;

    return res;

error:
    __disconnect(client);
    if(sock_status == 0)
        #ifdef _WIN32
            snprintf(msg, MAX_MSG_LEN, "Winsock error: %d", WSAGetLastError());
        #else
            snprintf(msg, MAX_MSG_LEN, "errno: %d", errno);
        #endif
    else if(sock_status == -1)
        snprintf(msg, MAX_MSG_LEN, "Socket closed unexpectedly");
    res.payload.message = &msg;
    return res;
}

LiNaResult deleteFile(LiNaClient* client, char* name)
{
    LiNaResult res = { .status = false };
    char *msg = (char *)malloc(MAX_MSG_LEN);

    uint8_t flags = LINA_DELETE;
    char* name_buf = (char*)malloc(LINA_NAME_LENGTH);
    // \0 is added by strncpy
    if (strlen(name) > LINA_NAME_LENGTH)
    {
        strcpy(msg, "File name is too long");
        res.payload.message = &msg;
        return res;
    }
    memcpy(name_buf, name, strlen(name));

    uint8_t* length = to_array(strlen(name), 4, true);

    CRC32 crc32 = CRC32_init();
    CRC32_update(&crc32, (uint8_t*)name_buf, LINA_NAME_LENGTH);
    CRC32_update(&crc32, length, 4);

    uint8_t* checksum = to_array(CRC32_finalize(&crc32), 4, true);

    int8_t sock_status = 0;
    // Connect to LiNa server
    __connect(client);
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

    if((sock_status = check_sendv(client, buffers, 4, "Upload file")) <= 0)
        goto error;
    
    char* header_buf = (char* )malloc(LINA_HEADER_LENGTH);
    if((sock_status = recv(client->sock, header_buf, LINA_HEADER_LENGTH, 0)) <= 0)
        goto error;
    __disconnect(client);

    free(name_buf);
    free(length);

    res.status = true;
    res.payload.data = NULL;

    return res;

error:
    __disconnect(client);
    if(sock_status == 0)
        #ifdef _WIN32
            snprintf(msg, MAX_MSG_LEN, "Winsock error: %d", WSAGetLastError());
        #else
            snprintf(msg, MAX_MSG_LEN, "errno: %d", errno);
        #endif
    else if(sock_status == -1)
        snprintf(msg, MAX_MSG_LEN, "Socket closed unexpectedly");
    res.payload.message = &msg;
    return res;
}

void freeResult(LiNaResult* result) {
    if (!result->status) {
        free(result->payload.message);
    }
}
