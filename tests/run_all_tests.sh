#!/bin/bash
# =============================================================================
# LiNaStore All-in-One Test Script
# =============================================================================
# This script runs all tests including:
# - Rust backend tests (cargo test)
# - Python client tests
# - C client tests
# - C++ client tests
# - Java client tests
# =============================================================================

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Project root directory
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$PROJECT_ROOT"

# Set TMPDIR to project local temp directory if not set or if /tmp is not writable
if [ -z "$TMPDIR" ] || [ ! -w "/tmp" ]; then
    export TMPDIR="$PROJECT_ROOT/.tmp"
    mkdir -p "$TMPDIR"
fi

# Test configuration
SERVER_HOST="${SERVER_HOST:-127.0.0.1}"
SERVER_PORT="${SERVER_PORT:-8096}"
HTTP_PORT="${HTTP_PORT:-8086}"
ADMIN_USER="${ADMIN_USER:-admin}"
ADMIN_PASSWORD="${ADMIN_PASSWORD:-admin123}"
TEST_DATA_SIZE="${TEST_DATA_SIZE:-1024}"

# Tracking variables
TESTS_PASSED=0
TESTS_FAILED=0
FAILED_TESTS=""

# =============================================================================
# Utility Functions
# =============================================================================

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[PASS]${NC} $1"
    ((TESTS_PASSED++))
}

log_error() {
    echo -e "${RED}[FAIL]${NC} $1"
    ((TESTS_FAILED++))
    FAILED_TESTS="$FAILED_TESTS\n- $1"
}

log_warning() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_section() {
    echo ""
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
    echo -e "${BLUE}  $1${NC}"
    echo -e "${BLUE}═══════════════════════════════════════════════════════════════${NC}"
}

check_command() {
    if ! command -v "$1" &> /dev/null; then
        log_warning "$1 not found, skipping related tests"
        return 1
    fi
    return 0
}

port_in_use() {
    local host="$1"
    local port="$2"
    if command -v nc >/dev/null 2>&1; then
        nc -z "$host" "$port" >/dev/null 2>&1
        return $?
    fi
    (echo > "/dev/tcp/$host/$port") >/dev/null 2>&1
}

wait_for_server() {
    local host="$1"
    local port="$2"
    local timeout="${3:-30}"
    local start_time=$(date +%s)
    
    log_info "Waiting for server at $host:$port..."
    
    while true; do
        if command -v nc >/dev/null 2>&1; then
            nc -z "$host" "$port" 2>/dev/null && {
                log_info "Server is ready at $host:$port"
                return 0
            }
        else
            (echo > "/dev/tcp/$host/$port") >/dev/null 2>&1 && {
                log_info "Server is ready at $host:$port"
                return 0
            }
        fi
        
        local current_time=$(date +%s)
        local elapsed=$((current_time - start_time))
        
        if [ $elapsed -ge $timeout ]; then
            log_error "Server at $host:$port did not start within ${timeout}s"
            return 1
        fi
        
        sleep 0.5
    done
}

# =============================================================================
# Server Management
# =============================================================================

start_server() {
    log_info "Building LiNaStore server..."
    cargo build --bins --quiet
    
    log_info "Starting LiNaStore server..."
    
    # Create temporary storage directory
    export TEST_STORAGE_DIR=$(mktemp -d)
    export TEST_DB_PATH="$TEST_STORAGE_DIR/test.db"
    
    local attempt=0
    local max_attempts=10

    while [ $attempt -lt $max_attempts ]; do
        local try_http_port=$((20000 + RANDOM % 20000))
        local try_server_port=$((try_http_port + 10))
        if [ $try_server_port -gt 65000 ]; then
            attempt=$((attempt + 1))
            continue
        fi

        # Start server in background with nohup to fully detach from terminal
        nohup env \
            RUST_LOG=info \
            LINASTORE_STORAGE_DIR="$TEST_STORAGE_DIR" \
            LINASTORE_DB_URL="sqlite:$TEST_DB_PATH" \
            LINASTORE_HTTP_PORT="$try_http_port" \
            LINASTORE_ADVANCED_PORT="$try_server_port" \
            ./target/debug/linastore-server start --foreground > /tmp/linastore_test.log 2>&1 &
        
        SERVER_PID=$!
        disown $SERVER_PID 2>/dev/null || true
        echo $SERVER_PID > /tmp/linastore_test.pid
        
        log_info "Server PID: $SERVER_PID"
        
        # Wait for server to be ready
        if wait_for_server "$SERVER_HOST" "$try_server_port" 30 && \
           wait_for_server "$SERVER_HOST" "$try_http_port" 10; then
            SERVER_PORT="$try_server_port"
            HTTP_PORT="$try_http_port"
            return 0
        fi

        log_warning "Port attempt failed (lina=$try_server_port http=$try_http_port); retrying..."
        cat /tmp/linastore_test.log
        kill $SERVER_PID 2>/dev/null || true
        rm -f /tmp/linastore_test.pid
        sleep 1
        attempt=$((attempt + 1))
    done

    log_error "Failed to start server"
    cat /tmp/linastore_test.log
    return 1
}

stop_server() {
    if [ -f /tmp/linastore_test.pid ]; then
        local pid=$(cat /tmp/linastore_test.pid)
        log_info "Stopping server (PID: $pid)..."
        kill $pid 2>/dev/null || true
        rm -f /tmp/linastore_test.pid
        sleep 1
    fi
    
    # Cleanup temp directory
    if [ -n "$TEST_STORAGE_DIR" ] && [ -d "$TEST_STORAGE_DIR" ]; then
        rm -rf "$TEST_STORAGE_DIR"
    fi
}

# =============================================================================
# Test Functions
# =============================================================================

test_rust_backend() {
    log_section "Testing Rust Backend"
    
    if ! check_command cargo; then
        log_error "cargo not found"
        return 1
    fi
    
    log_info "Running cargo test..."
    
    if cargo test --all 2>&1 | tee /tmp/cargo_test.log; then
        log_success "Rust backend tests"
    else
        log_error "Rust backend tests"
        return 1
    fi
}

test_python_client() {
    log_section "Testing Python Client"
    
    if ! check_command python3; then
        log_error "Python3 not found"
        return 1
    fi
    
    # Run the Python client test
    log_info "Running Python client test..."
    
    if PYTHONPATH="$PROJECT_ROOT/client/python:$PYTHONPATH" \
       python3 "$PROJECT_ROOT/tests/test_python_client.py" \
       --mode auth-free \
       --host "$SERVER_HOST" \
       --port "$SERVER_PORT" \
       --data-size "$TEST_DATA_SIZE" 2>&1 | tee /tmp/python_test.log; then
        log_success "Python client tests"
    else
        log_error "Python client tests"
        return 1
    fi
}

test_c_client() {
    log_section "Testing C Client"
    
    if ! check_command gcc; then
        log_error "GCC not found"
        return 1
    fi
    
    local TEST_BIN="$PROJECT_ROOT/tests/test_c_client"
    
    log_info "Building C client test runner..."
    
    # Build C client test
    if gcc -o "$TEST_BIN" \
        "$PROJECT_ROOT/tests/test_c_client.c" \
        "$PROJECT_ROOT/client/c/src/linaclient.c" \
        "$PROJECT_ROOT/client/c/src/crc32.c" \
        -I"$PROJECT_ROOT/client/c/src" \
        -lssl -lcrypto 2>&1; then
        
        log_info "Running C client tests..."
        if "$TEST_BIN" "$SERVER_HOST" "$SERVER_PORT" 2>&1 | tee /tmp/c_test.log; then
            log_success "C client tests"
        else
            log_error "C client tests"
            rm -f "$TEST_BIN"
            return 1
        fi
    else
        log_error "C client build failed"
        return 1
    fi
    
    # Cleanup
    rm -f "$TEST_BIN"
    return 0
}

test_cpp_client() {
    log_section "Testing C++ Client"
    
    if ! check_command g++; then
        log_error "G++ not found"
        return 1
    fi
    
    local TEST_BIN="$PROJECT_ROOT/tests/test_cpp_client"
    
    log_info "Building C++ client test runner..."
    
    # Build C++ client test
    if g++ -std=c++17 -o "$TEST_BIN" \
        "$PROJECT_ROOT/tests/test_cpp_client.cpp" \
        "$PROJECT_ROOT/client/cpp/src/linaclient.cpp" \
        "$PROJECT_ROOT/client/cpp/src/crc32.cpp" \
        "$PROJECT_ROOT/client/cpp/src/token_management.cpp" \
        -I"$PROJECT_ROOT/client/cpp/src" \
        -lssl -lcrypto 2>&1; then
        
        log_info "Running C++ client tests..."
        if "$TEST_BIN" "$SERVER_HOST" "$SERVER_PORT" 2>&1 | tee /tmp/cpp_test.log; then
            log_success "C++ client tests"
        else
            log_error "C++ client tests"
            rm -f "$TEST_BIN"
            return 1
        fi
    else
        log_error "C++ client build failed"
        return 1
    fi
    
    # Cleanup
    rm -f "$TEST_BIN"
    return 0
}

test_java_client() {
    log_section "Testing Java Client"
    
    if ! check_command mvn; then
        log_error "Maven (mvn) not found"
        return 1
    fi
    
    if ! check_command java; then
        log_error "Java not found"
        return 1
    fi
    
    local JAVA_DIR="$PROJECT_ROOT/client/java"
    local TEST_SRC="$JAVA_DIR/src/main/java/com/aimerick/linastore/TestRunner.java"
    
    log_info "Copying Java test runner to client directory..."
    cp "$PROJECT_ROOT/tests/TestJavaClient.java" "$TEST_SRC"
    
    log_info "Building Java client..."
    cd "$JAVA_DIR"
    
    if mvn clean compile -q 2>&1 | tee /tmp/java_build.log; then
        log_info "Java client compiled successfully"
    else
        log_error "Java client build failed"
        rm -f "$TEST_SRC"
        cd "$PROJECT_ROOT"
        return 1
    fi
    
    log_info "Running Java client tests..."
    if mvn exec:java -Dexec.mainClass="com.aimerick.linastore.TestRunner" \
       -Dexec.args="$SERVER_HOST $SERVER_PORT" 2>&1 | tee /tmp/java_test.log; then
        log_success "Java client tests"
    else
        log_error "Java client tests"
        rm -f "$TEST_SRC"
        cd "$PROJECT_ROOT"
        return 1
    fi
    
    # Cleanup
    rm -f "$TEST_SRC"
    cd "$PROJECT_ROOT"
    return 0
}

# =============================================================================
# Main Test Runner
# =============================================================================

print_summary() {
    log_section "Test Summary"
    
    echo ""
    echo -e "Tests Passed: ${GREEN}$TESTS_PASSED${NC}"
    echo -e "Tests Failed: ${RED}$TESTS_FAILED${NC}"
    
    if [ -n "$FAILED_TESTS" ]; then
        echo ""
        echo -e "${RED}Failed tests:${NC}"
        echo -e "$FAILED_TESTS"
    fi
    
    echo ""
    
    if [ $TESTS_FAILED -eq 0 ]; then
        echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
        echo -e "${GREEN}  ALL TESTS PASSED! 🎉${NC}"
        echo -e "${GREEN}═══════════════════════════════════════════════════════════════${NC}"
        return 0
    else
        echo -e "${RED}═══════════════════════════════════════════════════════════════${NC}"
        echo -e "${RED}  SOME TESTS FAILED${NC}"
        echo -e "${RED}═══════════════════════════════════════════════════════════════${NC}"
        return 1
    fi
}

cleanup() {
    log_info "Cleaning up..."
    stop_server
    rm -f /tmp/cargo_test.log /tmp/python_test.log /tmp/c_test.log \
          /tmp/cpp_test.log /tmp/java_test.log /tmp/java_build.log \
          /tmp/linastore_test.log
}

main() {
    # Parse arguments
    local RUN_BACKEND=true
    local RUN_PYTHON=true
    local RUN_C=true
    local RUN_CPP=true
    local RUN_JAVA=true
    local START_SERVER=true
    
    while [[ $# -gt 0 ]]; do
        case $1 in
            --no-backend)
                RUN_BACKEND=false
                shift
                ;;
            --no-python)
                RUN_PYTHON=false
                shift
                ;;
            --no-c)
                RUN_C=false
                shift
                ;;
            --no-cpp)
                RUN_CPP=false
                shift
                ;;
            --no-java)
                RUN_JAVA=false
                shift
                ;;
            --no-server)
                START_SERVER=false
                shift
                ;;
            --backend-only)
                RUN_PYTHON=false
                RUN_C=false
                RUN_CPP=false
                RUN_JAVA=false
                shift
                ;;
            --clients-only)
                RUN_BACKEND=false
                shift
                ;;
            --help)
                echo "Usage: $0 [options]"
                echo ""
                echo "Options:"
                echo "  --no-backend     Skip Rust backend tests"
                echo "  --no-python      Skip Python client tests"
                echo "  --no-c           Skip C client tests"
                echo "  --no-cpp         Skip C++ client tests"
                echo "  --no-java        Skip Java client tests"
                echo "  --no-server      Don't start server (use existing)"
                echo "  --backend-only   Only run Rust backend tests"
                echo "  --clients-only   Only run client tests"
                echo "  --help           Show this help message"
                echo ""
                echo "Environment variables:"
                echo "  SERVER_HOST      Server host (default: 127.0.0.1)"
                echo "  SERVER_PORT      Server port (default: 8096)"
                echo "  HTTP_PORT        HTTP port (default: 8080)"
                echo "  ADMIN_USER       Admin username (default: admin)"
                echo "  ADMIN_PASSWORD   Admin password (default: admin123)"
                echo "  TEST_DATA_SIZE   Test data size in bytes (default: 1024)"
                exit 0
                ;;
            *)
                echo "Unknown option: $1"
                exit 1
                ;;
        esac
    done
    
    # Setup cleanup trap
    trap cleanup EXIT
    
    echo ""
    echo -e "${BLUE}╔═══════════════════════════════════════════════════════════════╗${NC}"
    echo -e "${BLUE}║              LiNaStore Comprehensive Test Suite              ║${NC}"
    echo -e "${BLUE}╚═══════════════════════════════════════════════════════════════╝${NC}"
    echo ""
    
    # Start server if needed
    if [ "$START_SERVER" = true ] && ([ "$RUN_PYTHON" = true ] || [ "$RUN_C" = true ] || [ "$RUN_CPP" = true ] || [ "$RUN_JAVA" = true ]); then
        if ! start_server; then
            log_error "Failed to start server"
            exit 1
        fi
    fi
    
    # Run backend tests (doesn't need server)
    if [ "$RUN_BACKEND" = true ]; then
        test_rust_backend || true
    fi
    
    # Run client tests
    if [ "$RUN_PYTHON" = true ]; then
        test_python_client || true
    fi
    
    if [ "$RUN_C" = true ]; then
        test_c_client || true
    fi
    
    if [ "$RUN_CPP" = true ]; then
        test_cpp_client || true
    fi
    
    if [ "$RUN_JAVA" = true ]; then
        test_java_client || true
    fi
    
    # Print summary
    print_summary
}

main "$@"
