use blake3::Hasher;
use rayon::{iter::{IntoParallelRefIterator, ParallelIterator}, ThreadPool, ThreadPoolBuilder};
use core::panic;
use std::{error::Error, fs, io::{self, Read}, path::Path};

const BLOCK_SIZE: usize = 8;
const GROUP_SIZE: usize = BLOCK_SIZE * 8;
const BIT_MASKS: [u8; 8] = [0x80, 0x40, 0x20, 0x10, 0x08, 0x04, 0x02, 0x01];

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
        let input_size = input.len();
    
        // Calculate padding size
        let pad_size = if input_size % GROUP_SIZE != 0 {
            GROUP_SIZE - (input_size % GROUP_SIZE)
        } else {
            0
        };

        let chunks: Vec<&[u8]> = input.chunks(self.chunk_size).collect();

        let compressed_chunks = self.thread_pool.install(|| {
            chunks.par_iter().map(|&chunk| {
                let mut chunk_vec = chunk.to_vec();
                if chunk.len() % GROUP_SIZE != 0 {
                    chunk_vec.extend_from_slice(&vec![0u8; pad_size]);
                }

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
        let mut chunks_with_flag = Vec::with_capacity(input.len());

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

            let chunk_data = &input[i..i + chunk_len];
            chunks_with_flag.push((flag, chunk_data));
            i += chunk_len;
        }

        let decompressed_chunks = self.thread_pool.install(|| {
            chunks_with_flag.par_iter().map(|(flag, chunk)| {
                match flag {
                    0 => chunk.to_vec(), // Uncompressed chunk
                    1 => self.__decode(chunk), // Compressed chunk
                    _ => panic!("Unknown chunk flag"),
                }
            }).collect::<Vec<Vec<u8>>>()
        });

        let mut result = Vec::with_capacity(original_size + GROUP_SIZE);

        for chunk_result in decompressed_chunks {
            result.extend_from_slice(&chunk_result);
        }

        result.truncate(original_size);

        Ok(result)
    }
    // Input bytes less than 0x10000 (64KiB) - 0xa
    fn __encode(&self, chunk: &[u8]) -> Vec<u8> {
        let chunk_len = chunk.len();

        if chunk_len > self.chunk_size  || chunk_len % GROUP_SIZE != 0{
            panic!("Input bytes length is invalid: {:x}", chunk.len());
        }
        // Mutable size array
        let mut result = Vec::with_capacity(u16::MAX as usize);
        let mut payload = Vec::with_capacity(GROUP_SIZE / BLOCK_SIZE + GROUP_SIZE + 2);
        // Fixed size array
        let mut count_map = [0u8; 256];
        let mut payload_buf = [0u8; BLOCK_SIZE + 1];

        let chunk_ptr = chunk.as_ptr();

        for group_start in (0..chunk_len).step_by(GROUP_SIZE) {
            // Get group pointer
            let group_ptr = unsafe { chunk_ptr.add(group_start) };

            // Frequency count for the group
            unsafe { core::ptr::write_bytes(count_map.as_mut_ptr(), 0, 256) };

            let first_byte = unsafe { *group_ptr };
            count_map[first_byte as usize] = 1;
            let (mut record_byte, mut max_freq) = (first_byte, 1);

            for i in 1..GROUP_SIZE {
                let byte = unsafe { *group_ptr.add(i) };
                let count = unsafe { count_map.get_unchecked_mut(byte as usize) };
                *count += 1;
                
                if *count > max_freq {
                    record_byte = byte;
                    max_freq = *count;
                }
            }
            
            // Subblock compression
            let mut subblock_compressed = 0u8;
            unsafe { core::ptr::write_bytes(payload_buf.as_mut_ptr(), 0, BLOCK_SIZE + 1); }
        
            // Subblock compression
            payload.clear();
            for block in (0..GROUP_SIZE).step_by(BLOCK_SIZE) {
                let mut bitmask = 0u8;
                let block_ptr = unsafe { group_ptr.add(block) };

                let mut payload_buf_pos = 0;
                
                for i in 0..BLOCK_SIZE {
                    let byte = unsafe { *block_ptr.add(i) };
                    if byte == record_byte {
                        bitmask |= BIT_MASKS[i];
                    } else {
                        payload_buf[payload_buf_pos] = byte;
                        payload_buf_pos += 1;
                    }
                }
                
                if payload_buf_pos + 1 < BLOCK_SIZE && max_freq as usize > 2 {
                    subblock_compressed |= BIT_MASKS[block / BLOCK_SIZE];
                    unsafe {
                        payload.push(bitmask);
                        payload.extend_from_slice(core::slice::from_raw_parts(payload_buf.as_ptr(), payload_buf_pos));
                    }
                } else {
                    payload.extend_from_slice(&chunk[group_start +  block.. group_start + block + BLOCK_SIZE]);
                }
            }
            
            // Write result, the order is important
            result.push(subblock_compressed);
            if subblock_compressed != 0 {
                result.push(record_byte);
            }
            result.extend_from_slice(&payload);
        }
        
        result
    }

    fn __decode(&self, chunk: &[u8]) -> Vec<u8> {
        let chunk_len = chunk.len();
        let mut result = Vec::with_capacity(self.chunk_size);
        let num_blocks_per_group = GROUP_SIZE / BLOCK_SIZE;
        let mut i = 0;

        let mut block = [0u8; BLOCK_SIZE];

        while i < chunk_len {
            let subblock_compressed = unsafe { *chunk.get_unchecked(i) };
            i += 1;

            if subblock_compressed == 0 {
                // Uncompressed group: directly copy GROUP_SIZE bytes
                if i + GROUP_SIZE > chunk_len {
                    panic!("Invalid chunk length");
                }

                result.extend_from_slice(&chunk[i..i + GROUP_SIZE]);
                i += GROUP_SIZE;
                continue;
            }

            // Compressed group
            let record_byte = chunk[i];
            i += 1;

            for block_idx in 0..num_blocks_per_group {
                let bit_mask = BIT_MASKS[block_idx];
                if (subblock_compressed & bit_mask) == 0 {
                    // Block not compressed
                    let end = i + BLOCK_SIZE;
                    if end > chunk_len {
                        panic!("Invalid chunk length");
                    }
                    result.extend_from_slice(&chunk[i..end]);
                    i = end;
                    continue;
                }

                // Block is compressed
                let bitmask = chunk[i];
                i += 1;

                let num_payload_bytes = BLOCK_SIZE - bitmask.count_ones() as usize;

                if i + num_payload_bytes > chunk_len {
                    panic!("Invalid chunk length");
                }

                let payload = &chunk[i..i + num_payload_bytes];
                i += num_payload_bytes;

                let mut payload_idx = 0;

                for j in 0..BLOCK_SIZE {
                    if (bitmask & BIT_MASKS[j]) != 0 {
                        block[j] = record_byte;
                    } else {
                        if payload_idx >= payload.len() {
                            panic!("Payload index out of bounds");
                        }
                        block[j] = payload[payload_idx];
                        payload_idx += 1;
                    }
                }

                result.extend_from_slice(&block);
            }
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
        let data = fs::read("../causestracing.sql").expect("Failed to read file");

        
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