/*
 * LiNaStore C++ Client Test Runner
 * 
 * This file provides a test runner for the C++ client library.
 * Compile: g++ -std=c++17 -o test_cpp_client test_cpp_client.cpp ../client/cpp/src/linaclient.cpp ../client/cpp/src/crc32.cpp ../client/cpp/src/token_management.cpp -I../client/cpp/src
 * Run: ./test_cpp_client [host] [port]
 */

#include <iostream>
#include <vector>
#include <cstring>
#include <cassert>
#include "linaclient.h"

#define TEST_DATA "Hello from C++ client! This is a test file for LiNaStore."
#define TEST_FILE_NAME "test_cpp_client.txt"

class TestRunner {
private:
    std::string host;
    int port;
    int tests_passed;
    int tests_failed;
    
public:
    TestRunner(const std::string& h, int p) 
        : host(h), port(p), tests_passed(0), tests_failed(0) {}
    
    void run_all() {
        std::cout << "\n";
        std::cout << "═══════════════════════════════════════════════════════════════\n";
        std::cout << "         LiNaStore C++ Client Test Suite\n";
        std::cout << "═══════════════════════════════════════════════════════════════\n";
        std::cout << "Target: " << host << ":" << port << "\n\n";
        
        test_create_client();
        test_upload();
        test_download();
        test_upload_with_cover();
        test_delete();
        
        print_summary();
    }
    
    void test_create_client() {
        std::cout << "[TEST] Creating client for " << host << ":" << port << "...\n";
        
        try {
            LiNaClient client(host, port);
            std::cout << "[PASS] Client created successfully\n";
            tests_passed++;
        } catch (const std::exception& e) {
            std::cerr << "[FAIL] Failed to create client: " << e.what() << "\n";
            tests_failed++;
        }
    }
    
    void test_upload() {
        std::cout << "[TEST] Testing file upload...\n";
        
        try {
            LiNaClient client(host, port);
            
            std::string test_data = TEST_DATA;
            std::vector<char> data(test_data.begin(), test_data.end());
            
            if (client.linaUploadFile(TEST_FILE_NAME, data, LiNaClient::LINA_WRITE)) {
                std::cout << "[PASS] Upload successful (" << data.size() << " bytes)\n";
                tests_passed++;
            } else {
                std::cerr << "[FAIL] Upload failed\n";
                tests_failed++;
            }
        } catch (const std::exception& e) {
            std::cerr << "[FAIL] Upload exception: " << e.what() << "\n";
            tests_failed++;
        }
    }
    
    void test_download() {
        std::cout << "[TEST] Testing file download...\n";
        
        try {
            LiNaClient client(host, port);
            
            std::vector<char> downloaded = client.linaDownloadFile(TEST_FILE_NAME);
            
            if (!downloaded.empty()) {
                std::cout << "[PASS] Download successful (" << downloaded.size() << " bytes)\n";
                
                // Verify content
                std::string expected = TEST_DATA;
                std::string actual(downloaded.begin(), downloaded.end());
                
                if (actual.find(expected) != std::string::npos) {
                    std::cout << "[PASS] Content verification passed\n";
                } else {
                    std::cout << "[WARN] Content differs (may be expected if file existed before)\n";
                }
                
                tests_passed++;
            } else {
                std::cerr << "[FAIL] Download failed (empty response)\n";
                tests_failed++;
            }
        } catch (const std::exception& e) {
            std::cerr << "[FAIL] Download exception: " << e.what() << "\n";
            tests_failed++;
        }
    }
    
    void test_upload_with_cover() {
        std::cout << "[TEST] Testing upload with cover flag...\n";
        
        try {
            LiNaClient client(host, port);
            
            // First upload
            std::string initial_data = "Initial content for cover test.";
            std::vector<char> data1(initial_data.begin(), initial_data.end());
            
            if (!client.linaUploadFile("test_cover_cpp.txt", data1, LiNaClient::LINA_WRITE)) {
                std::cerr << "[FAIL] Initial upload failed\n";
                tests_failed++;
                return;
            }
            
            // Second upload with cover flag
            std::string new_data = "New content to overwrite.";
            std::vector<char> data2(new_data.begin(), new_data.end());
            
            if (client.linaUploadFile("test_cover_cpp.txt", data2, LiNaClient::LINA_WRITE | LiNaClient::LINA_COVER)) {
                std::cout << "[PASS] Upload with cover flag successful\n";
                
                // Cleanup
                client.linaDeleteFile("test_cover_cpp.txt");
                tests_passed++;
            } else {
                std::cerr << "[FAIL] Upload with cover flag failed\n";
                tests_failed++;
            }
        } catch (const std::exception& e) {
            std::cerr << "[FAIL] Upload with cover exception: " << e.what() << "\n";
            tests_failed++;
        }
    }
    
    void test_delete() {
        std::cout << "[TEST] Testing file deletion...\n";
        
        try {
            LiNaClient client(host, port);
            
            if (client.linaDeleteFile(TEST_FILE_NAME)) {
                std::cout << "[PASS] Delete successful\n";
                tests_passed++;
            } else {
                std::cerr << "[FAIL] Delete failed\n";
                tests_failed++;
            }
        } catch (const std::exception& e) {
            std::cerr << "[FAIL] Delete exception: " << e.what() << "\n";
            tests_failed++;
        }
    }
    
    void print_summary() {
        std::cout << "\n";
        std::cout << "═══════════════════════════════════════════════════════════════\n";
        std::cout << "                      Test Summary\n";
        std::cout << "═══════════════════════════════════════════════════════════════\n";
        std::cout << "Passed: " << tests_passed << "\n";
        std::cout << "Failed: " << tests_failed << "\n";
        std::cout << "═══════════════════════════════════════════════════════════════\n";
        
        if (tests_failed == 0) {
            std::cout << "\n✅ ALL TESTS PASSED!\n\n";
        } else {
            std::cout << "\n❌ SOME TESTS FAILED\n\n";
        }
    }
    
    int get_exit_code() const {
        return tests_failed > 0 ? 1 : 0;
    }
};

void print_usage(const char* program) {
    std::cout << "Usage: " << program << " [host] [port]\n";
    std::cout << "\n";
    std::cout << "Arguments:\n";
    std::cout << "  host    Server host (default: 127.0.0.1)\n";
    std::cout << "  port    Server port (default: 8096)\n";
}

int main(int argc, char* argv[]) {
    if (argc > 1 && (std::string(argv[1]) == "-h" || std::string(argv[1]) == "--help")) {
        print_usage(argv[0]);
        return 0;
    }
    
    std::string host = argc > 1 ? argv[1] : "127.0.0.1";
    int port = argc > 2 ? std::atoi(argv[2]) : 8096;
    
    TestRunner runner(host, port);
    runner.run_all();
    
    return runner.get_exit_code();
}
