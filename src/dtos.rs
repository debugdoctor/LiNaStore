use chrono::Utc;

#[derive(Clone, PartialEq)]
pub struct PayLoad {
    pub name: [u8; 256],
    pub length: u32,
    pub checksum: u32,
    pub data: Vec<u8>,
}

/// flags definition
/// ---
///
/// | Send? | Payload? | Reserved | Reserved | Reserved | Reserved | Cover? | Compress? |
#[derive(Clone, PartialEq)]
pub struct ProtocolMessage {
    pub flags: u8,
    pub status: Status,
    pub payload: PayLoad,
}

impl ProtocolMessage {
    pub fn new() -> Self {
        ProtocolMessage {
            flags: 0,
            status: Status::None,
            payload: PayLoad {
                name: [0; 256],
                length: 0,
                checksum: 0,
                data: Vec::new(),
            },
        }
    }
}



pub enum FlagType {
    SEND = 0x80,
    PAYLOAD = 0x40,
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
            uni_id: [0; 16],
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

#[derive(Clone, PartialEq)]
pub enum Behavior {
    GetFile,
    PutFile,
    None,
}