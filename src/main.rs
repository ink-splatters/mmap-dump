#![forbid(unsafe_op_in_unsafe_fn)]
#![warn(clippy::pedantic, clippy::nursery)]

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use memmap2::{Advice, MmapOptions};

/// Default chunk size for conservative memory mapping: 1 GiB.
const DEFAULT_CHUNK_SIZE: usize = 1 << 30;

/// Minimum threshold for exponential backoff during aggressive allocation.
/// Below this size, backoff stops and mapping failure is reported.
/// Aligned to common page size.
const MIN_MAP_SIZE: NonZeroUsize = match NonZeroUsize::new(4096) {
    Some(v) => v,
    None => unreachable!(),
};

/// Buffer capacity for stdout writes.
const STDOUT_BUF_CAPACITY: usize = 64 << 10;

/// Efficiently seeks through a file and dumps content from a specified offset.
/// Uses memory mapping with optional adaptive allocation strategies.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the target file
    #[arg(value_name = "FILE")]
    file_path: PathBuf,

    /// Start offset in bytes
    #[arg(value_name = "OFFSET")]
    offset: u64,

    /// Attempt to map the entire remaining file into memory.
    /// On allocation failure, gracefully degrades via exponential backoff
    /// (halving request size) until mapping succeeds or minimum threshold reached.
    #[arg(long, short = 'u')]
    unlimited_memory: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let file = File::open(&args.file_path)
        .with_context(|| format!("Failed to open file: {}", args.file_path.display()))?;

    // Acquire shared advisory lock. Concurrent readers permitted; signals to writers
    // (requesting exclusive locks) that file is in use.
    // POSIX caveat: advisory locks are cooperative, not enforced by kernel.
    file.try_lock_shared()
        .context("Failed to acquire shared lock. Another process may hold an exclusive lock.")?;

    let file_len = file
        .metadata()
        .context("Failed to read file metadata")?
        .len();

    if args.offset >= file_len {
        // Offset at or beyond EOF—nothing to output.
        return Ok(());
    }

    let stdout = io::stdout().lock();
    let mut writer = BufWriter::with_capacity(STDOUT_BUF_CAPACITY, stdout);

    dump_content(
        &file,
        file_len,
        args.offset,
        args.unlimited_memory,
        &mut writer,
    )?;

    writer.flush().context("Failed to flush stdout")?;

    Ok(())
}

/// Computes initial mapping chunk size.
///
/// Aggressive mode: attempts full remaining size (capped at `usize::MAX` for 32-bit).
/// Conservative mode: uses `DEFAULT_CHUNK_SIZE`.
const fn initial_chunk_size(remaining: u64, aggressive: bool) -> usize {
    if aggressive {
        // Saturate to usize::MAX on 32-bit platforms with large files.
        if remaining > usize::MAX as u64 {
            usize::MAX
        } else {
            // SAFETY: Just verified remaining <= usize::MAX
            #[allow(clippy::cast_possible_truncation)]
            let size = remaining as usize;
            size
        }
    } else {
        DEFAULT_CHUNK_SIZE
    }
}

/// Calculates the mapping length for current iteration.
///
/// Returns the minimum of `chunk_size` and remaining bytes, handling potential
/// truncation on 32-bit platforms.
const fn compute_map_len(remaining: u64, chunk_size: usize) -> usize {
    if remaining > chunk_size as u64 {
        chunk_size
    } else {
        // SAFETY: Just verified remaining <= chunk_size, which is usize
        #[allow(clippy::cast_possible_truncation)]
        let len = remaining as usize;
        len
    }
}

/// Memory-maps file content in adaptive chunks and streams to writer.
///
/// # Backoff Strategy
/// When mapping fails in aggressive mode, the requested size is halved
/// until either mapping succeeds or size falls below `MIN_MAP_SIZE`.
///
/// # Errors
/// Returns error if:
/// - Memory mapping fails after exhausting backoff attempts
/// - Write to output fails
fn dump_content<W: Write>(
    file: &File,
    file_len: u64,
    start_offset: u64,
    aggressive_alloc: bool,
    writer: &mut W,
) -> Result<()> {
    let mut current_offset = start_offset;
    let mut chunk_size =
        initial_chunk_size(file_len.saturating_sub(start_offset), aggressive_alloc);

    while current_offset < file_len {
        let remaining = file_len - current_offset;
        let mut map_len = compute_map_len(remaining, chunk_size);

        let mmap = loop {
            // SAFETY:
            // 1. `file` is a valid, open handle outliving the returned `Mmap`.
            // 2. Shared advisory lock held—reduces (does not eliminate) concurrent modification risk.
            // 3. Range `[current_offset, current_offset + map_len)` is within bounds:
            //    - `current_offset < file_len` (loop invariant)
            //    - `map_len <= remaining` (via `compute_map_len`)
            // 4. Resulting `Mmap` is accessed only as `&[u8]` (immutable).
            // 5. No mutable aliases exist; mapping dropped before next iteration.
            //
            // Residual risk: Non-cooperating processes may modify file contents
            // (POSIX advisory locks are not mandatory). This yields undefined behavior
            // per Rust's memory model—documented and accepted for this use case.
            let result = unsafe {
                MmapOptions::new()
                    .offset(current_offset)
                    .len(map_len)
                    .map(file)
            };

            match result {
                Ok(mmap) => {
                    // Advise kernel of sequential access pattern.
                    // Non-critical optimization; ignore failure.
                    let _ = mmap.advise(Advice::Sequential);
                    break mmap;
                }
                Err(_) if aggressive_alloc && map_len > MIN_MAP_SIZE.get() => {
                    // Exponential backoff: halve requested size.
                    map_len /= 2;
                    // Persist reduced expectation for subsequent chunks.
                    chunk_size = map_len;
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!(
                            "Memory-map failed: {map_len} bytes at offset {current_offset} \
                             (file size: {file_len} bytes)"
                        )
                    });
                }
            }
        };

        writer.write_all(&mmap).context("Write to output failed")?;

        // Safe: mmap.len() <= map_len <= remaining <= file_len - current_offset
        current_offset += mmap.len() as u64;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use anyhow::Result;
    use tempfile::NamedTempFile;

    use super::*;

    /// Helper to create a temporary file with given content.
    fn temp_file_with(content: &[u8]) -> Result<(File, u64)> {
        let mut tmp = NamedTempFile::new()?;
        tmp.write_all(content)?;
        tmp.flush()?;
        let file = tmp.reopen()?;
        Ok((file, content.len() as u64))
    }

    #[test]
    fn reads_from_offset() -> Result<()> {
        let content = b"Hello, world! This is a test file for mmap.";
        let (file, len) = temp_file_with(content)?;

        let mut buf = Vec::new();
        dump_content(&file, len, 7, false, &mut buf)?;

        assert_eq!(buf, &content[7..]);
        Ok(())
    }

    #[test]
    fn reads_full_file() -> Result<()> {
        let content = b"Complete file content here.";
        let (file, len) = temp_file_with(content)?;

        let mut buf = Vec::new();
        dump_content(&file, len, 0, true, &mut buf)?;

        assert_eq!(buf, content.as_slice());
        Ok(())
    }

    #[test]
    fn handles_empty_range_at_eof() -> Result<()> {
        let content = b"Some content";
        let (file, len) = temp_file_with(content)?;

        let mut buf = Vec::new();
        // Offset exactly at file length.
        dump_content(&file, len, len, false, &mut buf)?;

        assert!(buf.is_empty());
        Ok(())
    }

    #[test]
    fn handles_single_byte_file() -> Result<()> {
        let (file, len) = temp_file_with(b"X")?;

        let mut buf = Vec::new();
        dump_content(&file, len, 0, false, &mut buf)?;

        assert_eq!(buf, b"X");
        Ok(())
    }

    #[test]
    fn handles_offset_one_before_eof() -> Result<()> {
        let content = b"ABCDE";
        let (file, len) = temp_file_with(content)?;

        let mut buf = Vec::new();
        dump_content(&file, len, 4, false, &mut buf)?;

        assert_eq!(buf, b"E");
        Ok(())
    }

    #[test]
    fn aggressive_mode_works_on_small_file() -> Result<()> {
        let content = b"Small file for aggressive alloc testing";
        let (file, len) = temp_file_with(content)?;

        let mut buf = Vec::new();
        dump_content(&file, len, 0, true, &mut buf)?;

        assert_eq!(buf, content.as_slice());
        Ok(())
    }

    #[test]
    fn initial_chunk_size_conservative() {
        assert_eq!(initial_chunk_size(100, false), DEFAULT_CHUNK_SIZE);
        assert_eq!(initial_chunk_size(u64::MAX, false), DEFAULT_CHUNK_SIZE);
    }

    #[test]
    fn initial_chunk_size_aggressive() {
        assert_eq!(initial_chunk_size(1000, true), 1000);
        assert_eq!(initial_chunk_size(100_000_000_000, true), {
            #[cfg(target_pointer_width = "64")]
            {
                100_000_000_000_usize
            }
            #[cfg(target_pointer_width = "32")]
            {
                usize::MAX
            }
        });
    }

    #[test]
    fn compute_map_len_caps_at_chunk_size() {
        assert_eq!(compute_map_len(10_000, 1000), 1000);
        assert_eq!(compute_map_len(500, 1000), 500);
        assert_eq!(compute_map_len(1000, 1000), 1000);
    }
}
