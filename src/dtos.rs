use chrono::Utc;
use uuid::Uuid;

pub const NAME_SIZE: usize = 255;

#[derive(Clone, PartialEq)]
pub struct PayLoad {
    pub identifier: [u8; NAME_SIZE],
    pub length: u32,
    pub checksum: u32,
    pub data: Vec<u8>,
}

/// Flags Definition
/// ---
/// ```markdown
/// | File Operation | Communicate Options | Reserved | Reserved | Cover | Compress |
/// |----------------|----------|----------|----------|----------|-------|----------|
/// | 0xC0 - 0x40    |     0x20 - 0x10     | 0x08     | 0x04     | 0x02  | 0x01     |
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
                identifier: [0; 255],
                length: 0,
                checksum: 0,
                data: Vec::new(),
            },
        }
    }

    pub fn verify(&self) -> bool {
        self.payload.checksum == self.calculate_checksum()
    }

    // Calculate CRC32 checksum
    pub fn calculate_checksum(&self) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(&self.payload.identifier);
        hasher.update(&self.payload.length.to_le_bytes());
        hasher.update(&self.payload.data);
        hasher.finalize()
    }

    pub fn serialize_protocol_message(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(0x1000);

        payload.push(self.status.clone() as u8);
        payload.extend_from_slice(&self.payload.identifier);
        payload.extend_from_slice(&self.payload.length.to_le_bytes());
        payload.extend_from_slice(&self.payload.checksum.to_le_bytes());
        payload.extend_from_slice(&self.payload.data);
        payload
    }
}

pub enum FlagType {
    Delete = 0xC0,
    Write = 0x80,
    Read = 0x40,
    Auth = 0x30,
    Cover = 0x02,
    Compress = 0x01,
    None = 0x00,
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
                identifier: [0; NAME_SIZE],
                data: Vec::new(),
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
                identifier: [0; NAME_SIZE],
                data: Vec::new(),
            },
            created_at: Utc::now().timestamp(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Content {
    pub flags: u8,
    pub identifier: [u8; NAME_SIZE],
    pub data: Vec<u8>,
}

// Should not excced u8::MAX
#[derive(Clone, PartialEq)]
pub enum Status {
    Success = 0,
    FileNotFound = 1,
    StoreFailed = 2,
    FileNameInvalid = 3,
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
