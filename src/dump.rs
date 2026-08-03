use std::io::{self, Write};
use std::num::NonZeroUsize;

use memmap2::{Advice, Mmap, MmapOptions};
use thiserror::Error;

use crate::source::LockedFile;

/// Bounds each mapping in the default mode without limiting total output.
const DEFAULT_MAP_LEN: NonZeroUsize = NonZeroUsize::new(1 << 30).unwrap();

/// Smallest mapping attempted after a mapping failure.
const MIN_MAP_LEN: NonZeroUsize = NonZeroUsize::new(4 * 1024).unwrap();

#[derive(Debug, Error)]
pub enum DumpError {
    #[error("failed to map {len} bytes at offset {offset} from a {file_len}-byte file")]
    Map {
        offset: u64,
        len: usize,
        file_len: u64,
        #[source]
        source: io::Error,
    },

    #[error("mapping at offset {offset} returned {actual} bytes; expected {expected}")]
    UnexpectedMappingLength {
        offset: u64,
        expected: usize,
        actual: usize,
    },

    #[error("failed to write file data at offset {offset}")]
    Write {
        offset: u64,
        #[source]
        source: io::Error,
    },

    #[error("mapping length {len} does not fit in u64")]
    MappingLengthOverflow { len: usize },

    #[error("file offset overflowed u64")]
    OffsetOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MappingMode {
    Chunked,
    WholeFile,
}

/// Streams a snapshot of a file range through read-only memory maps.
pub fn file_range<W: Write>(
    file: &LockedFile,
    start_offset: u64,
    mode: MappingMode,
    output: &mut W,
) -> Result<(), DumpError> {
    let file_len = file.len();
    if start_offset >= file_len {
        return Ok(());
    }

    let remaining = file_len - start_offset;
    let policy = MappingPolicy::for_range(remaining, mode);

    stream_mappings(file_len, start_offset, policy, output, |offset, len| {
        map_file_region(file, offset, len)
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MappingPolicy {
    preferred_len: NonZeroUsize,
}

impl MappingPolicy {
    fn for_range(remaining: u64, mode: MappingMode) -> Self {
        debug_assert!(remaining > 0);

        let preferred_len = match mode {
            MappingMode::Chunked => DEFAULT_MAP_LEN,
            MappingMode::WholeFile => {
                let platform_limit = isize::MAX.unsigned_abs();
                let remaining = usize::try_from(remaining).unwrap_or(platform_limit);
                NonZeroUsize::new(remaining.min(platform_limit))
                    .expect("a non-empty range has a non-zero mapping length")
            }
        };

        Self { preferred_len }
    }
}

fn stream_mappings<W, F, M>(
    file_len: u64,
    start_offset: u64,
    policy: MappingPolicy,
    output: &mut W,
    mut map_region: F,
) -> Result<(), DumpError>
where
    W: Write,
    F: FnMut(u64, usize) -> io::Result<M>,
    M: AsRef<[u8]>,
{
    let mut current_offset = start_offset;
    let mut preferred_len = policy.preferred_len.get();

    while current_offset < file_len {
        let remaining = file_len - current_offset;
        let mut requested_len = map_len(remaining, preferred_len);

        let mapping = loop {
            match map_region(current_offset, requested_len) {
                Ok(mapping) => break mapping,
                Err(error) => {
                    let Some(reduced_len) = reduce_map_len(requested_len) else {
                        return Err(DumpError::Map {
                            offset: current_offset,
                            len: requested_len,
                            file_len,
                            source: error,
                        });
                    };

                    requested_len = reduced_len;
                    preferred_len = reduced_len;
                }
            }
        };

        let bytes = mapping.as_ref();
        if bytes.len() != requested_len {
            return Err(DumpError::UnexpectedMappingLength {
                offset: current_offset,
                expected: requested_len,
                actual: bytes.len(),
            });
        }

        output.write_all(bytes).map_err(|source| DumpError::Write {
            offset: current_offset,
            source,
        })?;

        let written = u64::try_from(bytes.len())
            .map_err(|_| DumpError::MappingLengthOverflow { len: bytes.len() })?;
        current_offset = current_offset
            .checked_add(written)
            .ok_or(DumpError::OffsetOverflow)?;
    }

    Ok(())
}

fn map_len(remaining: u64, preferred_len: usize) -> usize {
    usize::try_from(remaining).map_or(preferred_len, |remaining| remaining.min(preferred_len))
}

fn reduce_map_len(current: usize) -> Option<usize> {
    (current > MIN_MAP_LEN.get()).then(|| (current / 2).max(MIN_MAP_LEN.get()))
}

fn map_file_region(file: &LockedFile, offset: u64, len: usize) -> io::Result<Mmap> {
    // SAFETY: `LockedFile` keeps the descriptor open and shared-locked while
    // the map is live, and `file_range` bounds the region to its length
    // snapshot. Linux locks are advisory, so the command's safety contract
    // still requires writers to participate in the same locking protocol.
    let mapping = unsafe {
        MmapOptions::new()
            .offset(offset)
            .len(len)
            .map(file.handle())?
    };

    // Sequential advice is an optional optimization and is not available on
    // every operating system supported by memmap2.
    let _ = mapping.advise(Advice::Sequential);

    Ok(mapping)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::{self, Write};
    use std::num::NonZeroUsize;

    use tempfile::NamedTempFile;

    use crate::source::LockedFile;

    use super::{
        DEFAULT_MAP_LEN, DumpError, MIN_MAP_LEN, MappingMode, MappingPolicy, file_range, map_len,
        reduce_map_len, stream_mappings,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn file_with(contents: &[u8]) -> io::Result<NamedTempFile> {
        let mut file = NamedTempFile::new()?;
        file.write_all(contents)?;
        file.flush()?;
        Ok(file)
    }

    fn synthetic_mapper<'a>(
        contents: &'a [u8],
        calls: &'a RefCell<Vec<(u64, usize)>>,
    ) -> impl FnMut(u64, usize) -> io::Result<Vec<u8>> + 'a {
        move |offset, len| {
            calls.borrow_mut().push((offset, len));
            let start = usize::try_from(offset).expect("test offset should fit in usize");
            Ok(contents[start..start + len].to_vec())
        }
    }

    #[test]
    fn maps_an_unaligned_file_offset() -> TestResult {
        let contents = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let file = file_with(contents)?;
        let locked_file = LockedFile::open(file.path())?;
        let mut output = Vec::new();

        file_range(&locked_file, 7, MappingMode::Chunked, &mut output)?;

        assert_eq!(output, &contents[7..]);
        Ok(())
    }

    #[test]
    fn maps_multiple_chunks_without_skips_or_overlap() -> TestResult {
        let contents = b"0123456789";
        let calls = RefCell::new(Vec::new());
        let policy = MappingPolicy {
            preferred_len: NonZeroUsize::new(3).expect("three is non-zero"),
        };
        let mut output = Vec::new();

        stream_mappings(
            u64::try_from(contents.len())?,
            0,
            policy,
            &mut output,
            synthetic_mapper(contents, &calls),
        )?;

        assert_eq!(output, contents);
        assert_eq!(*calls.borrow(), [(0, 3), (3, 3), (6, 3), (9, 1)]);
        Ok(())
    }

    #[test]
    fn selects_the_initial_mapping_length_for_each_mode() {
        let chunked = MappingPolicy::for_range(123, MappingMode::Chunked);
        let whole_file = MappingPolicy::for_range(123, MappingMode::WholeFile);
        let platform_limited = MappingPolicy::for_range(u64::MAX, MappingMode::WholeFile);

        assert_eq!(chunked.preferred_len, DEFAULT_MAP_LEN);
        assert_eq!(whole_file.preferred_len.get(), 123);
        assert_eq!(
            platform_limited.preferred_len.get(),
            isize::MAX.unsigned_abs()
        );
    }

    #[test]
    fn backs_off_and_reuses_the_successful_mapping_length() -> TestResult {
        let file_len = MIN_MAP_LEN.get() * 4;
        let calls = RefCell::new(Vec::new());
        let policy = MappingPolicy {
            preferred_len: NonZeroUsize::new(file_len).expect("length is non-zero"),
        };
        let mut output = Vec::new();

        stream_mappings(
            u64::try_from(file_len)?,
            0,
            policy,
            &mut output,
            |offset, len| {
                calls.borrow_mut().push((offset, len));
                if len > MIN_MAP_LEN.get() {
                    Err(io::Error::from(io::ErrorKind::OutOfMemory))
                } else {
                    Ok(vec![0_u8; len])
                }
            },
        )?;

        assert_eq!(output.len(), file_len);
        assert_eq!(
            *calls.borrow(),
            [
                (0, MIN_MAP_LEN.get() * 4),
                (0, MIN_MAP_LEN.get() * 2),
                (0, MIN_MAP_LEN.get()),
                (u64::try_from(MIN_MAP_LEN.get())?, MIN_MAP_LEN.get()),
                (u64::try_from(MIN_MAP_LEN.get() * 2)?, MIN_MAP_LEN.get()),
                (u64::try_from(MIN_MAP_LEN.get() * 3)?, MIN_MAP_LEN.get()),
            ]
        );
        Ok(())
    }

    #[test]
    fn reports_the_final_mapping_failure_with_its_range() {
        let policy = MappingPolicy {
            preferred_len: NonZeroUsize::new(MIN_MAP_LEN.get() * 2).expect("length is non-zero"),
        };
        let mut output = Vec::new();

        let error = stream_mappings(
            u64::try_from(MIN_MAP_LEN.get() * 2).expect("test length fits in u64"),
            0,
            policy,
            &mut output,
            |_offset, _len| -> io::Result<Vec<u8>> {
                Err(io::Error::new(io::ErrorKind::OutOfMemory, "forced failure"))
            },
        )
        .expect_err("mapping should fail");

        match error {
            DumpError::Map {
                offset,
                len,
                file_len,
                source,
            } => {
                assert_eq!(offset, 0);
                assert_eq!(len, MIN_MAP_LEN.get());
                assert_eq!(file_len, u64::try_from(MIN_MAP_LEN.get() * 2).unwrap());
                assert_eq!(source.kind(), io::ErrorKind::OutOfMemory);
            }
            other => panic!("expected a mapping error, got {other:?}"),
        }
    }

    #[test]
    fn preserves_broken_pipe_as_the_error_source() {
        struct ClosedPipe;

        impl Write for ClosedPipe {
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let policy = MappingPolicy {
            preferred_len: NonZeroUsize::new(4).expect("four is non-zero"),
        };
        let mut output = ClosedPipe;
        let error = stream_mappings(4, 0, policy, &mut output, |_offset, len| {
            Ok(vec![0_u8; len])
        })
        .expect_err("writing should fail");

        match error {
            DumpError::Write { offset, source } => {
                assert_eq!(offset, 0);
                assert_eq!(source.kind(), io::ErrorKind::BrokenPipe);
            }
            other => panic!("expected a write error, got {other:?}"),
        }
    }

    #[test]
    fn does_not_map_an_empty_range() -> TestResult {
        let file = file_with(b"content")?;
        let locked_file = LockedFile::open(file.path())?;
        let mut output = Vec::new();

        file_range(&locked_file, 7, MappingMode::Chunked, &mut output)?;
        file_range(&locked_file, 10, MappingMode::WholeFile, &mut output)?;

        assert!(output.is_empty());
        Ok(())
    }

    #[test]
    fn rejects_a_mapping_with_an_unexpected_length() {
        let policy = MappingPolicy {
            preferred_len: NonZeroUsize::new(4).expect("four is non-zero"),
        };
        let mut output = Vec::new();

        let error = stream_mappings(4, 0, policy, &mut output, |_offset, _len| Ok(vec![0_u8; 3]))
            .expect_err("short mapping should fail");

        assert!(matches!(
            error,
            DumpError::UnexpectedMappingLength {
                offset: 0,
                expected: 4,
                actual: 3
            }
        ));
    }

    #[test]
    fn caps_a_mapping_at_the_remaining_range() {
        assert_eq!(map_len(10_000, 1_000), 1_000);
        assert_eq!(map_len(500, 1_000), 500);
        assert_eq!(map_len(1_000, 1_000), 1_000);
    }

    #[test]
    fn backoff_stops_at_the_minimum_mapping_length() {
        assert_eq!(
            reduce_map_len(MIN_MAP_LEN.get() * 2),
            Some(MIN_MAP_LEN.get())
        );
        assert_eq!(
            reduce_map_len(MIN_MAP_LEN.get() + 1),
            Some(MIN_MAP_LEN.get())
        );
        assert_eq!(reduce_map_len(MIN_MAP_LEN.get()), None);
    }
}
