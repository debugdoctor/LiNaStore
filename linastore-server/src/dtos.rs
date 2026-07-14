use bytes::{Bytes, BytesMut};
use chrono::Utc;
use uuid::Uuid;

#[derive(Clone, PartialEq)]
pub struct PayLoad {
    pub ilen: u8,            // Identifier length (variable length identifier)
    pub identifier: Bytes,   // Variable length identifier
    pub dlen: u32,           // Data length
    pub checksum: u32,
    pub data: Bytes,
}

/// Flags Definition
/// ---
/// ```markdown
/// | File Operation | Communicate Options | Reserved | Reserved | Cover | Compress |
/// |----------------|----------|----------|----------|----------|-------|----------|
/// | 0xC0 - 0x40    |     0x30 - 0x10     | 0x08     | 0x04     | 0x02  | 0x01     |
/// ```
#[derive(Clone, PartialEq)]
pub struct LiNaProtocol {
    pub flags: u8,
    pub status: Status, // Only for server response
    pub payload: PayLoad,
}

impl LiNaProtocol {
    pub fn new() -> Self {
        LiNaProtocol {
            flags: FlagType::None as u8,
            status: Status::None,
            payload: PayLoad {
                ilen: 0,
                identifier: Bytes::new(),
                dlen: 0,
                checksum: 0,
                data: Bytes::new(),
            },
        }
    }

    /// Parsed operation derived from the top 3 bits of `flags`.
    #[inline]
    pub fn op(&self) -> Op {
        Op::from_flags(self.flags)
    }

    pub fn verify(&self) -> bool {
        self.payload.checksum == self.calculate_checksum()
    }

    // Calculate CRC32 checksum
    pub fn calculate_checksum(&self) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&[self.payload.ilen]);
        hasher.update(&self.payload.identifier);
        hasher.update(&self.payload.dlen.to_le_bytes());
        hasher.update(&self.payload.data);
        hasher.finalize()
    }

    pub fn serialize_protocol_message(&self) -> Bytes {
        // status(1) + ilen(1) + identifier(ilen) + dlen(4) + checksum(4) + data
        let cap = 1 + 1 + self.payload.identifier.len() + 4 + 4 + self.payload.data.len();
        let mut buf = BytesMut::with_capacity(cap);

        buf.extend_from_slice(&[self.status.clone() as u8, self.payload.ilen]);
        buf.extend_from_slice(&self.payload.identifier);
        buf.extend_from_slice(&self.payload.dlen.to_le_bytes());
        buf.extend_from_slice(&self.payload.checksum.to_le_bytes());
        buf.extend_from_slice(&self.payload.data);
        buf.freeze()
    }
}

#[allow(dead_code)]
pub enum FlagType {
    Delete = 0xC0,
    Write = 0x80,
    Auth = 0x60,
    Read = 0x40,
    Cover = 0x02,
    Compress = 0x01,
    None = 0x00,
}

/// Parsed file-operation field — the top 3 bits of `flags`.
///
/// The bitmask layout used `Delete=0xC0=0b110_xxxxx`, `Write=0xB80=0b100`,
/// `Auth=0x60=0b011`, `Read=0x40=0b010`. Decoding via bitwise AND (e.g.
/// `flags & Read == Read`) is order-sensitive — `Delete & Read == Read` —
/// so any caller that doesn't check Delete first silently corrupts the
/// dispatch. This enum is the single source of truth and removes that trap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    None,
    Read,
    Write,
    Delete,
    Auth,
}

impl Op {
    /// Bitmask covering the file-operation field (top 3 bits of `flags`).
    pub const OP_MASK: u8 = 0b1110_0000;
    const OP_SHIFT: u8 = 5;

    #[inline]
    pub fn from_flags(flags: u8) -> Op {
        match (flags & Self::OP_MASK) >> Self::OP_SHIFT {
            0b010 => Op::Read,
            0b011 => Op::Auth,
            0b100 => Op::Write,
            0b110 => Op::Delete,
            _ => Op::None,
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Package {
    pub status: Status,
    pub uni_id: [u8; 16],
    pub behavior: Behavior,
    pub content: Content,
    pub created_at: i64,
}

impl Package {
    pub fn new() -> Self {
        Package {
            status: Status::None,
            uni_id: Uuid::new_v4().into_bytes(),
            behavior: Behavior::None,
            content: Content {
                flags: 0x40,
                identifier: Bytes::new(),
                data: Bytes::new(),
            },
            created_at: Utc::now().timestamp(),
        }
    }

    pub fn new_with_id(uni_id: &Uuid) -> Self {
        Package {
            status: Status::None,
            uni_id: uni_id.into_bytes(),
            behavior: Behavior::None,
            content: Content {
                flags: 0,
                identifier: Bytes::new(),
                data: Bytes::new(),
            },
            created_at: Utc::now().timestamp(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Content {
    pub flags: u8,
    pub identifier: Bytes, // Variable length identifier
    pub data: Bytes,
}

// Should not excced u8::MAX
#[derive(Clone, PartialEq, Debug)]
pub enum Status {
    Success = 0,
    FileNotFound = 1,
    StoreFailed = 2,
    FileNameInvalid = 3,
    Unauthorized = 4,
    BadRequest = 5,
    InternalError = 127,
    None = 255,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Behavior {
    GetFile,
    PutFile,
    DeleteFile,
    None,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payload_new() {
        let payload = PayLoad {
            ilen: 0,
            identifier: Bytes::new(),
            dlen: 0,
            checksum: 0,
            data: Bytes::new(),
        };

        assert_eq!(payload.ilen, 0);
        assert_eq!(payload.identifier.len(), 0);
        assert_eq!(payload.dlen, 0);
        assert_eq!(payload.checksum, 0);
        assert!(payload.data.is_empty());
    }

    #[test]
    fn test_lina_protocol_new() {
        let protocol = LiNaProtocol::new();

        assert_eq!(protocol.flags, FlagType::None as u8);
        assert_eq!(protocol.status, Status::None);
        assert_eq!(protocol.payload.ilen, 0);
        assert_eq!(protocol.payload.identifier.len(), 0);
        assert_eq!(protocol.payload.dlen, 0);
        assert!(protocol.payload.data.is_empty());
    }

    #[test]
    fn test_calculate_checksum() {
        let mut protocol = LiNaProtocol::new();
        protocol.payload.data = Bytes::from(vec![1, 2, 3, 4, 5]);
        protocol.payload.dlen = 5;

        let checksum = protocol.calculate_checksum();
        assert_ne!(checksum, 0);
    }

    #[test]
    fn test_verify_checksum_valid() {
        let mut protocol = LiNaProtocol::new();
        protocol.payload.data = Bytes::from(vec![1, 2, 3, 4, 5]);
        protocol.payload.dlen = 5;
        protocol.payload.checksum = protocol.calculate_checksum();

        assert!(protocol.verify());
    }

    #[test]
    fn test_verify_checksum_invalid() {
        let mut protocol = LiNaProtocol::new();
        protocol.payload.data = Bytes::from(vec![1, 2, 3, 4, 5]);
        protocol.payload.dlen = 5;
        protocol.payload.checksum = 12345; // Invalid checksum

        assert!(!protocol.verify());
    }

    #[test]
    fn test_serialize_protocol_message() {
        let mut protocol = LiNaProtocol::new();
        protocol.payload.identifier = Bytes::from(&b"test"[..]);
        protocol.payload.ilen = 4;
        protocol.payload.data = Bytes::from(vec![1, 2, 3, 4, 5]);
        protocol.payload.dlen = 5;
        protocol.payload.checksum = protocol.calculate_checksum();
        protocol.status = Status::Success;

        let serialized = protocol.serialize_protocol_message();

        // Status (1 byte) + ilen (1 byte) + identifier (4 bytes) + dlen (4 bytes) + checksum (4 bytes) + data (5 bytes)
        assert_eq!(serialized.len(), 1 + 1 + 4 + 4 + 4 + 5);
        assert_eq!(serialized[0], Status::Success as u8);
    }

    #[test]
    fn test_flag_type_values() {
        assert_eq!(FlagType::Delete as u8, 0xC0);
        assert_eq!(FlagType::Write as u8, 0x80);
        assert_eq!(FlagType::Auth as u8, 0x60);
        assert_eq!(FlagType::Read as u8, 0x40);
        assert_eq!(FlagType::Cover as u8, 0x02);
        assert_eq!(FlagType::Compress as u8, 0x01);
        assert_eq!(FlagType::None as u8, 0x00);
    }

    #[test]
    fn test_status_values() {
        assert_eq!(Status::Success as u8, 0);
        assert_eq!(Status::FileNotFound as u8, 1);
        assert_eq!(Status::StoreFailed as u8, 2);
        assert_eq!(Status::FileNameInvalid as u8, 3);
        assert_eq!(Status::Unauthorized as u8, 4);
        assert_eq!(Status::BadRequest as u8, 5);
        assert_eq!(Status::InternalError as u8, 127);
        assert_eq!(Status::None as u8, 255);
    }

    #[test]
    fn test_package_new() {
        let package = Package::new();

        assert_eq!(package.status, Status::None);
        assert_eq!(package.uni_id.len(), 16);
        assert_eq!(package.behavior, Behavior::None);
        assert_eq!(package.content.flags, 0x40);
        assert!(package.content.data.is_empty());
    }

    #[test]
    fn test_package_new_with_id() {
        let uuid = uuid::Uuid::new_v4();
        let package = Package::new_with_id(&uuid);

        assert_eq!(package.uni_id, uuid.into_bytes());
        assert_eq!(package.status, Status::None);
        assert_eq!(package.behavior, Behavior::None);
    }

    #[test]
    fn test_content_new() {
        let content = Content {
            flags: 0x01,
            identifier: Bytes::from(vec![42u8]),
            data: Bytes::from(vec![1, 2, 3]),
        };

        assert_eq!(content.flags, 0x01);
        assert_eq!(content.identifier[0], 42);
        assert_eq!(content.data.len(), 3);
    }

    #[test]
    fn test_behavior_equality() {
        assert_eq!(Behavior::GetFile, Behavior::GetFile);
        assert_ne!(Behavior::GetFile, Behavior::PutFile);
        assert_ne!(Behavior::PutFile, Behavior::DeleteFile);
    }

    #[test]
    fn test_op_from_flags_basic_values() {
        assert_eq!(Op::from_flags(FlagType::None as u8), Op::None);
        assert_eq!(Op::from_flags(FlagType::Read as u8), Op::Read);
        assert_eq!(Op::from_flags(FlagType::Auth as u8), Op::Auth);
        assert_eq!(Op::from_flags(FlagType::Write as u8), Op::Write);
        assert_eq!(Op::from_flags(FlagType::Delete as u8), Op::Delete);
    }

    #[test]
    fn test_op_does_not_confuse_delete_with_read() {
        // Delete = 0xC0 = 0b1100_0000. Old code did `flags & Read == Read`,
        // and 0xC0 & 0x40 == 0x40, so without ordering Delete was read as Read.
        // The enum-based parser must classify it as Delete regardless of caller.
        assert_eq!(Op::from_flags(0xC0), Op::Delete);
        // Option bits must not perturb the classification.
        assert_eq!(
            Op::from_flags(FlagType::Delete as u8 | FlagType::Cover as u8 | FlagType::Compress as u8),
            Op::Delete,
        );
        assert_eq!(
            Op::from_flags(FlagType::Write as u8 | FlagType::Compress as u8),
            Op::Write,
        );
    }

    #[test]
    fn test_op_unknown_bit_patterns_fall_back_to_none() {
        // 0b001 / 0b101 / 0b111 are not assigned in the spec.
        assert_eq!(Op::from_flags(0b0010_0000), Op::None);
        assert_eq!(Op::from_flags(0b1010_0000), Op::None);
        assert_eq!(Op::from_flags(0b1110_0000), Op::None);
    }

    #[test]
    fn test_protocol_op_with_option_bits() {
        let mut p = LiNaProtocol::new();
        p.flags = FlagType::Write as u8 | FlagType::Cover as u8;
        assert_eq!(p.op(), Op::Write);

        p.flags = FlagType::Read as u8 | FlagType::Compress as u8;
        assert_eq!(p.op(), Op::Read);
    }

    #[test]
    fn test_payload_with_large_data() {
        let large_data = vec![42u8; 100000];
        let mut protocol = LiNaProtocol::new();
        protocol.payload.data = Bytes::from(large_data.clone());
        protocol.payload.dlen = large_data.len() as u32;
        protocol.payload.checksum = protocol.calculate_checksum();

        assert!(protocol.verify());
        assert_eq!(protocol.payload.dlen, 100000);
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let mut original = LiNaProtocol::new();
        original.payload.identifier = Bytes::from(&b"test"[..]);
        original.payload.ilen = 4;
        original.payload.data = Bytes::from(vec![10, 20, 30, 40, 50]);
        original.payload.dlen = 5;
        original.payload.checksum = original.calculate_checksum();
        original.status = Status::Success;

        let serialized = original.serialize_protocol_message();

        // Verify serialization contains expected data
        assert_eq!(serialized[0], Status::Success as u8);

        // The data should be at the end
        let data_start = 1 + 1 + 4 + 4 + 4; // status + ilen + identifier + dlen + checksum
        assert_eq!(&serialized[data_start..], &[10, 20, 30, 40, 50][..]);
    }
}
