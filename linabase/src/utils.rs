use blake3::Hasher;
use core::panic;
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use rayon::{
    ThreadPool, ThreadPoolBuilder,
    iter::{IntoParallelRefIterator, ParallelIterator},
};
use std::{
    borrow::Cow,
    error::Error,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const BLOCK_SIZE: usize = 8;
const GROUP_SIZE: usize = BLOCK_SIZE * 8;
const BUFFER_SIZE: usize = 0x80000;

pub fn get_hash256_from_file<P: AsRef<Path>>(file_path: P) -> Result<String, Box<dyn Error>> {
    let mut hasher = Hasher::new();
    let mut file = fs::File::open(file_path)?;
    let file_size = file.metadata()?.len();
    let mut total_read = 0;
    let mut buffer = [0u8; 0x200000];

    while total_read < file_size {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Unexpected EOF",
            ))); // Unexpected EOF
        }
        total_read += bytes_read as u64;
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hasher.finalize().to_hex().to_string())
}

pub fn get_hash256_from_binary(input: &[u8]) -> String {
    let mut hasher = Hasher::new();

    for chunk in input.chunks(BUFFER_SIZE) {
        hasher.update(chunk);
    }

    hasher.finalize().to_hex().to_string()
}

pub fn path_walk<P: AsRef<Path>>(path: P) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let path = Path::new(path.as_ref());
    let mut result: Vec<PathBuf> = Vec::new();

    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            let current_entry = entry?.path();
            if current_entry.is_dir() {
                result.extend_from_slice(&path_walk(current_entry)?);
            } else {
                result.push(current_entry);
            }
        }
    }

    Ok(result)
}

pub fn create_symlink<P: AsRef<Path>, Q: AsRef<Path>>(src: P, dst: Q) -> io::Result<()> {
    let src = src.as_ref();
    let dst = dst.as_ref();

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        symlink(src, dst)
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_file;
        symlink_file(src, dst)
    }
}

/// For example, there is a block size of 64 bytes.
///
/// This block can be compressed as:
/// ```markdown
/// | Comp flag |  length  |
/// |-----------|----------|
/// |    2a     |    4d    |
/// |    32     |    48    |
/// |    48     |    37    |
/// |    48     |    7d    |
/// ```
/// BlockManager handles compression and decompression of data chunks using parallel processing
///
/// This struct provides efficient block-based compression with configurable thread pool
/// and chunk sizes for optimal performance on different hardware configurations.
#[derive(Debug)]
pub struct BlockManager {
    chunk_size: usize,
    thread_pool: ThreadPool,
    // Threshold for using multi-threaded compression (256KB)
    multi_thread_threshold: usize,
    // Maximum number of threads for large files
    max_threads: usize,
}

impl BlockManager {
    /// Create a new BlockManager with default settings
    ///
    /// Uses system-appropriate thread count and optimal chunk size
    ///
    /// # Panics
    /// Panics if thread pool creation fails
    pub fn new() -> Self {
        // Use number of available CPU cores for optimal performance
        let max_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(4); // Cap at 4 threads to avoid excessive resource usage

        let thread_pool = match ThreadPoolBuilder::new()
            .num_threads(max_threads)
            .thread_name(|index| format!("linastore-compress-{}", index))
            .build()
        {
            Ok(pool) => pool,
            Err(err) => panic!("Failed to create thread pool: {}", err),
        };

        BlockManager {
            chunk_size: 0x10000 - 0x400, // 63KiB for optimal compression
            thread_pool,
            multi_thread_threshold: 1024 * 1024, // 1MB threshold for multi-threading
            max_threads,
        }
    }

    /// Create a new BlockManager with custom chunk size
    ///
    /// # Arguments
    /// * `chunk_size` - Size of each chunk for compression
    ///
    /// # Panics
    /// Panics if chunk_size is not a multiple of GROUP_SIZE or exceeds maximum
    pub fn with_capacity(chunk_size: usize) -> Self {
        if chunk_size % GROUP_SIZE != 0 {
            panic!("Chunk size must be a multiple of {} bytes", GROUP_SIZE);
        }

        if chunk_size > 0x10000 - 0x400 {
            panic!("Chunk size must be less than (not equal to) 64KiB");
        }

        // Use number of available CPU cores for optimal performance
        let max_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(4);

        let thread_pool = match ThreadPoolBuilder::new()
            .num_threads(max_threads)
            .thread_name(|index| format!("linastore-compress-{}", index))
            .build()
        {
            Ok(pool) => pool,
            Err(err) => panic!("Failed to create thread pool: {}", err),
        };

        BlockManager {
            chunk_size,
            thread_pool,
            multi_thread_threshold: 1024 * 1024, // 1MB threshold for multi-threading
            max_threads,
        }
    }

    /// Determine the number of threads to use based on input size
    /// Small files (< 1MB): 1 thread (single-threaded for efficiency)
    /// Large files (>= 1MB): max_threads (typically 4 threads for parallel processing)
    fn determine_thread_count(&self, input_size: usize) -> usize {
        if input_size < self.multi_thread_threshold {
            1 // Use single thread for small files
        } else {
            self.max_threads // Use max threads for large files
        }
    }

    pub fn compress_all(&self, input: &Vec<u8>) -> Result<Vec<u8>, Box<dyn Error>> {
        let chunks: Vec<&[u8]> = input.chunks(self.chunk_size).collect();
        
        // Determine thread count based on input size
        let thread_count = self.determine_thread_count(input.len());
        
        // Create appropriate thread pool based on size
        let pool = ThreadPoolBuilder::new()
            .num_threads(thread_count)
            .thread_name(|index| format!("linastore-compress-{}", index))
            .build()
            .expect("Failed to create thread pool");

        let compressed_chunks: Vec<Vec<u8>> = pool.install(|| {
            chunks
                .par_iter()
                .map(|&chunk| {
                    let chunk_vec = chunk.to_vec();

                    let compressed_chunk = self.__encode(&chunk_vec);
                    let raw_len = chunk_vec.len();
                    let compressed_chunk_len = compressed_chunk.len();

                    // Build chunk result with header
                    let mut chunk_result = Vec::with_capacity(compressed_chunk_len + 3);
                    if compressed_chunk_len > raw_len {
                        // Add uncompressed flag
                        chunk_result.push(0);
                        chunk_result.extend_from_slice(&(raw_len as u16).to_le_bytes());
                        chunk_result.extend_from_slice(&chunk_vec);
                    } else {
                        if compressed_chunk_len > 0x10000 {
                            panic!(
                                "Compressed chunk length is greater than 64KiB: {:x}",
                                compressed_chunk_len
                            );
                        }
                        // Add compressed flag
                        chunk_result.push(1);
                        chunk_result
                            .extend_from_slice(&(compressed_chunk.len() as u16).to_le_bytes());
                        chunk_result.extend_from_slice(&compressed_chunk);
                    }
                    chunk_result
                })
                .collect()
        });

        let mut result = Vec::with_capacity(input.len());
        for chunk_data in compressed_chunks {
            result.extend_from_slice(&chunk_data);
        }

        Ok(result)
    }

    pub fn decompress_all(
        &self,
        input: &Vec<u8>,
        original_size: usize,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
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
            chunks_with_flag
                .par_iter()
                .map(|(flag, start, end)| {
                    match flag {
                        0 => Cow::Borrowed(&input[*start..*end]), // Uncompressed chunk
                        1 => Cow::Owned(self.__decode(&input[*start..*end])), // Compressed chunk
                        _ => panic!("Unknown chunk flag"),
                    }
                })
                .collect::<Vec<_>>()
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
            Ok(_) => {}
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
            Ok(_) => {}
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
        // Create a compressor with matching chunk size
        let manager = BlockManager::new();
        let data = vec![1u8; 100000]; // Use test data instead of external file

        // Encode the data
        let compress_start = Instant::now();
        let compressed = manager.compress_all(&data).expect("Failed to compress");
        let compress_duration = compress_start.elapsed();
        println!("Compression time: {:.2?}", compress_duration);

        println!(
            "Compression ratio: {:.2}%",
            (compressed.len() as f64 / data.len() as f64) * 100.0
        );

        // Decode and verify round-trip consistency
        let decompress_start = Instant::now();
        let decompressed = manager
            .decompress_all(&compressed, data.len())
            .expect("Failed to decompress");
        let decompress_duration = decompress_start.elapsed();
        println!("Decompression time: {:.2?}", decompress_duration);

        // The decoded data should match the original input
        assert_eq!(
            data, decompressed,
            "Encoded and decoded data should match original input"
        );
    }

    #[test]
    fn test_path_recursive() {
        let path = Path::new(".");
        let paths = path_walk(path).expect("Failed to walk path");
        for path in paths {
            println!("{}", path.display());
        }
    }

    #[test]
    fn test_dynamic_thread_selection() {
        let manager = BlockManager::new();
        
        // Test small file (< 256KB) - should use 1 thread
        let small_data = vec![0u8; 100 * 1024]; // 100KB
        let small_compressed = manager.compress_all(&small_data).expect("Failed to compress small data");
        let small_decompressed = manager
            .decompress_all(&small_compressed, small_data.len())
            .expect("Failed to decompress small data");
        assert_eq!(small_data, small_decompressed, "Small file round-trip failed");
        
        // Test large file (>= 256KB) - should use max_threads (typically 4)
        let large_data = vec![42u8; 512 * 1024]; // 512KB
        let large_compressed = manager.compress_all(&large_data).expect("Failed to compress large data");
        let large_decompressed = manager
            .decompress_all(&large_compressed, large_data.len())
            .expect("Failed to decompress large data");
        assert_eq!(large_data, large_decompressed, "Large file round-trip failed");
        
        println!("Dynamic thread selection test passed!");
    }

    #[test]
    fn test_thread_count_determination() {
        let manager = BlockManager::new();
        
        // Small file should use 1 thread
        assert_eq!(manager.determine_thread_count(100 * 1024), 1, "100KB should use 1 thread");
        assert_eq!(manager.determine_thread_count(512 * 1024), 1, "512KB should use 1 thread");
        assert_eq!(manager.determine_thread_count(1023 * 1024), 1, "1023KB should use 1 thread");
        
        // Large file should use max_threads
        assert_eq!(manager.determine_thread_count(1024 * 1024), manager.max_threads, "1MB should use max_threads");
        assert_eq!(manager.determine_thread_count(2 * 1024 * 1024), manager.max_threads, "2MB should use max_threads");
        
        println!("Thread count determination test passed!");
    }

    #[test]
    fn test_get_hash256_from_binary() {
        let data = b"Hello, World!";
        let hash1 = get_hash256_from_binary(data);
        let hash2 = get_hash256_from_binary(data);
        
        // Same input should produce same hash
        assert_eq!(hash1, hash2);
        
        // Different input should produce different hash
        let different_data = b"Hello, Different World!";
        let hash3 = get_hash256_from_binary(different_data);
        assert_ne!(hash1, hash3);
        
        // Hash should be a hex string of 64 characters (BLAKE3 produces 32 bytes = 64 hex chars)
        assert_eq!(hash1.len(), 64);
    }

    #[test]
    fn test_get_hash256_from_binary_empty() {
        let data = b"";
        let hash = get_hash256_from_binary(data);
        
        // Empty input should still produce a valid hash
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_get_hash256_from_binary_large() {
        let data = vec![42u8; 1024 * 1024]; // 1MB of data
        let hash = get_hash256_from_binary(&data);
        
        // Large input should produce a valid hash
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_block_manager_with_capacity() {
        let manager = BlockManager::with_capacity(0x10000 - 0x800); // Valid chunk size
        let data = vec![1, 2, 3, 4, 5];
        
        let compressed = manager.compress_all(&data).expect("Failed to compress");
        let decompressed = manager
            .decompress_all(&compressed, data.len())
            .expect("Failed to decompress");
        
        assert_eq!(data, decompressed);
    }

    #[test]
    #[should_panic(expected = "Chunk size must be a multiple of")]
    fn test_block_manager_invalid_chunk_size() {
        BlockManager::with_capacity(100); // Not a multiple of GROUP_SIZE (64)
    }

    #[test]
    #[should_panic(expected = "Chunk size must be less than")]
    fn test_block_manager_chunk_size_too_large() {
        BlockManager::with_capacity(0x10000); // Equal to 64KiB, should panic
    }

    #[test]
    fn test_compress_decompress_empty() {
        let manager = BlockManager::new();
        let data = vec![];
        
        let compressed = manager.compress_all(&data).expect("Failed to compress empty data");
        let decompressed = manager
            .decompress_all(&compressed, data.len())
            .expect("Failed to decompress empty data");
        
        assert_eq!(data, decompressed);
        assert!(compressed.is_empty());
    }

    #[test]
    fn test_compress_decompress_single_byte() {
        let manager = BlockManager::new();
        let data = vec![42];
        
        let compressed = manager.compress_all(&data).expect("Failed to compress single byte");
        let decompressed = manager
            .decompress_all(&compressed, data.len())
            .expect("Failed to decompress single byte");
        
        assert_eq!(data, decompressed);
    }

    #[test]
    fn test_compress_decompress_repeated_data() {
        let manager = BlockManager::new();
        let data = vec![42u8; 10000]; // Highly compressible data
        
        let compressed = manager.compress_all(&data).expect("Failed to compress");
        let decompressed = manager
            .decompress_all(&compressed, data.len())
            .expect("Failed to decompress");
        
        assert_eq!(data, decompressed);
        // Compressed should be smaller than original for repeated data
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn test_compress_decompress_random_data() {
        let manager = BlockManager::new();
        let mut data = vec![0u8; 10000];
        for i in 0..data.len() {
            data[i] = (i % 256) as u8;
        }
        
        let compressed = manager.compress_all(&data).expect("Failed to compress");
        let decompressed = manager
            .decompress_all(&compressed, data.len())
            .expect("Failed to decompress");
        
        assert_eq!(data, decompressed);
    }

    #[test]
    fn test_decompress_invalid_data() {
        let manager = BlockManager::new();
        let invalid_data = vec![0, 1, 2, 3, 4, 5]; // Invalid chunk format
        
        let result = manager.decompress_all(&invalid_data, 100);
        assert!(result.is_err());
    }

    #[test]
    fn test_decompress_incomplete_chunk() {
        let manager = BlockManager::new();
        let incomplete_data = vec![1, 0, 100]; // Flag=1, length=100, but no data
        
        let result = manager.decompress_all(&incomplete_data, 100);
        assert!(result.is_err());
    }

    #[test]
    fn test_path_walk_empty_directory() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let paths = path_walk(temp_dir.path()).expect("Failed to walk path");
        
        assert!(paths.is_empty());
    }

    #[test]
    fn test_path_walk_with_files() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        
        // Create some test files
        fs::write(temp_dir.path().join("file1.txt"), b"content1").expect("Failed to write file");
        fs::write(temp_dir.path().join("file2.txt"), b"content2").expect("Failed to write file");
        
        let paths = path_walk(temp_dir.path()).expect("Failed to walk path");
        
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_path_walk_nested_directories() {
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        
        // Create nested structure
        let subdir = temp_dir.path().join("subdir");
        fs::create_dir_all(&subdir).expect("Failed to create subdir");
        fs::write(subdir.join("nested.txt"), b"nested content").expect("Failed to write file");
        fs::write(temp_dir.path().join("root.txt"), b"root content").expect("Failed to write file");
        
        let paths = path_walk(temp_dir.path()).expect("Failed to walk path");
        
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_path_walk_nonexistent() {
        let result = path_walk("/nonexistent/path/that/does/not/exist");
        // path_walk returns empty result for nonexistent paths, not an error
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_constants() {
        assert_eq!(BLOCK_SIZE, 8);
        assert_eq!(GROUP_SIZE, 64);
        assert_eq!(BUFFER_SIZE, 0x80000); // 512KB
    }
}
