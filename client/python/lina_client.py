import socket
import io
import binascii

class LiNaStoreClientError(Exception):
    """Base exception for LiNaStore client errors"""
    pass

class LiNaStoreConnectionError(LiNaStoreClientError):
    """Exception raised for connection errors"""
    pass

class LiNaStoreProtocolError(LiNaStoreClientError):
    """Exception raised for protocol errors"""
    pass

class LiNaStoreChecksumError(LiNaStoreProtocolError):
    """Exception raised for checksum verification failures"""
    pass

class LiNaStoreClient:
    DELETE = 0xC0
    WRITE = 0x80
    READ = 0x40
    COVER = 0x02
    COMPRESS = 0x01
    NONE = 0x00

    LINA_NAME_MAX_LENGTH = 255
    LINA_HEADER_LENGTH = 0x108

    def __init__(self, ip_address: str, port: int, timeout: int = 5):
        self.ip_address = ip_address
        self.port = port
        self.timeout = timeout
        self.socket = None
        
    def connect(self):
        # Logic to connect to the LiNaStore service
        try:
            self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.socket.settimeout(self.timeout)
            self.socket.connect((self.ip_address, self.port))
        except socket.error as e:
            raise LiNaStoreConnectionError(f"Failed to connect to {self.ip_address}:{self.port}: {str(e)}")

    def disconnect(self):
        # Logic to disconnect from the LiNaStore service
        if self.socket:
            try:
                self.socket.close()
            except socket.error:
                pass  # Ignore errors during disconnect
            finally:
                self.socket = None

    def upload_file(self, file_name: str, reader: io.BufferedReader) -> bool:
        # Logic to upload a file to LiNaStore
        if not self.socket:
            self.connect()
        
        try:
            file_data = reader.read()
            flags = 0x80.to_bytes(1, 'little')
            name_bin = file_name.encode()
            if len(name_bin) > self.LINA_NAME_MAX_LENGTH:
                raise LiNaStoreProtocolError(f"File name too long: {len(name_bin)} > {self.LINA_NAME_MAX_LENGTH}")
            else:
                for i in range(self.LINA_NAME_MAX_LENGTH - len(name_bin)):
                    name_bin += b'\x00'
            length = len(file_data).to_bytes(4, 'little')
            checksum = binascii.crc32(name_bin + length + file_data).to_bytes(4, 'little')
            
            try:
                self.socket.sendall(flags + name_bin + length + checksum + file_data)
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to send data for file {file_name}: {str(e)}")

            try:
                resp = self.socket.recv(self.LINA_HEADER_LENGTH)
                if len(resp) < self.LINA_HEADER_LENGTH:
                    raise LiNaStoreProtocolError(f"Incomplete response received: {len(resp)} < {self.LINA_HEADER_LENGTH}")
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to receive response for file {file_name}: {str(e)}")

            if resp[0] != 0:
                raise LiNaStoreProtocolError(f"Server returned error code: {resp[0]} for file: {file_name}")

            return True
        finally:
            self.disconnect()

    def download_file(self, file_name: str) -> bytes:
        # Logic to download a file from LiNaStore
        if not self.socket:
            self.connect()
        
        try:
            flags = 0x40.to_bytes(1, 'little')
            name_bin = file_name.encode()
            if len(name_bin) > self.LINA_NAME_MAX_LENGTH:
                raise LiNaStoreProtocolError(f"File name too long: {len(name_bin)} > {self.LINA_NAME_MAX_LENGTH}")
            else:
                for i in range(self.LINA_NAME_MAX_LENGTH - len(name_bin)):
                    name_bin += b'\x00'
            length = int(0).to_bytes(4, 'little')
            checksum = binascii.crc32(name_bin + length).to_bytes(4, 'little')
                                                               
            try:
                self.socket.sendall(flags + name_bin + length + checksum)
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to send request for file {file_name}: {str(e)}")

            try:
                header = self.socket.recv(self.LINA_HEADER_LENGTH)
                if len(header) < self.LINA_HEADER_LENGTH:
                    raise LiNaStoreProtocolError(f"Incomplete header received: {len(header)} < {self.LINA_HEADER_LENGTH}")
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to receive header for file {file_name}: {str(e)}")
            
            data_pointer = 0

            flags = int(header[0])
            data_pointer += 1

            # name is no needed, just skip it
            data_pointer += self.LINA_NAME_MAX_LENGTH

            length = int.from_bytes(header[data_pointer: data_pointer + 4], 'little')
            data_pointer += 4

            checksum = int.from_bytes(header[data_pointer: data_pointer + 4], 'little')
            data_pointer += 4

            if flags != 0:
                raise LiNaStoreProtocolError(f"Server returned error code: {flags} for file: {file_name}")

            try:
                data = self._recv_all(length)
                if len(data) < length:
                    raise LiNaStoreProtocolError(f"Incomplete data received: {len(data)} < {length}")
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to receive data for file {file_name}: {str(e)}")

            if not self.verify_checksum(name_bin, length, data, checksum):
                raise LiNaStoreChecksumError(f"Checksum verification failed for file: {file_name}")
            
            return data
        finally:
            self.disconnect()
    
    def delete_file(self, file_name: str) -> bool:
        # Logic to delete a file from LiNaStore
        if not self.socket:
            self.connect()
        
        try:
            flags = 0xC0.to_bytes(1, 'little')
            name_bin = file_name.encode()
            if len(name_bin) > self.LINA_NAME_MAX_LENGTH:
                raise LiNaStoreProtocolError(f"File name too long: {len(name_bin)} > {self.LINA_NAME_MAX_LENGTH}")
            else:
                for i in range(self.LINA_NAME_MAX_LENGTH - len(name_bin)):
                    name_bin += b'\x00'
            length = int(0).to_bytes(4, 'little')
            checksum = binascii.crc32(name_bin + length).to_bytes(4, 'little')

            try:
                self.socket.sendall(flags + name_bin + length + checksum)
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to send delete request for file {file_name}: {str(e)}")

            try:
                resp = self.socket.recv(self.LINA_HEADER_LENGTH)
                if len(resp) < self.LINA_HEADER_LENGTH:
                    raise LiNaStoreProtocolError(f"Incomplete response received: {len(resp)} < {self.LINA_HEADER_LENGTH}")
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to receive delete response for file {file_name}: {str(e)}")

            if resp[0] != 0:
                raise LiNaStoreProtocolError(f"Server returned error code: {resp[0]} for file: {file_name}")

            return True
        finally:
            self.disconnect()


    def _recv_all(self, size: int) -> bytes:
        """Helper method to receive all data of specified size"""
        data = b''
        while len(data) < size:
            chunk = self.socket.recv(size - len(data))
            if not chunk:
                raise LiNaStoreConnectionError("Connection closed while receiving data")
            data += chunk
        return data

    def verify_checksum(self, name_bin: bytes, length: int, data: bytes, checksum: int):
        calculated_checksum = binascii.crc32(name_bin + length.to_bytes(4, 'little') + data)
        return calculated_checksum == checksum