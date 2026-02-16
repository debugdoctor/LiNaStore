package com.aimerick.linastore;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.security.SecureRandom;
import java.util.zip.CRC32;
import javax.crypto.Cipher;
import javax.crypto.spec.GCMParameterSpec;
import javax.crypto.spec.SecretKeySpec;

public class LiNaStoreClient {
    private Socket socket;
    private final InetSocketAddress address;
    private final int timeout;
    private final static int LINA_NAME_MAX_LENGTH = 255;
    private final static int LINA_HEADER_BASE_LENGTH = 10; // flags(1) + ilen(1) + dlen(4) + checksum(4)
    private String sessionToken = null;
    private long tokenExpiresAt = 0;
    
    // Token management
    private final boolean autoRefresh;
    private final int refreshBuffer;
    private String cachedUsername = null;
    private char[] cachedPassword = null;
    
    // Custom exceptions for better error handling
    public static class LiNaStoreException extends Exception {
        public LiNaStoreException(String message) {
            super(message);
        }
        
        public LiNaStoreException(String message, Throwable cause) {
            super(message, cause);
        }
    }
    
    public static class LiNaStoreConnectionException extends LiNaStoreException {
        public LiNaStoreConnectionException(String message) {
            super(message);
        }
        
        public LiNaStoreConnectionException(String message, Throwable cause) {
            super(message, cause);
        }
    }
    
    public static class LiNaStoreProtocolException extends LiNaStoreException {
        public LiNaStoreProtocolException(String message) {
            super(message);
        }
        
        public LiNaStoreProtocolException(String message, Throwable cause) {
            super(message, cause);
        }
    }
    
    public static class LiNaStoreChecksumException extends LiNaStoreProtocolException {
        public LiNaStoreChecksumException(String message) {
            super(message);
        }
    }

    public LiNaStoreClient(String address, int port) {
        this(address, port, 5000, true, 300);
    }

    public LiNaStoreClient(String address, int port, int timeout) {
        this(address, port, timeout, true, 300);
    }

    public LiNaStoreClient(String address, int port, int timeout, boolean autoRefresh, int refreshBuffer) {
        this.address = new InetSocketAddress(address, port);
        this.timeout = timeout;
        this.autoRefresh = autoRefresh;
        this.refreshBuffer = refreshBuffer;
    }

    private static byte[] encryptWithToken(String token, byte[] plaintext) throws LiNaStoreProtocolException {
        try {
            MessageDigest sha256 = MessageDigest.getInstance("SHA-256");
            byte[] keyBytes = sha256.digest(token.getBytes(StandardCharsets.UTF_8));
            SecretKeySpec key = new SecretKeySpec(keyBytes, "AES");

            byte[] nonce = new byte[12];
            new SecureRandom().nextBytes(nonce);
            GCMParameterSpec gcmSpec = new GCMParameterSpec(128, nonce);

            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
            cipher.init(Cipher.ENCRYPT_MODE, key, gcmSpec);
            byte[] ciphertextAndTag = cipher.doFinal(plaintext);

            byte[] out = new byte[nonce.length + ciphertextAndTag.length];
            System.arraycopy(nonce, 0, out, 0, nonce.length);
            System.arraycopy(ciphertextAndTag, 0, out, nonce.length, ciphertextAndTag.length);
            return out;
        } catch (NoSuchAlgorithmException e) {
            throw new LiNaStoreProtocolException("SHA-256 not available", e);
        } catch (Exception e) {
            throw new LiNaStoreProtocolException("Failed to encrypt payload", e);
        }
    }

    /**
     * Connects to the server.
     */
    private void connect() throws LiNaStoreConnectionException {
        if (socket != null && !socket.isClosed()) {
            return; // Already connected
        }
        
        try {
            socket = new Socket();
            socket.connect(address, timeout);
            socket.setSoTimeout(timeout);
        } catch (IOException e) {
            throw new LiNaStoreConnectionException("Failed to connect to server at " + address.toString(), e);
        }
    }

    /**
     * Disconnects from the server.
     */
    private void disconnect() {
        if (socket != null) {
            try {
                if (!socket.isClosed()) {
                    socket.close();
                }
            } catch (IOException e) {
                // Ignore errors during disconnect
            } finally {
                socket = null;
            }
        }
    }

    /**
     * Checks if the current token is expired or will expire soon.
     *
     * @return true if token is expired or will expire within refresh_buffer seconds
     */
    private boolean isTokenExpired() {
        if (tokenExpiresAt == 0) {
            return true;  // No token, treat as expired
        }
        
        long currentTime = System.currentTimeMillis() / 1000;
        
        // Check if token is expired or will expire within refresh_buffer seconds
        return currentTime >= (tokenExpiresAt - refreshBuffer);
    }

    /**
     * Refreshes the token if it's expired and auto-refresh is enabled.
     *
     * @throws LiNaStoreException if token refresh fails
     */
    private void refreshTokenIfNeeded() throws LiNaStoreException {
        if (!autoRefresh) {
            return;  // Auto-refresh disabled
        }

        // Auth-free mode: no token and no cached credentials means no refresh needed.
        if (sessionToken == null && (cachedUsername == null || cachedPassword == null)) {
            return;
        }
        
        if (isTokenExpired()) {
            if (cachedUsername != null && cachedPassword != null) {
                // Use cached credentials to refresh
                HandshakeResult res = handshake(cachedUsername, new String(cachedPassword), false);
                if (!res.getToken().isEmpty()) {
                    // Token refreshed successfully
                }
            } else {
                throw new LiNaStoreException("Token expired and no cached credentials available");
            }
        }
    }

    /**
     * Caches credentials for auto-refresh.
     *
     * @param username The username to cache
     * @param password The password to cache
     */
    public void cacheCredentials(String username, String password) {
        this.cachedUsername = username;
        this.cachedPassword = password.toCharArray();
    }

    /**
     * Clears cached credentials for security.
     */
    public void clearCachedCredentials() {
        if (cachedPassword != null) {
            java.util.Arrays.fill(cachedPassword, '\0');
        }
        cachedPassword = null;
        cachedUsername = null;
    }

    /**
     * Gets token information.
     *
     * @return TokenInfo containing token status
     */
    public TokenInfo getTokenInfo() {
        TokenInfo info = new TokenInfo();
        info.hasToken = sessionToken != null && !sessionToken.isEmpty();
        info.isExpired = isTokenExpired();
        info.expiresAt = tokenExpiresAt;
        
        if (tokenExpiresAt > 0) {
            long currentTime = System.currentTimeMillis() / 1000;
            info.expiresIn = (tokenExpiresAt > currentTime) ? (tokenExpiresAt - currentTime) : 0;
        } else {
            info.expiresIn = 0;
        }
        
        info.hasCachedCredentials = cachedUsername != null && cachedPassword != null;
        
        return info;
    }

    /**
     * Token information class.
     */
    public static class TokenInfo {
        public boolean hasToken;
        public boolean isExpired;
        public long expiresAt;
        public long expiresIn;
        public boolean hasCachedCredentials;
    }

    /**
     * Uploads a file to the server and reads response.
     *
     * @param fileName The name of the file to upload.
     * @param data     The content of the file.
     * @param flags    Optional flags to control upload behavior.
     * @throws IOException If upload fails due to network or protocol issues.
     */
    public boolean uploadFile(String fileName, byte[] data, int flags) throws LiNaStoreException {
        // Refresh token if needed before operation
        refreshTokenIfNeeded();
        
        if (fileName == null || fileName.isEmpty()) {
            throw new LiNaStoreProtocolException("File name cannot be null or empty");
        }
        
        if (data == null) {
            throw new LiNaStoreProtocolException("File data cannot be null");
        }
        
        try {
            connect();
            InputStream is;
            OutputStream os;
            try {
                is = socket.getInputStream();
                os = socket.getOutputStream();
            } catch (IOException e) {
                throw new LiNaStoreConnectionException("Failed to get streams for socket", e);
            }

            // Prepare flag byte
            byte flagByte = (byte) (flags & 0xFF);

            // Prepare identifier (variable length)
            byte[] fileNameBytes = fileName.getBytes(StandardCharsets.UTF_8);
            if (fileNameBytes.length > LINA_NAME_MAX_LENGTH) {
                throw new LiNaStoreProtocolException("File name is too long: " + fileNameBytes.length + " bytes");
            }
            byte ilen = (byte) fileNameBytes.length;
            byte[] identifier = fileNameBytes;

            // Prepare data with optional session token
            byte[] payloadData;
            if (sessionToken != null && !sessionToken.isEmpty()) {
                byte[] tokenBytes = sessionToken.getBytes(StandardCharsets.UTF_8);
                byte[] encrypted = encryptWithToken(sessionToken, data);

                payloadData = new byte[tokenBytes.length + 1 + encrypted.length];
                System.arraycopy(tokenBytes, 0, payloadData, 0, tokenBytes.length);
                payloadData[tokenBytes.length] = 0; // Null terminator
                System.arraycopy(encrypted, 0, payloadData, tokenBytes.length + 1, encrypted.length);
            } else {
                payloadData = data;
            }

            // dlen (data length) - 4 bytes little-endian
            int dlen = payloadData.length;
            byte[] dlenBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(dlen).array();

            // CRC32 checksum
            CRC32 crc32 = new CRC32();
            crc32.update(ilen);
            crc32.update(identifier);
            crc32.update(dlenBuffer);
            crc32.update(payloadData);
            byte[] checksumBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt((int) crc32.getValue()).array();

            // Send all parts
            try {
                os.write(flagByte);
                os.write(ilen);
                os.write(identifier);
                os.write(dlenBuffer);
                os.write(checksumBuffer);
                os.write(payloadData);
                os.flush();
            } catch (IOException e){
                throw new LiNaStoreConnectionException("Failed to send data for file: " + fileName, e);
            }

            // Read response
            try {
                int headerLen = LINA_HEADER_BASE_LENGTH + ilen;
                byte[] response = new byte[headerLen];
                int totalRead = 0;
                while (totalRead < headerLen) {
                    int bytesRead = is.read(response, totalRead, headerLen - totalRead);
                    if (bytesRead == -1) {
                        throw new LiNaStoreConnectionException("Connection closed while reading response for file: " + fileName);
                    }
                    totalRead += bytesRead;
                }
                
                if (response[0] != 0) {
                    throw new LiNaStoreProtocolException("Server returned error: " + response[0] + " for file: " + fileName);
                }
                return true;
            } catch (IOException e){
                throw new LiNaStoreConnectionException("Failed to read response for file: " + fileName, e);
            }
        } finally {
            disconnect();
        }
    }

    /**
     * Downloads a file from the server and verifies response.
     *
     * @param fileName Name of the file to download.
     * @return The downloaded data.
     * @throws IOException If download fails due to network or integrity issue.
     */
    public byte[] downloadFile(String fileName) throws LiNaStoreException {
        // Refresh token if needed before operation
        refreshTokenIfNeeded();
        
        if (fileName == null || fileName.isEmpty()) {
            throw new LiNaStoreProtocolException("File name cannot be null or empty");
        }
        
        try {
            connect();
            OutputStream os;
            InputStream is;
            try {
                os = socket.getOutputStream();
                is = socket.getInputStream();
            } catch (IOException e) {
                throw new LiNaStoreConnectionException("Failed to get streams for socket", e);
            }

            // Prepare flag byte
            byte flagByte = (byte) LiNaFlags.READ.getValue();

            // Prepare identifier (variable length)
            byte[] fileNameBytes = fileName.getBytes(StandardCharsets.UTF_8);
            if (fileNameBytes.length > LINA_NAME_MAX_LENGTH) {
                throw new LiNaStoreProtocolException("File name is too long: " + fileNameBytes.length + " bytes");
            }
            byte ilen = (byte) fileNameBytes.length;
            byte[] identifier = fileNameBytes;

            // Prepare data with optional session token
            byte[] payloadData;
            if (sessionToken != null && !sessionToken.isEmpty()) {
                payloadData = sessionToken.getBytes(StandardCharsets.UTF_8);
            } else {
                payloadData = new byte[0];
            }

            // dlen (data length) - 4 bytes little-endian
            int dlen = payloadData.length;
            byte[] dlenBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(dlen).array();

            // CRC32 checksum
            CRC32 crc32 = new CRC32();
            crc32.update(ilen);
            crc32.update(identifier);
            crc32.update(dlenBuffer);
            crc32.update(payloadData);
            byte[] checksumBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt((int) crc32.getValue()).array();

            // Send request
            try {
                os.write(flagByte);
                os.write(ilen);
                os.write(identifier);
                os.write(dlenBuffer);
                os.write(checksumBuffer);
                os.write(payloadData);
                os.flush();
            } catch (IOException e){
                throw new LiNaStoreConnectionException("Failed to send request for file: " + fileName, e);
            }

            // Read response header
            int headerLen = LINA_HEADER_BASE_LENGTH + ilen;
            byte[] header = new byte[headerLen];
            int totalRead = 0;
            while (totalRead < header.length) {
                int read;
                try {
                    read = is.read(header, totalRead, header.length - totalRead);
                } catch (IOException e) {
                    throw new LiNaStoreConnectionException("Failed to read header for file: " + fileName, e);
                }
                if (read == -1) {
                    throw new LiNaStoreConnectionException("Connection closed while reading header for file: " + fileName);
                }
                totalRead += read;
            }

            if (totalRead < header.length) {
                throw new LiNaStoreProtocolException("Incomplete response header received: " + totalRead + " < " + header.length);
            }

            // Check response flag first
            if (header[0] != 0) {
                throw new LiNaStoreProtocolException("Server returned error code: " + header[0] + " for file: " + fileName);
            }

            // Parse header: flags(1) + ilen(1) + identifier(ilen) + dlen(4) + checksum(4)
            int p = 1; // Skip flag
            byte ilenRecv = header[p];
            p += 1;
            byte[] identifierRecv = new byte[ilenRecv];
            System.arraycopy(header, p, identifierRecv, 0, ilenRecv);
            p += ilenRecv;
            int dataLength = ByteBuffer.wrap(header, p, 4).order(ByteOrder.LITTLE_ENDIAN).getInt();
            p += 4;
            byte[] checksumRecv = new byte[4];
            System.arraycopy(header, p, checksumRecv, 0, 4);

            if (dataLength < 0) {
                throw new LiNaStoreProtocolException("Invalid data length received: " + dataLength);
            }
            
            byte[] data = new byte[dataLength];
            totalRead = 0;
            while (totalRead < dataLength) {
                int read;
                try {
                    read = is.read(data, totalRead, dataLength - totalRead);
                } catch (IOException e) {
                    throw new LiNaStoreConnectionException("Failed to read data for file: " + fileName, e);
                }
                if (read == -1) {
                    throw new LiNaStoreConnectionException("Connection closed while reading data for file: " + fileName);
                }
                totalRead += read;
            }

            if (totalRead < dataLength) {
                throw new LiNaStoreProtocolException("Incomplete data received: " + totalRead + " < " + dataLength);
            }

            // Verify checksum
            crc32.reset();
            crc32.update(ilenRecv);
            crc32.update(identifierRecv);
            crc32.update(data);
            long expectedChecksum = crc32.getValue();
            long receivedChecksum = ByteBuffer.wrap(checksumRecv).order(ByteOrder.LITTLE_ENDIAN).getInt() & 0xFFFFFFFFL;

            if (expectedChecksum != receivedChecksum) {
                throw new LiNaStoreChecksumException("Checksum verification failed for file: " + fileName +
                    " (expected: " + expectedChecksum + ", received: " + receivedChecksum + ")");
            }

            return data;
        } finally {
            disconnect();
        }
    }

    /**
     * Deletes a file on the server.
     *
     * @param fileName Name of the file to delete.
     * @return True if successful.
     */
    public boolean deleteFile(String fileName) throws LiNaStoreException {
        // Refresh token if needed before operation
        refreshTokenIfNeeded();
        
        if (fileName == null || fileName.isEmpty()) {
            throw new LiNaStoreProtocolException("File name cannot be null or empty");
        }
        
        try {
            connect();
            OutputStream os;
            InputStream is;
            try {
                os = socket.getOutputStream();
                is = socket.getInputStream();
            } catch (IOException e) {
                throw new LiNaStoreConnectionException("Failed to get streams for socket", e);
            }

            // Prepare flag byte
            byte flagByte = (byte) LiNaFlags.DELETE.getValue();

            // Prepare identifier (variable length)
            byte[] fileNameBytes = fileName.getBytes(StandardCharsets.UTF_8);
            if (fileNameBytes.length > LINA_NAME_MAX_LENGTH) {
                throw new LiNaStoreProtocolException("File name is too long: " + fileNameBytes.length + " bytes");
            }
            byte ilen = (byte) fileNameBytes.length;
            byte[] identifier = fileNameBytes;

            // Prepare data with optional session token
            byte[] payloadData;
            if (sessionToken != null && !sessionToken.isEmpty()) {
                payloadData = sessionToken.getBytes(StandardCharsets.UTF_8);
            } else {
                payloadData = new byte[0];
            }

            // dlen (data length) - 4 bytes little-endian
            int dlen = payloadData.length;
            byte[] dlenBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(dlen).array();
            
            // CRC32 checksum
            CRC32 crc32 = new CRC32();
            crc32.update(ilen);
            crc32.update(identifier);
            crc32.update(dlenBuffer);
            crc32.update(payloadData);
            byte[] checksumBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt((int) crc32.getValue()).array();

            // Send request
            try {
                os.write(flagByte);
                os.write(ilen);
                os.write(identifier);
                os.write(dlenBuffer);
                os.write(checksumBuffer);
                os.write(payloadData);
                os.flush();
            } catch (IOException e){
                throw new LiNaStoreConnectionException("Failed to send delete request for file: " + fileName, e);
            }

            // Read response
            int headerLen = LINA_HEADER_BASE_LENGTH + ilen;
            byte[] response = new byte[headerLen];
            int totalRead = 0;
            while (totalRead < headerLen) {
                int bytesRead;
                try {
                    bytesRead = is.read(response, totalRead, headerLen - totalRead);
                } catch (IOException e) {
                    throw new LiNaStoreConnectionException("Failed to read delete response for file: " + fileName, e);
                }
                if (bytesRead == -1) {
                    throw new LiNaStoreConnectionException("Connection closed while reading delete response for file: " + fileName);
                }
                totalRead += bytesRead;
            }
            
            if (totalRead < headerLen) {
                throw new LiNaStoreProtocolException("Incomplete delete response received: " + totalRead + " < " + headerLen);
            }
            
            if (response[0] != 0) {
                throw new LiNaStoreProtocolException("Server returned error code: " + response[0] + " for file: " + fileName);
            }

            return true;
        } finally {
            disconnect();
        }
    }

    /**
     * Authenticates with the server using username and password and stores the session token.
     *
     * @param username The username for authentication (max 255 bytes).
     * @param password The password for authentication.
     * @param cacheCredentials Whether to cache credentials for auto-refresh.
     * @return A HandshakeResult containing the session token and expiration timestamp.
     * @throws LiNaStoreException If authentication fails due to network or protocol issues.
     */
    public HandshakeResult handshake(String username, String password, boolean cacheCredentials) throws LiNaStoreException {
        if (username == null || username.isEmpty()) {
            throw new LiNaStoreProtocolException("Username cannot be null or empty");
        }
        if (password == null || password.isEmpty()) {
            throw new LiNaStoreProtocolException("Password cannot be null or empty");
        }
        
        // Cache credentials if requested
        if (cacheCredentials) {
            cacheCredentials(username, password);
        }
        
        try {
            connect();
            InputStream is;
            OutputStream os;
            try {
                is = socket.getInputStream();
                os = socket.getOutputStream();
            } catch (IOException e) {
                throw new LiNaStoreConnectionException("Failed to get streams for socket", e);
            }

            // Prepare flag byte for authentication
            byte flagByte = (byte) LiNaFlags.AUTH.getValue();

            // Prepare identifier (variable length) - username
            byte[] usernameBytes = username.getBytes(StandardCharsets.UTF_8);
            if (usernameBytes.length > LINA_NAME_MAX_LENGTH) {
                throw new LiNaStoreProtocolException("Username is too long: " + usernameBytes.length + " bytes");
            }
            byte ilen = (byte) usernameBytes.length;
            byte[] identifier = usernameBytes;

            // Prepare data - password (null-terminated)
            byte[] passwordBytes = password.getBytes(StandardCharsets.UTF_8);
            byte[] passwordData = new byte[passwordBytes.length + 1];
            System.arraycopy(passwordBytes, 0, passwordData, 0, passwordBytes.length);
            passwordData[passwordBytes.length] = 0; // Null terminator

            // dlen (data length) - 4 bytes little-endian
            int dlen = passwordData.length;
            byte[] dlenBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(dlen).array();
            
            // CRC32 checksum
            CRC32 crc32 = new CRC32();
            crc32.update(ilen);
            crc32.update(identifier);
            crc32.update(dlenBuffer);
            crc32.update(passwordData);
            byte[] checksumBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt((int) crc32.getValue()).array();

            // Send authentication request
            try {
                os.write(flagByte);
                os.write(ilen);
                os.write(identifier);
                os.write(dlenBuffer);
                os.write(checksumBuffer);
                os.write(passwordData);
                os.flush();
            } catch (IOException e){
                throw new LiNaStoreConnectionException("Failed to send authentication request", e);
            }

            // Read response header (no identifier in response)
            int headerLen = LINA_HEADER_BASE_LENGTH;
            byte[] header = new byte[headerLen];
            int totalRead = 0;
            while (totalRead < header.length) {
                int read;
                try {
                    read = is.read(header, totalRead, header.length - totalRead);
                } catch (IOException e) {
                    throw new LiNaStoreConnectionException("Failed to read authentication response", e);
                }
                if (read == -1) {
                    throw new LiNaStoreConnectionException("Connection closed while reading authentication response");
                }
                totalRead += read;
            }

            if (totalRead < header.length) {
                throw new LiNaStoreProtocolException("Incomplete authentication response received: " + totalRead + " < " + header.length);
            }

            // Parse header: status(1) + ilen(1) + dlen(4) + checksum(4)
            int p = 0;
            byte status = header[p];
            p += 1;
            byte ilenRecv = header[p];
            p += 1;
            int dataLength = ByteBuffer.wrap(header, p, 4).order(ByteOrder.LITTLE_ENDIAN).getInt();
            p += 4;
            // Skip checksum (not needed for validation here)

            // Check for error status
            if (status != 0) {
                // Read error status from data field
                if (dataLength > 0) {
                    byte[] errorData = new byte[dataLength];
                    totalRead = 0;
                    while (totalRead < dataLength) {
                        int read;
                        try {
                            read = is.read(errorData, totalRead, dataLength - totalRead);
                        } catch (IOException e) {
                            throw new LiNaStoreConnectionException("Failed to read error data", e);
                        }
                        if (read == -1) {
                            throw new LiNaStoreConnectionException("Connection closed while reading error data");
                        }
                        totalRead += read;
                    }
                    
                    if (errorData.length > 0) {
                        int errorCode = errorData[0] & 0xFF;
                        String errorMsg;
                        switch (errorCode) {
                            case 1:
                                errorMsg = "Invalid password";
                                break;
                            case 2:
                                errorMsg = "Authentication disabled";
                                break;
                            case 127:
                                errorMsg = "Internal server error";
                                break;
                            default:
                                errorMsg = "Authentication failed with error code: " + errorCode;
                        }
                        throw new LiNaStoreProtocolException(errorMsg);
                    }
                }
                throw new LiNaStoreProtocolException("Authentication failed with status: " + status);
            }

            // Read response data: status(1) + token + '\0' + expires_at
            if (dataLength > 0) {
                byte[] responseData = new byte[dataLength];
                totalRead = 0;
                while (totalRead < dataLength) {
                    int read;
                    try {
                        read = is.read(responseData, totalRead, dataLength - totalRead);
                    } catch (IOException e) {
                        throw new LiNaStoreConnectionException("Failed to read response data", e);
                    }
                    if (read == -1) {
                        throw new LiNaStoreConnectionException("Connection closed while reading response data");
                    }
                    totalRead += read;
                }

                // Parse response: handshakeStatus(1) + token + '\0' + expires_at
                byte handshakeStatus = responseData[0];
                
                if (handshakeStatus == 0) { // Success
                    // Find null terminator after token
                    int nullPos = -1;
                    for (int i = 1; i < responseData.length; i++) {
                        if (responseData[i] == 0) {
                            nullPos = i;
                            break;
                        }
                    }
                    
                    if (nullPos == -1) {
                        throw new LiNaStoreProtocolException("Invalid auth response: missing null terminator");
                    }
                    
                    String token = new String(responseData, 1, nullPos - 1, StandardCharsets.UTF_8);
                    String expiresAtStr = new String(responseData, nullPos + 1, responseData.length - nullPos - 1, StandardCharsets.UTF_8);
                    long expiresAt = Long.parseLong(expiresAtStr);
                    
                    // Store session token and expiration
                    this.sessionToken = token;
                    this.tokenExpiresAt = expiresAt;
                    
                    return new HandshakeResult(token, expiresAt);
                } else {
                    String errorMsg;
                    switch (handshakeStatus) {
                        case 1:
                            errorMsg = "Invalid password";
                            break;
                        case 2:
                            errorMsg = "Authentication disabled";
                            break;
                        case 127:
                            errorMsg = "Internal server error";
                            break;
                        default:
                            errorMsg = "Handshake failed with status: " + handshakeStatus;
                    }
                    throw new LiNaStoreProtocolException(errorMsg);
                }
            } else {
                throw new LiNaStoreProtocolException("Empty auth response received");
            }
        } finally {
            // Don't disconnect after handshake - keep connection for subsequent operations
            // disconnect();
        }
    }

    /**
     * Authenticates with the server using username and password and stores the session token.
     * This is a convenience method that caches credentials by default.
     *
     * @param username The username for authentication (max 255 bytes).
     * @param password The password for authentication.
     * @return A HandshakeResult containing the session token and expiration timestamp.
     * @throws LiNaStoreException If authentication fails due to network or protocol issues.
     */
    public HandshakeResult handshake(String username, String password) throws LiNaStoreException {
        return handshake(username, password, true);
    }

    /**
     * Result class for handshake authentication.
     */
    public static class HandshakeResult {
        private final String token;
        private final long expiresAt;
        
        public HandshakeResult(String token, long expiresAt) {
            this.token = token;
            this.expiresAt = expiresAt;
        }
        
        public String getToken() {
            return token;
        }
        
        public long getExpiresAt() {
            return expiresAt;
        }
    }

    /**
     * Authenticates with the server using a password and stores the session token.
     * This is a convenience method that uses a default username.
     *
     * @param password The password for authentication.
     * @return True if authentication successful.
     * @throws LiNaStoreException If authentication fails due to network or protocol issues.
     * @deprecated Use {@link #handshake(String, String)} instead for proper username-based authentication.
     */
    @Deprecated
    public boolean authenticate(String password) throws LiNaStoreException {
        handshake("admin", password, true);
        return true;
    }

    /**
     * Gets the current session token.
     *
     * @return The session token if authenticated, null otherwise.
     */
    public String getSessionToken() {
        return sessionToken;
    }

    /**
     * Checks if the client is authenticated.
     *
     * @return True if authenticated, false otherwise.
     */
    public boolean isAuthenticated() {
        return sessionToken != null && !sessionToken.isEmpty();
    }

    /**
     * Logs out by invalidating the session token.
     */
    public void logout() {
        sessionToken = null;
    }

    /**
     * Main method for testing.
     */
    public static void main(String[] args) {
        String host = "localhost";
        int port = 8096; // Advanced service port
        String testFile = "test.txt";
        String username = "admin"; // Test username
        String password = "test123"; // Test password

        try {
            LiNaStoreClient client = new LiNaStoreClient(host, port);

            // Test handshake authentication
            System.out.println("Testing handshake authentication...");
            HandshakeResult result = client.handshake(username, password, true);
            System.out.println("Authentication successful!");
            System.out.println("Session token: " + result.getToken());
            System.out.println("Expires at: " + result.getExpiresAt());
            
            // Test upload with authentication
            byte[] testData = "This is a test file with authentication.".getBytes(StandardCharsets.UTF_8);
            boolean uploadSuccess = client.uploadFile(testFile, testData, LiNaFlags.WRITE.getValue());
            if (uploadSuccess) {
                System.out.println("Upload with authentication successful.");
                
                // Test download with authentication
                byte[] downloadedData = client.downloadFile(testFile);
                System.out.println("Downloaded data: " + new String(downloadedData));
                
                // Test delete with authentication
                boolean deleted = client.deleteFile(testFile);
                System.out.println("Delete success: " + deleted);
            } else {
                System.out.println("Upload failed.");
            }
            
            // Test logout
            client.logout();
            System.out.println("Logged out successfully.");

        } catch (LiNaStoreException e) {
            System.err.println("LiNaStore error: " + e.getMessage());
            e.printStackTrace();
        }
    }
}
