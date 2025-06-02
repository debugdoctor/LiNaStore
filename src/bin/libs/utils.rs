use blake3::Hasher;
use rayon::{iter::{IntoParallelRefIterator, ParallelIterator}, ThreadPool, ThreadPoolBuilder};
use core::panic;
use std::{borrow::Cow, error::Error, fs, io::{self, Read, Write}, path::Path};
use flate2::{read::GzDecoder, write::GzEncoder, Compression};

const BLOCK_SIZE: usize = 8;
const GROUP_SIZE: usize = BLOCK_SIZE * 8;

pub fn get_hash256<P: AsRef<Path>>(file_path: P) -> Result<String, Box<dyn Error>> {
    let mut hasher = Hasher::new();
    let mut file = fs::File::open(file_path)?;
    let file_size = file.metadata()?.len();
    let mut total_read = 0;
    let mut buffer = [0u8; 0x100000]; 

    while total_read < file_size {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            return Err(Box::new(io::Error::new(io::ErrorKind::UnexpectedEof, "Unexpected EOF"))); // Unexpected EOF
        }
        total_read += bytes_read as u64;
        hasher.update(&buffer[..bytes_read]);
    }
    
    Ok(hasher.finalize().to_hex().to_string())
}

/// For example, there is a block size of 64 bytes.
/// 
/// This block can be compressed as:
/// ```
/// |blocks_flg |max_value |
/// |-----------|----------|
/// |block1_flag|----------|
/// |-----------|----------|
/// |    2a     |    4d    |
/// |    32     |    48    |
/// |    48     |    37    |
/// |    48     |    7d    |
/// ```
/// 
pub struct BlockManager {
    chunk_size: usize,
    thread_pool: ThreadPool,
}

impl BlockManager {
    pub fn new() -> Self {
        let thread_pool = match ThreadPoolBuilder::new()
            .num_threads(4)
            .build() {
                Ok(pool) => pool,
                Err(err) => panic!("{}", err)
            };

        BlockManager { chunk_size: 0x10000 - 0x400, thread_pool }
    }

    pub fn with_capacity(
        chunk_size: usize,
    ) -> Self {
        if chunk_size % GROUP_SIZE != 0 {
            panic!("Must be multiples of 64 Byte");
        }
        
        if chunk_size > 0x10000 - 0x400 {
            panic!("Chunk size must be less than (not equal to) 64KiB");
        }

        let thread_pool = match ThreadPoolBuilder::new()
            .num_threads(4)
            .build() {
                Ok(pool) => pool,
                Err(err) => panic!("{}", err)
            };

        BlockManager { chunk_size, thread_pool }
    }

    pub fn compress_all(&self, input: &Vec<u8>) -> Result<Vec<u8>, Box<dyn Error>> { 
        let chunks: Vec<&[u8]> = input.chunks(self.chunk_size).collect();

        let compressed_chunks = self.thread_pool.install(|| {
            chunks.par_iter().map(|&chunk| {
                let chunk_vec = chunk.to_vec();

                let compressed_chunk = self.__encode(&chunk_vec);
                let raw_len = chunk_vec.len();
                let compressed_chunk_len = compressed_chunk.len();

                // Build chunk result with header
                let mut chunk_result = Vec::with_capacity(compressed_chunk_len + 3);
                if compressed_chunk_len > raw_len {
                    chunk_result.push(0);
                    chunk_result.extend_from_slice(&(raw_len as u16).to_le_bytes());
                    chunk_result.extend_from_slice(&chunk_vec);
                } else {
                    if compressed_chunk_len > 0x10000 {
                        panic!("Compressed chunk length is greater than 64KiB: {:x}", compressed_chunk_len);
                    }
                    chunk_result.push(1);
                    chunk_result.extend_from_slice(&(compressed_chunk.len() as u16).to_le_bytes());
                    chunk_result.extend_from_slice(&compressed_chunk);
                }
                chunk_result
            }).collect::<Vec<_>>()
        });

        let mut result = Vec::with_capacity(input.len());
        for chunk_data in compressed_chunks {
            result.extend_from_slice(&chunk_data);
        }

        Ok(result)
    }

    pub fn decompress_all(&self, input: &Vec<u8>, original_size: usize) -> Result<Vec<u8>, Box<dyn Error>> {
        let mut i = 0;
        let mut chunks_with_flag = Vec::with_capacity(0x400000);

        while i < input.len() {
            // Ensure at least 2 bytes available for length
            if i + 3 > input.len() {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Incomplete chunk length",
                )));
            }

            // Read chunk flag and chunk length (u16, little-endian)
            let flag = input[i];
            let len_bytes = [input[i + 1], input[i + 2]];
            let chunk_len = u16::from_le_bytes(len_bytes) as usize;
            i += 3;

            // Ensure enough data is available for this chunk
            if i + chunk_len > input.len() {
                return Err(Box::new(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Incomplete chunk data",
                )));
            }

            chunks_with_flag.push((flag, i, i + chunk_len));
            i += chunk_len;
        }

        let decompressed_chunks = self.thread_pool.install(|| {
            chunks_with_flag.par_iter().map(|(flag, start, end)| {
                match flag {
                    0 => Cow::Borrowed(&input[*start..*end]), // Uncompressed chunk
                    1 => Cow::Owned(self.__decode(&input[*start..*end])), // Compressed chunk
                    _ => panic!("Unknown chunk flag"),
                }
            }).collect::<Vec<_>>()
        });

        let mut result = Vec::with_capacity(original_size + GROUP_SIZE);

        for chunk_result in decompressed_chunks {
            result.extend_from_slice(&chunk_result);
        }

        Ok(result)
    }
    // Input bytes less than 0x10000 (64KiB) - 0xa
    fn __encode(&self, chunk: &[u8]) -> Vec<u8> {
        // Mutable size array
        let result = Vec::with_capacity(u16::MAX as usize);
        
        let mut encoder = GzEncoder::new(result, Compression::fast());
        match encoder.write_all(chunk) {
            Ok(_)  => {},
            Err(e) => panic!("Failed to encode chunk: {}", e),
        }

        match encoder.finish() {
            Ok(compressed_data) => compressed_data,
            Err(e) => panic!("Failed to finalize compression: {}", e),
        }
    }

    fn __decode(&self, chunk: &[u8]) -> Vec<u8> {
        let mut result = Vec::with_capacity(u16::MAX as usize);
        let mut decoder = GzDecoder::new(chunk);

        match decoder.read_to_end(&mut result) {
            Ok(_) => {},
            Err(e) => panic!("Failed to write chunk for decompression: {}", e),
        }
        
        result
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn test_encode_consistency() {
        // Convert hex dump to byte array
        // Create a compressor with matching chunk size
        let manager = BlockManager::new();
        let data = fs::read("../../Hadoop.jar").expect("Failed to read file");

        
        // Encode the data
        let compress_start = Instant::now(); 
        let compressed = manager.compress_all(&data).expect(" Failed to compress");
        let compress_duration = compress_start.elapsed();
        println!("Compression time: {:.2?}", compress_duration);

        println!("Compression ratio: {:.2}%", 
            (compressed.len() as f64 / data.len() as f64) * 100.0);
        
        // Decode and verify round-trip consistency
        let decompress_start = Instant::now(); 
        let decompressed = manager.decompress_all(&compressed, data.len()).expect(" Failed to decompress");
        let decompress_duration = decompress_start.elapsed();
        println!("Decompression time: {:.2?}", decompress_duration);
        
        // The decoded data should match the original input
        assert_eq!(data, decompressed, "Encoded and decoded data should match original input");
    }
}