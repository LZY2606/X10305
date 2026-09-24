use std::cell::RefCell;
use std::io::{Error as IoError, ErrorKind, Read, Result as IoResult, Seek, SeekFrom, Write};
use std::rc::Rc;

/// Shared in-memory file that implements Read + Write + Seek.
///
/// This allows running the public `E57Writer` and `E57Reader`
/// completely in memory: fixture generation and verification leave
/// no temporary files behind. The shared handle makes it possible
/// to inspect the bytes after the writer has been dropped.
#[derive(Clone, Default)]
pub struct MemFile {
    data: Rc<RefCell<Vec<u8>>>,
    pos: usize,
}

impl MemFile {
    /// Creates a new empty in-memory file.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a copy of the current file content.
    /// Named `to_bytes` because the by-value `Read::bytes` method
    /// would otherwise win method resolution.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.data.borrow().clone()
    }

    /// Returns the current length of the file in bytes.
    pub fn len(&self) -> usize {
        self.data.borrow().len()
    }

    /// Returns true if the file contains no bytes.
    pub fn is_empty(&self) -> bool {
        self.data.borrow().is_empty()
    }
}

impl Read for MemFile {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let data = self.data.borrow();
        let available = data.len().saturating_sub(self.pos);
        let count = available.min(buf.len());
        buf[..count].copy_from_slice(&data[self.pos..self.pos + count]);
        self.pos += count;
        Ok(count)
    }
}

impl Write for MemFile {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        let mut data = self.data.borrow_mut();
        let end = self.pos + buf.len();
        if end > data.len() {
            data.resize(end, 0);
        }
        data[self.pos..end].copy_from_slice(buf);
        self.pos = end;
        Ok(buf.len())
    }

    fn flush(&mut self) -> IoResult<()> {
        Ok(())
    }
}

impl Seek for MemFile {
    fn seek(&mut self, pos: SeekFrom) -> IoResult<u64> {
        let len = self.data.borrow().len() as i64;
        let next = match pos {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::End(p) => len + p,
            SeekFrom::Current(p) => self.pos as i64 + p,
        };
        if next < 0 {
            return Err(IoError::new(
                ErrorKind::InvalidInput,
                "cannot seek to negative offset",
            ));
        }
        self.pos = next as usize;
        Ok(self.pos as u64)
    }
}

/// Writer wrapper that injects exactly one recoverable IO failure
/// at a configurable write call index.
///
/// Used to prove that a failed fixture generation leaves no caches,
/// counters or temporary state behind and that a retry with the
/// same inputs produces a stable result.
pub struct FlakyWriter<T> {
    inner: T,
    writes: usize,
    fail_at: usize,
    failed: bool,
}

impl<T> FlakyWriter<T> {
    /// Creates a new flaky writer that fails the write call with
    /// the given zero-based index exactly once.
    pub fn new(inner: T, fail_at: usize) -> Self {
        Self {
            inner,
            writes: 0,
            fail_at,
            failed: false,
        }
    }

    /// Returns the number of write calls that were attempted.
    pub fn write_attempts(&self) -> usize {
        self.writes
    }

    /// Consumes the wrapper and returns the inner writer.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: Read> Read for FlakyWriter<T> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        self.inner.read(buf)
    }
}

impl<T: Seek> Seek for FlakyWriter<T> {
    fn seek(&mut self, pos: SeekFrom) -> IoResult<u64> {
        self.inner.seek(pos)
    }
}

impl<T: Write> Write for FlakyWriter<T> {
    fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        let index = self.writes;
        self.writes += 1;
        if !self.failed && index == self.fail_at {
            self.failed = true;
            return Err(IoError::other("injected recoverable write failure"));
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> IoResult<()> {
        self.inner.flush()
    }
}
