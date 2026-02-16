import socket
import io
import binascii
import hashlib
import time
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
import os
from typing import Optional

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
    AUTH = 0x60
    READ = 0x40
    COVER = 0x02
    COMPRESS = 0x01
    NONE = 0x00

    LINA_NAME_MAX_LENGTH = 255
    LINA_HEADER_BASE_LENGTH = 10  # flags(1) + ilen(1) + dlen(4) + checksum(4)

    def __init__(self, address: str, port: int, timeout: int = 5,
                 auto_refresh: bool = True, refresh_buffer: int = 300):
        """
        Initialize LiNaStore client.
        
        Args:
            address: Server IP address or hostname (e.g., "127.0.0.1" or "example.com")
            port: Server port
            timeout: Connection timeout in seconds
            auto_refresh: Enable automatic token refresh when expired
            refresh_buffer: Buffer time in seconds before expiration to refresh token
        """
        self.address = address
        self.port = port
        self.timeout = timeout
        self.socket = None
        self.session_token = None
        self.token_expires_at = None  # Unix timestamp when token expires
        self._cached_username = None  # Cached username for auto-refresh
        self._cached_password = None  # Cached password for auto-refresh
        self.auto_refresh = auto_refresh
        self.refresh_buffer = refresh_buffer  # Refresh token N seconds before expiration
        
    def connect(self):
        # Logic to connect to the LiNaStore service
        try:
            self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self.socket.settimeout(self.timeout)
            self.socket.connect((self.address, self.port))
        except socket.error as e:
            raise LiNaStoreConnectionError(f"Failed to connect to {self.address}:{self.port}: {str(e)}")

    def disconnect(self):
        # Logic to disconnect from the LiNaStore service
        if self.socket:
            try:
                self.socket.close()
            except socket.error:
                pass  # Ignore errors during disconnect
            finally:
                self.socket = None

    def set_session_token(self, token: str):
        """Set the session token for authentication and encryption"""
        self.session_token = token

    def _is_token_expired(self) -> bool:
        """
        Check if the current session token is expired or about to expire.
        
        Returns:
            True if token is expired or will expire within refresh_buffer seconds
        """
        if self.token_expires_at is None:
            return True  # No token, treat as expired
        
        current_time = int(time.time())
        return current_time >= (self.token_expires_at - self.refresh_buffer)

    def _refresh_token_if_needed(self) -> None:
        """
        Refresh the session token if it's expired or about to expire.
        Uses cached credentials for automatic re-authentication.
        
        Raises:
            LiNaStoreProtocolError: If refresh fails and no credentials are cached
        """
        if not self.auto_refresh:
            return

        # Auth-free mode: no token and no cached credentials means no refresh needed.
        if self.session_token is None and not (self._cached_username and self._cached_password):
            return
        
        if self._is_token_expired():
            if self._cached_username and self._cached_password:
                # Use cached credentials to refresh
                self.handshake(self._cached_username, self._cached_password)
            else:
                raise LiNaStoreProtocolError(
                    "Token expired and no cached credentials available for refresh. "
                    "Please call handshake() with username and password again."
                )

    def cache_credentials(self, username: str, password: str) -> None:
        """
        Cache username and password in memory for automatic token refresh.
        
        Warning: Passwords are stored in plain text in memory. Use with caution
        and consider clearing credentials after use.
        
        Args:
            username: Username to cache
            password: Password to cache
        """
        self._cached_username = username
        self._cached_password = password

    def clear_cached_credentials(self) -> None:
        """Clear cached username and password from memory."""
        self._cached_username = None
        self._cached_password = None

    def get_token_info(self) -> dict:
        """
        Get information about the current session token.
        
        Returns:
            Dictionary with token info:
            - has_token: bool
            - is_expired: bool
            - expires_at: int (Unix timestamp) or None
            - expires_in: int (seconds until expiration) or None
            - has_cached_credentials: bool
        """
        current_time = int(time.time())
        
        if self.token_expires_at is None:
            return {
                'has_token': False,
                'is_expired': True,
                'expires_at': None,
                'expires_in': None,
                'has_cached_credentials': bool(self._cached_username and self._cached_password)
            }
        
        return {
            'has_token': True,
            'is_expired': self._is_token_expired(),
            'expires_at': self.token_expires_at,
            'expires_in': max(0, self.token_expires_at - current_time),
            'has_cached_credentials': bool(self._cached_username and self._cached_password)
        }

    def handshake(self, username: str, password: str, cache_credentials: bool = True) -> tuple[str, int]:
        """
        Perform authentication handshake with the server.
        
        Args:
            username: Username for authentication (max 255 bytes)
            password: Password for authentication
            cache_credentials: Whether to cache credentials for auto-refresh (default: True)
            
        Returns:
            Tuple of (session_token, expires_at_timestamp)
            
        Raises:
            LiNaStoreConnectionError: If connection fails
            LiNaStoreProtocolError: If protocol error occurs or authentication fails
        """
        # Cache credentials for auto-refresh if requested
        if cache_credentials:
            self.cache_credentials(username, password)
        if not self.socket:
            self.connect()
        
        try:
            # Prepare auth request
            # Flags: AUTH (0x60)
            flags = self.AUTH.to_bytes(1, 'little')
            
            # Identifier: username (null-terminated)
            username_bytes = username.encode()
            if len(username_bytes) > self.LINA_NAME_MAX_LENGTH:
                raise LiNaStoreProtocolError(f"Username too long: {len(username_bytes)} > {self.LINA_NAME_MAX_LENGTH}")
            ilen = len(username_bytes).to_bytes(1, 'little')
            
            # Data: password (null-terminated)
            password_bytes = password.encode()
            password_data = password_bytes + b'\x00'
            dlen = len(password_data).to_bytes(4, 'little')
            
            # Calculate checksum
            checksum = binascii.crc32(ilen + username_bytes + dlen + password_data).to_bytes(4, 'little')
            
            # Send auth request
            try:
                self.socket.sendall(flags + ilen + username_bytes + dlen + checksum + password_data)
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to send auth request: {str(e)}")
            
            # Receive response header
            try:
                header_len = self.LINA_HEADER_BASE_LENGTH  # No identifier in response
                header = self.socket.recv(header_len)
                if len(header) < header_len:
                    raise LiNaStoreProtocolError(f"Incomplete header received: {len(header)} < {header_len}")
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to receive auth response header: {str(e)}")
            
            # Parse response header
            data_pointer = 0
            status = int(header[data_pointer])
            data_pointer += 1
            
            ilen_recv = int(header[data_pointer])
            data_pointer += 1
            
            # Skip identifier (should be 0)
            data_pointer += ilen_recv
            
            dlen_recv = int.from_bytes(header[data_pointer: data_pointer + 4], 'little')
            data_pointer += 4
            
            checksum_recv = int.from_bytes(header[data_pointer: data_pointer + 4], 'little')
            data_pointer += 4
            
            # Check for error status
            if status != 0:
                # Receive error status from data field
                if dlen_recv > 0:
                    error_data = self._recv_all(dlen_recv)
                    if error_data:
                        error_code = error_data[0]
                        error_messages = {
                            1: "Invalid password",
                            2: "Authentication disabled",
                            127: "Internal server error"
                        }
                        msg = error_messages.get(error_code, f"Authentication failed with error code: {error_code}")
                        raise LiNaStoreProtocolError(msg)
                raise LiNaStoreProtocolError(f"Authentication failed with status: {status}")
            
            # Receive response data: status(1) + token + '\0' + expires_at
            if dlen_recv > 0:
                response_data = self._recv_all(dlen_recv)
                
                # Parse response: status(1) + token + '\0' + expires_at
                handshake_status = response_data[0]
                
                if handshake_status == 0:  # Success
                    # Find null terminator after token
                    null_pos = response_data.find(0, 1)  # Start from position 1 (after status)
                    if null_pos == -1:
                        raise LiNaStoreProtocolError("Invalid auth response: missing null terminator")
                    
                    token = response_data[1:null_pos].decode('utf-8')
                    expires_at_str = response_data[null_pos + 1:].decode('utf-8')
                    expires_at = int(expires_at_str)
                    
                    # Store session token and expiration time
                    self.session_token = token
                    self.token_expires_at = expires_at
                    
                    return token, expires_at
                else:
                    error_messages = {
                        1: "Invalid password",
                        2: "Authentication disabled",
                        127: "Internal server error"
                    }
                    msg = error_messages.get(handshake_status, f"Handshake failed with status: {handshake_status}")
                    raise LiNaStoreProtocolError(msg)
            
            raise LiNaStoreProtocolError("Empty auth response received")
            
        finally:
            # Don't disconnect after handshake - keep connection for subsequent operations
            pass

    def encrypt_with_token(self, token: str, data: bytes) -> bytes:
        """Encrypt data using the session token as the encryption key"""
        # Derive a 256-bit key from the token using SHA-256
        key = hashlib.sha256(token.encode()).digest()
        
        # Create AES-GCM cipher
        aesgcm = AESGCM(key)
        
        # Generate a random nonce (96 bits for AES-GCM)
        nonce = os.urandom(12)
        
        # Encrypt the data
        ciphertext = aesgcm.encrypt(nonce, data, None)
        
        # Return nonce + ciphertext (nonce is needed for decryption)
        return nonce + ciphertext

    def decrypt_with_token(self, token: str, encrypted_data: bytes) -> bytes:
        """Decrypt data using the session token as the decryption key"""
        # Derive a 256-bit key from the token using SHA-256
        key = hashlib.sha256(token.encode()).digest()
        
        # Create AES-GCM cipher
        aesgcm = AESGCM(key)
        
        # Extract nonce (first 12 bytes) and ciphertext
        nonce_size = 12
        if len(encrypted_data) < nonce_size:
            raise ValueError("Encrypted data is too short")
        
        nonce = encrypted_data[:nonce_size]
        ciphertext = encrypted_data[nonce_size:]
        
        # Decrypt the data
        plaintext = aesgcm.decrypt(nonce, ciphertext, None)
        
        return plaintext

    def upload_file(self, file_name: str, reader: io.BufferedReader) -> bool:
        """
        Upload a file to LiNaStore.
        
        Automatically refreshes the session token if it's expired and auto_refresh is enabled.
        
        Args:
            file_name: Name of the file to upload
            reader: BufferedReader containing file data
            
        Returns:
            True if upload successful
            
        Raises:
            LiNaStoreConnectionError: If connection fails
            LiNaStoreProtocolError: If protocol error occurs or authentication fails
        """
        # Refresh token if needed before operation
        self._refresh_token_if_needed()
        
        # Logic to upload a file to LiNaStore
        if not self.socket:
            self.connect()
        
        try:
            file_data = reader.read()
            
            # Encrypt data if session token is available
            if self.session_token:
                file_data = self.encrypt_with_token(self.session_token, file_data)
                # Prepend token to encrypted data for server decryption
                file_data = self.session_token.encode() + b'\x00' + file_data
            
            flags = 0x80.to_bytes(1, 'little')
            identifier = file_name.encode()
            if len(identifier) > self.LINA_NAME_MAX_LENGTH:
                raise LiNaStoreProtocolError(f"File name too long: {len(identifier)} > {self.LINA_NAME_MAX_LENGTH}")
            ilen = len(identifier).to_bytes(1, 'little')
            dlen = len(file_data).to_bytes(4, 'little')
            checksum = binascii.crc32(ilen + identifier + dlen + file_data).to_bytes(4, 'little')
            
            try:
                self.socket.sendall(flags + ilen + identifier + dlen + checksum + file_data)
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to send data for file {file_name}: {str(e)}")

            try:
                header_len = self.LINA_HEADER_BASE_LENGTH + len(identifier)
                resp = self._recv_all(header_len)
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to receive response for file {file_name}: {str(e)}")

            if resp[0] != 0:
                raise LiNaStoreProtocolError(f"Server returned error code: {resp[0]} for file: {file_name}")

            return True
        finally:
            self.disconnect()

    def download_file(self, file_name: str) -> bytes:
        """
        Download a file from LiNaStore.
        
        Automatically refreshes the session token if it's expired and auto_refresh is enabled.
        
        Args:
            file_name: Name of the file to download
            
        Returns:
            File data as bytes
            
        Raises:
            LiNaStoreConnectionError: If connection fails
            LiNaStoreProtocolError: If protocol error occurs or authentication fails
        """
        # Refresh token if needed before operation
        self._refresh_token_if_needed()
        
        # Logic to download a file from LiNaStore
        if not self.socket:
            self.connect()
        
        try:
            flags = 0x40.to_bytes(1, 'little')
            identifier = file_name.encode()
            if len(identifier) > self.LINA_NAME_MAX_LENGTH:
                raise LiNaStoreProtocolError(f"File name too long: {len(identifier)} > {self.LINA_NAME_MAX_LENGTH}")
            ilen = len(identifier).to_bytes(1, 'little')
            
            # Include session token in data field for authenticated requests
            if self.session_token:
                data = self.session_token.encode() + b'\x00'
            else:
                data = b''
            dlen = len(data).to_bytes(4, 'little')
            checksum = binascii.crc32(ilen + identifier + dlen + data).to_bytes(4, 'little')
                                                               
            try:
                self.socket.sendall(flags + ilen + identifier + dlen + checksum + data)
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to send request for file {file_name}: {str(e)}")

            try:
                header_len = self.LINA_HEADER_BASE_LENGTH + len(identifier)
                header = self._recv_all(header_len)
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to receive header for file {file_name}: {str(e)}")
            
            data_pointer = 0

            flags = int(header[0])
            data_pointer += 1

            ilen_recv = int(header[data_pointer])
            data_pointer += 1

            identifier_recv = header[data_pointer: data_pointer + ilen_recv]
            data_pointer += ilen_recv

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

            if not self.verify_checksum(identifier_recv, length, data, checksum):
                raise LiNaStoreChecksumError(f"Checksum verification failed for file: {file_name}")
            
            return data
        finally:
            self.disconnect()
    
    def delete_file(self, file_name: str) -> bool:
        """
        Delete a file from LiNaStore.
        
        Automatically refreshes the session token if it's expired and auto_refresh is enabled.
        
        Args:
            file_name: Name of the file to delete
            
        Returns:
            True if deletion successful
            
        Raises:
            LiNaStoreConnectionError: If connection fails
            LiNaStoreProtocolError: If protocol error occurs or authentication fails
        """
        # Refresh token if needed before operation
        self._refresh_token_if_needed()
        
        # Logic to delete a file from LiNaStore
        if not self.socket:
            self.connect()
        
        try:
            flags = 0xC0.to_bytes(1, 'little')
            identifier = file_name.encode()
            if len(identifier) > self.LINA_NAME_MAX_LENGTH:
                raise LiNaStoreProtocolError(f"File name too long: {len(identifier)} > {self.LINA_NAME_MAX_LENGTH}")
            ilen = len(identifier).to_bytes(1, 'little')
            
            # Include session token in data field for authenticated requests
            if self.session_token:
                data = self.session_token.encode() + b'\x00'
            else:
                data = b''
            dlen = len(data).to_bytes(4, 'little')
            checksum = binascii.crc32(ilen + identifier + dlen + data).to_bytes(4, 'little')

            try:
                self.socket.sendall(flags + ilen + identifier + dlen + checksum + data)
            except socket.error as e:
                raise LiNaStoreConnectionError(f"Failed to send delete request for file {file_name}: {str(e)}")

            try:
                header_len = self.LINA_HEADER_BASE_LENGTH + len(identifier)
                resp = self._recv_all(header_len)
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

    def verify_checksum(self, identifier: bytes, length: int, data: bytes, checksum: int):
        ilen = len(identifier).to_bytes(1, 'little')
        calculated_checksum = binascii.crc32(ilen + identifier + length.to_bytes(4, 'little') + data)
        return calculated_checksum == checksum
