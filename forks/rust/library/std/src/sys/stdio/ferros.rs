//! Stdout and stderr go to the kernel's console (grant slot 0, the line prefixed with the process's name); stdin is empty.

use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};

pub struct Stdin;
pub struct Stdout;
pub type Stderr = Stdout;

impl Stdin {
    pub const fn new() -> Stdin {
        Stdin
    }
}

impl io::Read for Stdin {
    #[inline]
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Ok(0)
    }

    #[inline]
    fn read_buf(&mut self, _cursor: BorrowedCursor<'_>) -> io::Result<()> {
        Ok(())
    }

    #[inline]
    fn read_vectored(&mut self, _bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        Ok(0)
    }

    #[inline]
    fn is_read_vectored(&self) -> bool {
        false
    }

    #[inline]
    fn read_exact(&mut self, buf: &mut [u8]) -> io::Result<()> {
        if !buf.is_empty() { Err(io::Error::READ_EXACT_EOF) } else { Ok(()) }
    }

    #[inline]
    fn read_buf_exact(&mut self, cursor: BorrowedCursor<'_>) -> io::Result<()> {
        if cursor.capacity() != 0 { Err(io::Error::READ_EXACT_EOF) } else { Ok(()) }
    }

    #[inline]
    fn read_to_end(&mut self, _buf: &mut Vec<u8>) -> io::Result<usize> {
        Ok(0)
    }

    #[inline]
    fn read_to_string(&mut self, _buf: &mut String) -> io::Result<usize> {
        Ok(0)
    }
}

impl Stdout {
    pub const fn new() -> Stdout {
        Stdout
    }
}

impl io::Write for Stdout {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        ferros_rt::console::write(buf);
        Ok(buf.len())
    }

    #[inline]
    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let mut total = 0;
        for b in bufs {
            ferros_rt::console::write(b);
            total += b.len();
        }
        Ok(total)
    }

    #[inline]
    fn is_write_vectored(&self) -> bool {
        true
    }

    #[inline]
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub const STDIN_BUF_SIZE: usize = 0;

pub fn is_ebadf(_err: &io::Error) -> bool {
    true
}

pub fn panic_output() -> Option<impl io::Write> {
    Some(Stdout::new())
}
