//! List and copy files out of an f2fs partition image with ferros_f2fs, the read-only reader the kernel itself uses: the same code reads the phone's partitions, so what this shows is what ferros sees.
//!
//! `f2fs-cat <image> ls <dir>`: each entry's name, kind and size. `f2fs-cat <image> get <path> <out>`: one file to `out`.
//! A host file outside the image is only ever written by `get`, to the path given.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

struct Image(File);

impl ferros_f2fs::Disk for Image {
    fn read_block(&mut self, block: u64, buf: &mut [u8; ferros_f2fs::BLOCK]) -> Result<(), ferros_f2fs::Error> {
        self.0.seek(SeekFrom::Start(block * ferros_f2fs::BLOCK as u64)).map_err(|_| ferros_f2fs::Error::Io)?;
        self.0.read_exact(buf).map_err(|_| ferros_f2fs::Error::Io)
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 4 {
        eprintln!("usage: f2fs-cat <image> ls <dir> | get <path> <out>");
        std::process::exit(2);
    }
    let img = File::open(&a[1]).unwrap_or_else(|e| fail(&format!("{}: {e}", a[1])));
    let mut fs = ferros_f2fs::Fs::mount(Image(img)).unwrap_or_else(|e| fail(&format!("mount: {e:?}")));
    match a[2].as_str() {
        "ls" => {
            let dir = fs.open(a[3].as_bytes()).unwrap_or_else(|e| fail(&format!("{}: {e:?}", a[3])));
            let mut names = Vec::new();
            fs.list(&dir, &mut |name, ino| names.push((name.to_vec(), ino))).unwrap_or_else(|e| fail(&format!("list: {e:?}")));
            for (name, ino) in names {
                let i = fs.inode(ino).unwrap_or_else(|e| fail(&format!("inode {ino}: {e:?}")));
                println!("{} {} {}", String::from_utf8_lossy(&name), if i.is_dir() { "dir" } else if i.is_file() { "file" } else { "other" }, i.size);
            }
        }
        "get" if a.len() == 5 => {
            let ino = fs.open(a[3].as_bytes()).unwrap_or_else(|e| fail(&format!("{}: {e:?}", a[3])));
            let mut out = Vec::with_capacity(ino.size as usize);
            let mut buf = [0u8; ferros_f2fs::BLOCK];
            while (out.len() as u64) < ino.size {
                let n = fs.read_at(&ino, out.len() as u64, &mut buf).unwrap_or_else(|e| fail(&format!("read: {e:?}")));
                if n == 0 {
                    fail("read: short file");
                }
                out.extend_from_slice(&buf[..n]);
            }
            File::create(&a[4]).and_then(|mut f| f.write_all(&out)).unwrap_or_else(|e| fail(&format!("{}: {e}", a[4])));
        }
        _ => fail("usage: f2fs-cat <image> ls <dir> | get <path> <out>"),
    }
}

fn fail(m: &str) -> ! {
    eprintln!("f2fs-cat: {m}");
    std::process::exit(1)
}
