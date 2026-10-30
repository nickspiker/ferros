//! Files on Android's partitions, read only, through the kernel's partition grants (process.rs `Object::Part`, partfs.rs `part_call`): [1] + path → the size; [2] + offset + path → the bytes from there, up to 1 MiB a call. The same shape as the kernel's partfs where modem.rs uses it.

use crate::S_PART;
use alloc::string::String;
use alloc::vec::Vec;

/// The partition grants in slot order from `S_PART` (main.rs).
const PARTS: [&str; 5] = ["modem_a", "efs", "persist", "modem_userdata", "vendor_a"];

/// A partition by its grant.
pub struct Mount {
    slot: u64,
    name: &'static str,
}

/// A file found on it: its path and size.
pub struct Inode {
    path: String,
    pub size: u64,
}

impl Inode {
    pub fn is_file(&self) -> bool {
        true
    }
}

pub fn mount(name: &str) -> Result<Mount, String> {
    let k = PARTS.iter().position(|p| *p == name).ok_or_else(|| alloc::format!("no grant for partition {name}"))?;
    Ok(Mount { slot: S_PART + k as u64, name: PARTS[k] })
}

impl Mount {
    pub fn open(&mut self, path: &str) -> Result<Inode, String> {
        let mut req = alloc::vec![1u8];
        req.extend_from_slice(path.as_bytes());
        let mut r = [0u8; 8];
        let n = ferros_sys::call(crate::hw::cap(self.slot), &req, &mut r);
        if n != 8 {
            return Err(alloc::format!("{}{path}: not found", self.name));
        }
        Ok(Inode { path: String::from(path), size: u64::from_le_bytes(r) })
    }

    /// `buf.len()` bytes (fewer at the end) from `offset`, in the kernel's chunks.
    pub fn read(&mut self, ino: &Inode, offset: u64, buf: &mut [u8]) -> Result<usize, String> {
        let mut req = alloc::vec![2u8];
        req.extend_from_slice(&[0; 8]);
        req.extend_from_slice(ino.path.as_bytes());
        let mut done = 0;
        while done < buf.len() && offset + (done as u64) < ino.size {
            req[1..9].copy_from_slice(&(offset + done as u64).to_le_bytes());
            let n = ferros_sys::call(crate::hw::cap(self.slot), &req, &mut buf[done..]);
            if n == 0 || n == ferros_sys::ERR {
                return Err(alloc::format!("{}{}: read at G#{:X} failed", self.name, ino.path, offset + done as u64));
            }
            done += n as usize;
        }
        Ok(done)
    }
}

/// A whole file.
pub fn read_f2fs(part: &str, path: &str) -> Result<Vec<u8>, String> {
    let mut m = mount(part)?;
    let ino = m.open(path)?;
    let mut v = alloc::vec![0u8; ino.size as usize];
    let n = m.read(&ino, 0, &mut v)?;
    v.truncate(n);
    Ok(v)
}
