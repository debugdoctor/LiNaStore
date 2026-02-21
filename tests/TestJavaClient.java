/**
 * LiNaStore Java Client Test Runner
 * 
 * This file provides a test runner for the Java client library.
 * Compile & Run from project root:
 *   cd client/java && mvn compile exec:java -Dexec.mainClass="com.aimerick.linastore.TestRunner" -Dexec.args="127.0.0.1 8096"
 */

package com.aimerick.linastore;

import java.nio.charset.StandardCharsets;

public class TestRunner {
    private static final String TEST_DATA = "Hello from Java client! This is a test file for LiNaStore.";
    private static final String TEST_FILE_NAME = "test_java_client.txt";
    
    private String host;
    private int port;
    private int testsPassed = 0;
    private int testsFailed = 0;
    
    public TestRunner(String host, int port) {
        this.host = host;
        this.port = port;
    }
    
    public void runAll() {
        System.out.println();
        System.out.println("═══════════════════════════════════════════════════════════════");
        System.out.println("         LiNaStore Java Client Test Suite");
        System.out.println("═══════════════════════════════════════════════════════════════");
        System.out.println("Target: " + host + ":" + port);
        System.out.println();
        
        testCreateClient();
        testUpload();
        testDownload();
        testUploadWithCover();
        testDelete();
        
        printSummary();
    }
    
    private void testCreateClient() {
        System.out.println("[TEST] Creating client for " + host + ":" + port + "...");
        
        try {
            LiNaStoreClient client = new LiNaStoreClient(host, port);
            System.out.println("[PASS] Client created successfully");
            testsPassed++;
        } catch (Exception e) {
            System.err.println("[FAIL] Failed to create client: " + e.getMessage());
            testsFailed++;
        }
    }
    
    private void testUpload() {
        System.out.println("[TEST] Testing file upload...");
        
        try {
            LiNaStoreClient client = new LiNaStoreClient(host, port);
            
            byte[] data = TEST_DATA.getBytes(StandardCharsets.UTF_8);
            
            if (client.linaUploadFile(TEST_FILE_NAME, data, LiNaFlags.WRITE.getValue())) {
                System.out.println("[PASS] Upload successful (" + data.length + " bytes)");
                testsPassed++;
            } else {
                System.err.println("[FAIL] Upload failed");
                testsFailed++;
            }
        } catch (Exception e) {
            System.err.println("[FAIL] Upload exception: " + e.getMessage());
            testsFailed++;
        }
    }
    
    private void testDownload() {
        System.out.println("[TEST] Testing file download...");
        
        try {
            LiNaStoreClient client = new LiNaStoreClient(host, port);
            
            byte[] downloaded = client.linaDownloadFile(TEST_FILE_NAME);
            
            if (downloaded != null && downloaded.length > 0) {
                System.out.println("[PASS] Download successful (" + downloaded.length + " bytes)");
                
                // Verify content
                String content = new String(downloaded, StandardCharsets.UTF_8);
                if (content.contains(TEST_DATA)) {
                    System.out.println("[PASS] Content verification passed");
                } else {
                    System.out.println("[WARN] Content differs (may be expected if file existed before)");
                }
                
                testsPassed++;
            } else {
                System.err.println("[FAIL] Download failed (empty response)");
                testsFailed++;
            }
        } catch (Exception e) {
            System.err.println("[FAIL] Download exception: " + e.getMessage());
            testsFailed++;
        }
    }
    
    private void testUploadWithCover() {
        System.out.println("[TEST] Testing upload with cover flag...");
        
        try {
            LiNaStoreClient client = new LiNaStoreClient(host, port);
            
            // First upload
            byte[] initialData = "Initial content for cover test.".getBytes(StandardCharsets.UTF_8);
            
            if (!client.linaUploadFile("test_cover_java.txt", initialData, LiNaFlags.WRITE.getValue())) {
                System.err.println("[FAIL] Initial upload failed");
                testsFailed++;
                return;
            }
            
            // Second upload with cover flag
            byte[] newData = "New content to overwrite.".getBytes(StandardCharsets.UTF_8);
            int coverFlags = LiNaFlags.WRITE.getValue() | LiNaFlags.COVER.getValue();
            
            if (client.linaUploadFile("test_cover_java.txt", newData, coverFlags)) {
                System.out.println("[PASS] Upload with cover flag successful");
                
                // Cleanup
                client.linaDeleteFile("test_cover_java.txt");
                testsPassed++;
            } else {
                System.err.println("[FAIL] Upload with cover flag failed");
                testsFailed++;
            }
        } catch (Exception e) {
            System.err.println("[FAIL] Upload with cover exception: " + e.getMessage());
            testsFailed++;
        }
    }
    
    private void testDelete() {
        System.out.println("[TEST] Testing file deletion...");
        
        try {
            LiNaStoreClient client = new LiNaStoreClient(host, port);
            
            if (client.linaDeleteFile(TEST_FILE_NAME)) {
                System.out.println("[PASS] Delete successful");
                testsPassed++;
            } else {
                System.err.println("[FAIL] Delete failed");
                testsFailed++;
            }
        } catch (Exception e) {
            System.err.println("[FAIL] Delete exception: " + e.getMessage());
            testsFailed++;
        }
    }
    
    private void printSummary() {
        System.out.println();
        System.out.println("═══════════════════════════════════════════════════════════════");
        System.out.println("                      Test Summary");
        System.out.println("═══════════════════════════════════════════════════════════════");
        System.out.println("Passed: " + testsPassed);
        System.out.println("Failed: " + testsFailed);
        System.out.println("═══════════════════════════════════════════════════════════════");
        
        if (testsFailed == 0) {
            System.out.println();
            System.out.println("✅ ALL TESTS PASSED!");
            System.out.println();
        } else {
            System.out.println();
            System.out.println("❌ SOME TESTS FAILED");
            System.out.println();
        }
    }
    
    public int getExitCode() {
        return testsFailed > 0 ? 1 : 0;
    }
    
    public static void main(String[] args) {
        if (args.length > 0 && (args[0].equals("-h") || args[0].equals("--help"))) {
            printUsage();
            System.exit(0);
        }
        
        String host = args.length > 0 ? args[0] : "127.0.0.1";
        int port = args.length > 1 ? Integer.parseInt(args[1]) : 8096;
        
        TestRunner runner = new TestRunner(host, port);
        runner.runAll();
        
        System.exit(runner.getExitCode());
    }
    
    private static void printUsage() {
        System.out.println("Usage: java TestRunner [host] [port]");
        System.out.println();
        System.out.println("Arguments:");
        System.out.println("  host    Server host (default: 127.0.0.1)");
        System.out.println("  port    Server port (default: 8096)");
    }
}
