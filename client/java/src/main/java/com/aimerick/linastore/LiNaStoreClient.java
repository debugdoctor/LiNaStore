package com.aimerick.linastore;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.zip.CRC32;

import com.aimerick.linastore.LiNaFlags;

public class LiNaStoreClient {
    private Socket socket;
    private final InetSocketAddress address;
    private final int timeout;
    private final static int LINA_NAME_MAX_LENGTH = 255;
    private final static int LINA_HEADER_LENGTH = 0x108;
    
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

    public LiNaStoreClient(String ip, int port) {
        this(ip, port, 5000);
    }

    public LiNaStoreClient(String ip, int port, int timeout) {
        this.address = new InetSocketAddress(ip, port);
        this.timeout = timeout;
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
     * Uploads a file to the server and reads response.
     *
     * @param fileName The name of the file to upload.
     * @param data     The content of the file.
     * @param flags    Optional flags to control upload behavior.
     * @throws IOException If upload fails due to network or protocol issues.
     */
    public boolean uploadFile(String fileName, byte[] data, int flags) throws LiNaStoreException {
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

            // Prepare file name buffer (255 bytes)
            byte[] fullFileNameBuffer = new byte[LINA_NAME_MAX_LENGTH];
            byte[] fileNameBytes = fileName.getBytes(StandardCharsets.UTF_8);
            if (fileNameBytes.length > LINA_NAME_MAX_LENGTH) {
                throw new LiNaStoreProtocolException("File name is too long: " + fileNameBytes.length + " bytes");
            }
            System.arraycopy(fileNameBytes, 0, fullFileNameBuffer, 0, fileNameBytes.length);

            // Length of data
            byte[] lengthBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(data.length).array();

            // CRC32 checksum
            CRC32 crc32 = new CRC32();
            crc32.update(fullFileNameBuffer);
            crc32.update(lengthBuffer);
            crc32.update(data);
            byte[] checksumBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt((int) crc32.getValue()).array();

            // Send all parts
            try {
                os.write(flagByte);
                os.write(fullFileNameBuffer);
                os.write(lengthBuffer);
                os.write(checksumBuffer);
                os.write(data);
                os.flush();
            } catch (IOException e){
                throw new LiNaStoreConnectionException("Failed to send data for file: " + fileName, e);
            }

            // Read response
            try {
                byte[] response = new byte[LINA_HEADER_LENGTH];
                int totalRead = 0;
                while (totalRead < LINA_HEADER_LENGTH) {
                    int bytesRead = is.read(response, totalRead, LINA_HEADER_LENGTH - totalRead);
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

            // Prepare file name buffer (255 bytes)
            byte[] fullFileNameBuffer = new byte[LINA_NAME_MAX_LENGTH];
            byte[] fileNameBytes = fileName.getBytes(StandardCharsets.UTF_8);
            if (fileNameBytes.length > LINA_NAME_MAX_LENGTH) {
                throw new LiNaStoreProtocolException("File name is too long: " + fileNameBytes.length + " bytes");
            }
            System.arraycopy(fileNameBytes, 0, fullFileNameBuffer, 0, fileNameBytes.length);

            // Length buffer (zero)
            byte[] lengthBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(0).array();

            // CRC32 checksum
            CRC32 crc32 = new CRC32();
            crc32.update(fullFileNameBuffer);
            crc32.update(lengthBuffer);
            byte[] checksumBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt((int) crc32.getValue()).array();

            // Send request
            try {
                os.write(flagByte);
                os.write(fullFileNameBuffer);
                os.write(lengthBuffer);
                os.write(checksumBuffer);
                os.flush();
            } catch (IOException e){
                throw new LiNaStoreConnectionException("Failed to send request for file: " + fileName, e);
            }

            // Read response header
            byte[] header = new byte[LINA_HEADER_LENGTH]; // 1(flag) + 255(name) + 4(length) + 4(checksum)
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

            int dataLength = ByteBuffer.wrap(header, 256, 4).order(ByteOrder.LITTLE_ENDIAN).getInt();
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
            crc32.update(fullFileNameBuffer);
            crc32.update(lengthBuffer);
            crc32.update(data);
            long expectedChecksum = crc32.getValue();
            long receivedChecksum = ByteBuffer.wrap(header, 260, 4).order(ByteOrder.LITTLE_ENDIAN).getInt() & 0xFFFFFFFFL;

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

            // Prepare file name buffer (255 bytes)
            byte[] fullFileNameBuffer = new byte[LINA_NAME_MAX_LENGTH];
            byte[] fileNameBytes = fileName.getBytes(StandardCharsets.UTF_8);
            if (fileNameBytes.length > LINA_NAME_MAX_LENGTH) {
                throw new LiNaStoreProtocolException("File name is too long: " + fileNameBytes.length + " bytes");
            }
            System.arraycopy(fileNameBytes, 0, fullFileNameBuffer, 0, fileNameBytes.length);

            // Length buffer (zero)
            byte[] lengthBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(0).array();

            // CRC32 checksum
            CRC32 crc32 = new CRC32();
            crc32.update(fullFileNameBuffer);
            crc32.update(lengthBuffer);
            byte[] checksumBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt((int) crc32.getValue()).array();

            // Send request
            try {
                os.write(flagByte);
                os.write(fullFileNameBuffer);
                os.write(lengthBuffer);
                os.write(checksumBuffer);
                os.flush();
            } catch (IOException e){
                throw new LiNaStoreConnectionException("Failed to send delete request for file: " + fileName, e);
            }

            // Read response
            byte[] response = new byte[LINA_HEADER_LENGTH];
            int totalRead = 0;
            while (totalRead < LINA_HEADER_LENGTH) {
                int bytesRead;
                try {
                    bytesRead = is.read(response, totalRead, LINA_HEADER_LENGTH - totalRead);
                } catch (IOException e) {
                    throw new LiNaStoreConnectionException("Failed to read delete response for file: " + fileName, e);
                }
                if (bytesRead == -1) {
                    throw new LiNaStoreConnectionException("Connection closed while reading delete response for file: " + fileName);
                }
                totalRead += bytesRead;
            }
            
            if (totalRead < LINA_HEADER_LENGTH) {
                throw new LiNaStoreProtocolException("Incomplete delete response received: " + totalRead + " < " + LINA_HEADER_LENGTH);
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
     * Main method for testing.
     */
    public static void main(String[] args) {
        String host = "localhost";
        int port = 8080;
        String testFile = "test.txt";

        try {
            LiNaStoreClient client = new LiNaStoreClient(host, port);

            // Test upload
            byte[] testData = "This is a test file.".getBytes(StandardCharsets.UTF_8);
            client.uploadFile(testFile, testData, LiNaFlags.WRITE.getValue());
            System.out.println("Upload complete.");

            // Test download
            byte[] downloadedData = client.downloadFile(testFile);
            System.out.println("Downloaded data: " + new String(downloadedData));

            // Test delete
            boolean deleted = client.deleteFile(testFile);
            System.out.println("Delete success: " + deleted);

        } catch (LiNaStoreException e) {
            System.err.println("LiNaStore error: " + e.getMessage());
            e.printStackTrace();
        }
    }
}