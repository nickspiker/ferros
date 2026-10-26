//! Against a real image: `F2FS_IMAGE=<path> cargo test -p ferros_f2fs -- --nocapture` lists the root and reads every regular file in it; with `F2FS_EXPECT=<dir>` each file must equal the one of the same name there (from `dump.f2fs -i <ino>`). Skipped without an image: the images are per-unit and never in the repo.
use ferros_f2fs::{Disk, Error, Fs, BLOCK};
use std::io::{Read, Seek, SeekFrom};

struct File(std::fs::File);
impl Disk for File {
    fn read_block(&mut self, block: u64, buf: &mut [u8; BLOCK]) -> Result<(), Error> {
        self.0.seek(SeekFrom::Start(block * BLOCK as u64)).map_err(|_| Error::Io)?;
        self.0.read_exact(buf).map_err(|_| Error::Io)
    }
}

#[test]
fn image() {
    let Ok(path) = std::env::var("F2FS_IMAGE") else {
        eprintln!("F2FS_IMAGE not set: skipped");
        return;
    };
    let mut fs = Fs::mount(File(std::fs::File::open(path).unwrap())).unwrap();
    println!("checkpoint version {:x}", fs.checkpoint_ver);
    let root = fs.root().unwrap();
    let mut names = Vec::new();
    fs.list(&root, &mut |n, ino| names.push((n.to_vec(), ino))).unwrap();
    for (n, ino) in &names {
        let name = String::from_utf8_lossy(n).into_owned();
        let f = fs.inode(*ino).unwrap();
        assert_eq!(fs.lookup(&root, n).unwrap(), *ino);
        if !f.is_file() {
            println!("{name} ino {ino} mode {:o}", f.mode);
            continue;
        }
        let mut data = vec![0u8; f.size as usize];
        assert_eq!(fs.read_at(&f, 0, &mut data).unwrap(), data.len());
        let sum: u64 = data.iter().map(|&b| b as u64).sum();
        println!("{name} ino {ino} size {} byte sum {sum}{}", f.size, if f.size <= 64 { format!(" text {:?}", String::from_utf8_lossy(&data)) } else { String::new() });
        if let Ok(dir) = std::env::var("F2FS_EXPECT") {
            if let Ok(want) = std::fs::read(format!("{dir}/{name}")) {
                assert_eq!(data, want, "{name} differs from the dump");
                println!("  = the dump");
            }
        }
    }
}
