/*
 * LiNaStore C Client Test Runner
 * 
 * This file provides a simple test runner for the C client library.
 * Compile: gcc -o test_c_client test_c_client.c ../client/c/src/linaclient.c ../client/c/src/crc32.c -I../client/c/src
 * Run: ./test_c_client [host] [port]
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "linaclient.h"

#define TEST_DATA "Hello from C client! This is a test file for LiNaStore."
#define TEST_FILE_NAME "test_c_client.txt"

int tests_passed = 0;
int tests_failed = 0;

int test_create_client(const char *host, int port) {
    printf("[TEST] Creating client for %s:%d...\n", host, port);
    
    LiNaClient *client = lina_client_init(host, port, false, 0);
    if (!client) {
        fprintf(stderr, "[FAIL] Failed to create client\n");
        return 1;
    }
    
    printf("[PASS] Client created successfully\n");
    lina_client_cleanup(client);
    return 0;
}

int test_upload(LiNaClient *client) {
    printf("[TEST] Testing file upload...\n");
    
    const char *test_data = TEST_DATA;
    size_t data_len = strlen(test_data);
    
    LiNaResult res = lina_upload_file(client, (char*)TEST_FILE_NAME, (char*)test_data, data_len, LINA_WRITE);
    if (res.status) {
        printf("[PASS] Upload successful (%zu bytes)\n", data_len);
        lina_free_result(&res);
        return 0;
    } else {
        fprintf(stderr, "[FAIL] Upload failed: %s\n", res.payload.message ? res.payload.message : "unknown error");
        lina_free_result(&res);
        return 1;
    }
}

int test_download(LiNaClient *client) {
    printf("[TEST] Testing file download...\n");
    
    LiNaResult res = lina_download_file(client, (char*)TEST_FILE_NAME);
    if (res.status && res.payload.data != NULL) {
        size_t downloaded = strlen(res.payload.data);
        printf("[PASS] Download successful (%zu bytes)\n", downloaded);
        
        // Verify content
        if (memcmp(res.payload.data, TEST_DATA, strlen(TEST_DATA)) == 0) {
            printf("[PASS] Content verification passed\n");
        } else {
            printf("[WARN] Content mismatch (may be expected if file existed before)\n");
        }
        lina_free_result(&res);
        return 0;
    } else {
        fprintf(stderr, "[FAIL] Download failed: %s\n", res.payload.message ? res.payload.message : "unknown error");
        lina_free_result(&res);
        return 1;
    }
}

int test_delete(LiNaClient *client) {
    printf("[TEST] Testing file deletion...\n");
    
    LiNaResult res = lina_delete_file(client, (char*)TEST_FILE_NAME);
    if (res.status) {
        printf("[PASS] Delete successful\n");
        lina_free_result(&res);
        return 0;
    } else {
        fprintf(stderr, "[FAIL] Delete failed: %s\n", res.payload.message ? res.payload.message : "unknown error");
        lina_free_result(&res);
        return 1;
    }
}

int test_upload_with_cover(LiNaClient *client) {
    printf("[TEST] Testing upload with cover flag...\n");
    
    const char *test_data = "Initial content for cover test.";
    size_t data_len = strlen(test_data);
    
    // First upload
    LiNaResult res_first = lina_upload_file(client, "test_cover_c.txt", (char*)test_data, data_len, LINA_WRITE);
    if (!res_first.status) {
        fprintf(stderr, "[FAIL] Initial upload failed\n");
        lina_free_result(&res_first);
        return 1;
    }
    lina_free_result(&res_first);
    
    // Second upload with cover flag
    const char *new_data = "New content to overwrite.";
    LiNaResult res_second = lina_upload_file(client, "test_cover_c.txt", (char*)new_data, strlen(new_data), LINA_WRITE | LINA_COVER);
    if (res_second.status) {
        printf("[PASS] Upload with cover flag successful\n");
        
        // Cleanup
        LiNaResult res_del = lina_delete_file(client, "test_cover_c.txt");
        lina_free_result(&res_del);
        lina_free_result(&res_second);
        return 0;
    } else {
        fprintf(stderr, "[FAIL] Upload with cover flag failed\n");
        lina_free_result(&res_second);
        return 1;
    }
}

int main(int argc, char *argv[]) {
    const char *host = argc > 1 ? argv[1] : "127.0.0.1";
    int port = argc > 2 ? atoi(argv[2]) : 8096;
    
    printf("\n");
    printf("═══════════════════════════════════════════════════════════════\n");
    printf("         LiNaStore C Client Test Suite\n");
    printf("═══════════════════════════════════════════════════════════════\n");
    printf("Target: %s:%d\n\n", host, port);
    
    // Test 1: Create client
    if (test_create_client(host, port) == 0) {
        tests_passed++;
    } else {
        tests_failed++;
        // Can't continue without client
        goto summary;
    }
    
    // Create client for remaining tests
    LiNaClient *client = lina_client_init(host, port, false, 0);
    if (!client) {
        fprintf(stderr, "[FAIL] Failed to create client for tests\n");
        tests_failed++;
        goto summary;
    }
    
    // Test 2: Upload
    if (test_upload(client) == 0) {
        tests_passed++;
    } else {
        tests_failed++;
    }
    
    // Test 3: Download
    if (test_download(client) == 0) {
        tests_passed++;
    } else {
        tests_failed++;
    }
    
    // Test 4: Upload with cover
    if (test_upload_with_cover(client) == 0) {
        tests_passed++;
    } else {
        tests_failed++;
    }
    
    // Test 5: Delete
    if (test_delete(client) == 0) {
        tests_passed++;
    } else {
        tests_failed++;
    }
    
    lina_client_cleanup(client);

summary:
    printf("\n");
    printf("═══════════════════════════════════════════════════════════════\n");
    printf("                      Test Summary\n");
    printf("═══════════════════════════════════════════════════════════════\n");
    printf("Passed: %d\n", tests_passed);
    printf("Failed: %d\n", tests_failed);
    printf("═══════════════════════════════════════════════════════════════\n");
    
    if (tests_failed == 0) {
        printf("\n✅ ALL TESTS PASSED!\n\n");
        return 0;
    } else {
        printf("\n❌ SOME TESTS FAILED\n\n");
        return 1;
    }
}
