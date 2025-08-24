package cc.aimerick.server.utils;

import cc.aimerick.server.enumeration.LiNaFlags;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.zip.CRC32;

public class LiNaStoreClient {
    private Socket socket;
    private final InetSocketAddress address;
    private final int timeout;
    private final static int LINA_NAME_MAX_LENGTH = 255;
    private final static int LINA_HEADER_LENGTH = 0x108;

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
    private void connect() throws IOException {
        socket = new Socket();
        try{
            socket.connect(address, timeout);
        } catch (IOException e) {
            throw new IOException("Failed to connect to server: " , e);
        }
    }

    /**
     * Disconnects from the server.
     */
    private void disconnect() throws IOException {
        if (socket != null && !socket.isClosed()) {
            socket.close();
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
    public boolean uploadFile(String fileName, byte[] data, int flags) throws IOException {
        try {
            connect();
            InputStream is = socket.getInputStream();
            OutputStream os = socket.getOutputStream();

            // Prepare flag byte
            byte flagByte = (byte) (flags & 0xFF);

            // Prepare file name buffer (255 bytes)
            byte[] fullFileNameBuffer = new byte[LINA_NAME_MAX_LENGTH];
            byte[] fileNameBytes = fileName.getBytes(StandardCharsets.UTF_8);
            if (fileNameBytes.length > LINA_NAME_MAX_LENGTH) {
                throw new IllegalArgumentException("File name is too long: " + fileNameBytes.length + " bytes");
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
            } catch (IOException e){
                throw new IOException("Failed to send data for file: " + fileName, e);
            }

            // Read response
            try {
                byte[] response = new byte[LINA_HEADER_LENGTH];
                int bytesRead = is.read(response);
                if (bytesRead > 0 && response[0] != 0) {
                     throw new IOException("Server returned error: " + response[0] + " for file: " + fileName);
                }
                return response[0] == 0;
            } catch (IOException e){
                throw new IOException("Failed to read response for file: " + fileName, e);
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
    public byte[] downloadFile(String fileName) throws IOException {
        try {
            connect();
            OutputStream os = socket.getOutputStream();
            InputStream is = socket.getInputStream();

            // Prepare flag byte
            byte flagByte = (byte) LiNaFlags.READ.getValue();

            // Prepare file name buffer (255 bytes)
            byte[] fullFileNameBuffer = new byte[LINA_NAME_MAX_LENGTH];
            byte[] fileNameBytes = fileName.getBytes(StandardCharsets.UTF_8);
            if (fileNameBytes.length > LINA_NAME_MAX_LENGTH) {
                throw new IllegalArgumentException("File name is too long: " + fileNameBytes.length + " bytes");
            }
            System.arraycopy(fileNameBytes, 0, fullFileNameBuffer, 0, fileNameBytes.length);

            // Length buffer (zero)
            byte[] lengthBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(0).array();

            // CRC32 checksum
            CRC32 crc32 = new CRC32();
            crc32.update(fullFileNameBuffer);
            crc32.update(lengthBuffer);
            byte[] checksumBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putLong(crc32.getValue()).array();

            // Send request
            try {
                os.write(flagByte);
                os.write(fullFileNameBuffer);
                os.write(lengthBuffer);
                os.write(checksumBuffer);
            } catch (IOException e){
                throw new IOException("Failed to send request for file: " + fileName, e);
            }

            // Read response
            byte[] header = new byte[LINA_HEADER_LENGTH]; // 1(flag) + 255(name) + 4(length) + 4(checksum)
            int totalRead = 0;
            while (totalRead < header.length) {
                int read = is.read(header, totalRead, header.length - totalRead);
                if (read == -1) break;
                totalRead += read;
            }

            if (totalRead < header.length) {
                throw new IOException("Incomplete response header received.");
            }

            int dataLength = ByteBuffer.wrap(header, 256, 4).order(ByteOrder.LITTLE_ENDIAN).getInt();
            byte[] data = new byte[dataLength];
            totalRead = 0;
            while (totalRead < dataLength) {
                int read = is.read(data, totalRead, dataLength - totalRead);
                if (read == -1) break;
                totalRead += read;
            }

            // Verify checksum
            crc32.reset();
            crc32.update(fullFileNameBuffer);
            crc32.update(lengthBuffer);
            crc32.update(data);
            long expectedChecksum = crc32.getValue();
            long receivedChecksum = ByteBuffer.wrap(header, 260, 4).order(ByteOrder.LITTLE_ENDIAN).getLong();

            if (expectedChecksum != receivedChecksum) {
                throw new IOException("Checksum verification failed");
            }

            // Check response flag
            if (header[0] != 0) {
                throw new IOException("Server returned error code: " + header[0]);
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
    public boolean deleteFile(String fileName) throws IOException {
        try {
            connect();
            OutputStream os = socket.getOutputStream();
            InputStream is = socket.getInputStream();

            // Prepare flag byte
            byte flagByte = (byte) LiNaFlags.DELETE.getValue();

            // Prepare file name buffer (255 bytes)
            byte[] fullFileNameBuffer = new byte[LINA_NAME_MAX_LENGTH];
            byte[] fileNameBytes = fileName.getBytes(StandardCharsets.UTF_8);
            if (fileNameBytes.length > LINA_NAME_MAX_LENGTH) {
                throw new IllegalArgumentException("File name is too long: " + fileNameBytes.length + " bytes");
            }
            System.arraycopy(fileNameBytes, 0, fullFileNameBuffer, 0, fileNameBytes.length);

            // Length buffer (zero)
            byte[] lengthBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putInt(0).array();

            // CRC32 checksum
            CRC32 crc32 = new CRC32();
            crc32.update(fullFileNameBuffer);
            crc32.update(lengthBuffer);
            byte[] checksumBuffer = ByteBuffer.allocate(4).order(ByteOrder.LITTLE_ENDIAN).putLong(crc32.getValue()).array();

            // Send request
            try {
                os.write(flagByte);
                os.write(fullFileNameBuffer);
                os.write(lengthBuffer);
                os.write(checksumBuffer);
            } catch (IOException e){
                throw new IOException("Failed to send request for file: " + fileName, e);
            }

            // Read response
            byte[] response = new byte[LINA_HEADER_LENGTH];
            int bytesRead = is.read(response);
            if (bytesRead > 0) {
                return response[0] == 0;
            }

            return false;
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

        } catch (IOException e) {
            e.printStackTrace();
        }
    }
}