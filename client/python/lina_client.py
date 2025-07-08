import socket
import io
import binascii

class LiNaStoreClient:
    DELETE = 0xC0
    WRITE = 0x80
    READ = 0x40
    COVER = 0x02
    COMPRESS = 0x01
    NONE = 0x00

    LINA_NAME_MAX_LENGTH = 255
    LINA_HEADER_LENGTH = 0x108

    def __init__(self, ip_address: str, port: int):
        self.ip_address = ip_address
        self.port = port
        self.socket = None
        
    def connect(self):
        # Logic to connect to the LiNaStore service
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.socket.connect((self.ip_address, self.port))

    def disconnect(self):
        # Logic to disconnect from the LiNaStore service
        self.socket.close()

    def upload_file(self, file_name: str, reader: io.BufferedReader) -> bool:
        # Logic to upload a file to LiNaStore
        self.connect()
        file_data = reader.read()
        flags = 0x80.to_bytes(1, 'little')
        name_bin = file_name.encode()
        if len(name_bin) > self.LINA_NAME_MAX_LENGTH:
            raise ValueError("File name too long")
        else:
            for i in range(self.LINA_NAME_MAX_LENGTH - len(name_bin)):
                name_bin += b'\x00'
        length = len(file_data).to_bytes(4, 'little')
        checksum = binascii.crc32(name_bin + length + file_data).to_bytes(4, 'little')
        
        self.socket.sendall(flags + name_bin + length + checksum + file_data)

        resp = self.socket.recv(self.LINA_HEADER_LENGTH)

        self.disconnect()

        return int(resp[0]) == 0

    def download_file(self, file_name: str) -> tuple[bool, bytes]:
        # Logic to download a file from LiNaStore
        self.connect()
        
        flags = 0x40.to_bytes(1, 'little')
        name_bin = file_name.encode()
        if len(name_bin) > self.LINA_NAME_MAX_LENGTH:
            raise ValueError("File name too long")
        else:
            for i in range(self.LINA_NAME_MAX_LENGTH - len(name_bin)):
                name_bin += b'\x00'
        length = int(0).to_bytes(4, 'little')
        checksum = binascii.crc32(name_bin + length).to_bytes(4, 'little')
                                                              
        self.socket.sendall(flags + name_bin + length + checksum, bytes())

        header = self.socket.recv(self.LINA_HEADER_LENGTH)
        data_pointer = 0

        flags = int(header[0])
        data_pointer += 1

        # name is no needed, just skip it
        data_pointer += self.LINA_NAME_MAX_LENGTH

        length = int.from_bytes(header[data_pointer: data_pointer + 4], 'little')
        data_pointer += 4

        checksum = int.from_bytes(header[data_pointer: data_pointer + 4], 'little')
        data_pointer += 4

        data = self.socket.recv(length)

        self.disconnect()

        if not self.verify_checksum(name_bin, length, data, checksum):
            return False, None
        else:
            return True, data
    
    def delete_file(self, file_name: str) -> bool:
        # Logic to delete a file from LiNaStore
        self.connect()

        flags = 0xC0.to_bytes(1, 'little')
        name_bin = file_name.encode()
        if len(name_bin) > self.LINA_NAME_MAX_LENGTH:
            raise ValueError("File name too long")
        else:
            for i in range(self.LINA_NAME_MAX_LENGTH - len(name_bin)):
                name_bin += b'\x00'
        length = int(0).to_bytes(4, 'little')
        checksum = binascii.crc32(name_bin + length).to_bytes(4, 'little')

        self.socket.sendall(flags + name_bin + length + checksum, bytes())

        resp = self.socket.recv(self.LINA_HEADER_LENGTH)

        self.disconnect()

        return int(resp[0]) == 0


    def verify_checksum(self, name_bin: bytes, length: bytes, data: bytes, checksum: int):
        calculated_checksum = binascii.crc32(name_bin + length.to_bytes(4, 'little') + data)
        return calculated_checksum == checksum