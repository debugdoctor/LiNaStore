use chrono::Utc;
use uuid::Uuid;

#[derive(Clone, PartialEq)]
pub struct PayLoad {
    pub name: [u8; 256],
    pub length: u32,
    pub checksum: u32,
    pub data: Vec<u8>,
}

/// Flags Definition
/// ---
/// ```markdown
/// | File Operation | Reserved | Reserved | Reserved | Reserved | Cover | Compress |
/// |----------------|----------|----------|----------|----------|-------|----------|
/// | 0x88 - 0x40    | 0x20     | 0x10     | 0x08     | 0x04     | 0x02  | 0x01     |
/// ```
#[derive(Clone, PartialEq)]
pub struct ProtocolMessage {
    pub flags: u8,
    pub status: Status,
    pub payload: PayLoad,
}

impl ProtocolMessage {
    pub fn new() -> Self {
        ProtocolMessage {
            flags: 0x40,
            status: Status::None,
            payload: PayLoad {
                name: [0; 256],
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
        hasher.update(&self.payload.name);
        hasher.update(&self.payload.length.to_le_bytes());
        hasher.update(&self.payload.data);
        hasher.finalize()
    }

    pub fn serialize_protocol_message(
        &self,
    ) -> Vec<u8> {
        let mut payload = Vec::with_capacity(0x1000);

        payload.push(self.status.clone() as u8);
        payload.extend_from_slice(&self.payload.length.to_le_bytes());
        payload.extend_from_slice(&self.payload.checksum.to_le_bytes());
        payload.extend_from_slice(&self.payload.data);
        payload
    }
}



pub enum FlagType {
    DELETE = 0x80,
    SEND = 0x48,
    READ = 0x40,
    PAYLOAD = 0x20,
    COVER = 0x2,
    COMPRESS = 0x1,
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
                name: [0; 256],
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
                name: [0; 256],
                data: Vec::new(),
            },
            created_at: Utc::now().timestamp(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Content {
    pub flags: u8,
    pub name: [u8; 256],
    pub data: Vec<u8>,
}

// Should not excced u8::MAX
#[derive(Clone, PartialEq)]
pub enum Status {
    Success = 0,
    FileNotFound = 1,
    StoreFailed = 2,
    InvalidRequest = 3,
    FileNameInvalid = 4,
    InternalError = 127,
    None = 255,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Behavior {
    GetFile,
    PutFile,
    None,
}