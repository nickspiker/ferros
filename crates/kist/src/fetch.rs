//! Fetching a kist's capsule (payloads/kists, the stower; docs/KISTS.md "The stower"): the pure half, host-tested.
//!
//! - **The URL**: `http://host[:port]/path`, plain HTTP only (fgtw.org's kists.fgtw.org answers port 80 with no redirect; ferros speaks no TLS yet).
//! - **The response** decoded as it arrives (`Response`): the status line and the headers, then the body by its length, in chunks, or up to the close. Anything but 200 is the capsule missing there, and nothing of the body is read.
//! - **The capsule** taken in as it streams into the vault (`Intake`): its blake3 (the address the person's list names it by) and the file hash the signature covers (blake3 with the header's signature slot read as zeros, as vsf's `header_eggs` computes it) both kept as the bytes pass, and its header read from its first bytes, so a capsule of 106 MB is never held whole. Its eggs are checked against the developer's key bundle by the caller (fgtw's `pq::verify_eggs`, which this crate does not carry: the kernel builds it).

use alloc::vec::Vec;
use vsf::decoding::parse;
use vsf::eggs::Egg;
use vsf::VsfType;

/// The most header bytes a response may have before its body (WHY: a server whose headers run past this is not one this client speaks to; a few hundred bytes is usual).
pub const HEADERS_MAX: usize = 1 << 13;

/// A capsule's first bytes kept to read its header and declaration from: the header (with its eggs, 8.6 KB for a release key's three), the `kist` and `wants` sections, and the start of its code (the kernel's `kist_need` reads the same span; ferros_kist's own test holds the code's start under it).
pub const HEAD: usize = 1 << 16;

/// `http://host[:port]/path` split: (host, port, path). None for any other scheme, an empty host, a port that is not a number, or anything but printable ASCII.
pub fn split_url(url: &str) -> Option<(&str, u16, &str)> {
    if !url.bytes().all(|b| (0x21..0x7F).contains(&b)) {
        return None;
    }
    let rest = url.strip_prefix("http://")?;
    let slash = rest.find('/').unwrap_or(rest.len());
    let (authority, path) = rest.split_at(slash);
    let path = if path.is_empty() { "/" } else { path };
    let (host, port) = match authority.rfind(':') {
        Some(c) => (&authority[..c], authority[c + 1..].parse::<u16>().ok()?),
        None => (authority, 80),
    };
    if host.is_empty() || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-') {
        return None;
    }
    Some((host, port, path))
}

/// How a response's body ends.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Body {
    /// `Content-Length`: this many bytes.
    Length(u64),
    /// `Transfer-Encoding: chunked`.
    Chunked,
    /// Neither: the body is everything until the server closes.
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Chunk {
    /// Reading a chunk's size line (hex digits, an extension after `;` ignored), its value so far, and whether the digits ended.
    Size(u64, bool),
    /// Inside a chunk's data, this many bytes left.
    Data(u64),
    /// The CRLF after a chunk's data: how many of its two bytes have come.
    DataEnd(u8),
    /// After the last chunk: the trailer's lines up to the empty one, the bytes of the current line.
    Trailer(u32),
}

/// A plain HTTP/1.1 response decoded as its bytes arrive.
pub struct Response {
    head: Vec<u8>,
    /// The status code, once the headers are in.
    pub status: u16,
    /// How the body ends, once the headers are in.
    pub body: Option<Body>,
    /// Body bytes passed on so far.
    pub got: u64,
    chunk: Chunk,
    /// The whole body has come.
    pub done: bool,
}

impl Default for Response {
    fn default() -> Self {
        Self::new()
    }
}

impl Response {
    pub fn new() -> Response {
        Response { head: Vec::new(), status: 0, body: None, got: 0, chunk: Chunk::Size(0, false), done: false }
    }

    /// The bytes that came, the body's passed to `out` as they are found. Err for headers that do not parse or run past `HEADERS_MAX`, a status other than 200 (`status` says which; no byte of the body is passed on), chunks that do not frame, more bytes than the length said, or an error from `out`.
    pub fn feed(&mut self, mut input: &[u8], out: &mut impl FnMut(&[u8]) -> Result<(), &'static str>) -> Result<(), &'static str> {
        if self.body.is_none() {
            // The headers end at the first empty line: each byte kept until the last four are CR LF CR LF.
            let mut ended = None;
            for (k, &b) in input.iter().enumerate() {
                if self.head.len() >= HEADERS_MAX {
                    return Err("the response's headers run on");
                }
                self.head.push(b);
                if self.head.ends_with(b"\r\n\r\n") {
                    ended = Some(k + 1);
                    break;
                }
            }
            let Some(at) = ended else { return Ok(()) };
            self.headers()?;
            input = &input[at..];
        }
        if self.status != 200 {
            return Err("the server does not have it");
        }
        while !input.is_empty() {
            if self.done {
                return Err("bytes after the response's end");
            }
            match (self.body.unwrap(), self.chunk) {
                (Body::Length(n), _) => {
                    // WHY: the length is the server's; more bytes than it said is a broken response, never part of the capsule.
                    if input.len() as u64 > n - self.got {
                        return Err("more bytes than the response's length");
                    }
                    self.got += input.len() as u64;
                    out(input)?;
                    self.done = self.got == n;
                    input = &[];
                }
                (Body::Close, _) => {
                    self.got += input.len() as u64;
                    out(input)?;
                    input = &[];
                }
                (Body::Chunked, Chunk::Size(v, ended)) => {
                    let b = input[0];
                    input = &input[1..];
                    self.chunk = match b {
                        b'\n' if v == 0 => Chunk::Trailer(0),
                        b'\n' => Chunk::Data(v),
                        b'\r' | b';' => Chunk::Size(v, true),
                        _ if ended => Chunk::Size(v, true),
                        _ => {
                            let d = (b as char).to_digit(16).ok_or("a chunk size that is not hex")? as u64;
                            // WHY: a size past 2^60 is no chunk a server sends; the shift would lose its top digits.
                            if v >> 60 != 0 {
                                return Err("a chunk size too large");
                            }
                            Chunk::Size(v << 4 | d, false)
                        }
                    };
                }
                (Body::Chunked, Chunk::Data(left)) => {
                    // WHY: the chunk's size is the server's; what is left of it may be less than what came.
                    let n = (left as usize).min(input.len());
                    self.got += n as u64;
                    out(&input[..n])?;
                    input = &input[n..];
                    self.chunk = if left - n as u64 == 0 { Chunk::DataEnd(0) } else { Chunk::Data(left - n as u64) };
                }
                (Body::Chunked, Chunk::DataEnd(k)) => {
                    let b = input[0];
                    input = &input[1..];
                    self.chunk = match (k, b) {
                        (0, b'\r') => Chunk::DataEnd(1),
                        (0, b'\n') | (1, b'\n') => Chunk::Size(0, false),
                        _ => return Err("a chunk not followed by its line end"),
                    };
                }
                (Body::Chunked, Chunk::Trailer(line)) => {
                    let b = input[0];
                    input = &input[1..];
                    self.chunk = match b {
                        b'\n' if line == 0 => {
                            self.done = true;
                            Chunk::Trailer(0)
                        }
                        b'\n' => Chunk::Trailer(0),
                        b'\r' => Chunk::Trailer(line),
                        _ => Chunk::Trailer(line + 1),
                    };
                }
            }
        }
        Ok(())
    }

    /// The server closed the connection: Ok when the body is whole (or ends at the close), else Err.
    pub fn closed(&mut self) -> Result<(), &'static str> {
        match self.body {
            Some(Body::Close) if self.status == 200 => {
                self.done = true;
                Ok(())
            }
            _ if self.done => Ok(()),
            None => Err("the connection closed before the response's headers"),
            _ => Err("the connection closed before the response's end"),
        }
    }

    /// The status line and the two headers the body's end depends on.
    fn headers(&mut self) -> Result<(), &'static str> {
        let text = core::str::from_utf8(&self.head).map_err(|_| "headers that are not text")?;
        let mut lines = text.split("\r\n");
        let status = lines.next().ok_or("no status line")?;
        let mut words = status.split(' ');
        if !words.next().is_some_and(|v| v.starts_with("HTTP/1.")) {
            return Err("not an HTTP/1 response");
        }
        self.status = words.next().and_then(|c| c.parse::<u16>().ok()).ok_or("a status that is not a number")?;
        let (mut length, mut chunked) = (None, false);
        for l in lines {
            let Some((name, value)) = l.split_once(':') else { continue };
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                length = Some(value.parse::<u64>().map_err(|_| "a length that is not a number")?);
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                chunked = value.split(',').any(|v| v.trim().eq_ignore_ascii_case("chunked"));
            }
        }
        // Chunked wins over a length (RFC 9112 6.3).
        self.body = Some(if chunked {
            Body::Chunked
        } else {
            match length {
                Some(n) => Body::Length(n),
                None => Body::Close,
            }
        });
        if self.body == Some(Body::Length(0)) {
            self.done = true;
        }
        Ok(())
    }
}

/// What a capsule's header says of its signature: the key in its header (`ke`), its eggs (`gm`, or a single Ed25519 `ge`), and where the signature's value lies in the file (start, length), the span its file hash reads as zeros.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub signer: [u8; 32],
    pub eggs: Vec<Egg>,
    pub slot: (usize, usize),
}

/// The signature of a capsule from its first bytes, walking the header as vsf's `header_eggs` does (magic, version, compatibility, header length, the optional file length and creation time, the provenance hash, then the key and the signature): Err when the bytes are not a signed VSF file or end before the signature does.
pub fn head(bytes: &[u8]) -> Result<Head, &'static str> {
    if bytes.len() < 4 || &bytes[0..3] != "RÅ".as_bytes() || bytes[3] != b'<' {
        return Err("not a VSF file");
    }
    let mut p = 4;
    let next = |p: &mut usize| parse(bytes, p).map_err(|_| "a header that does not parse");
    for _ in 0..3 {
        next(&mut p)?;
    }
    for optional in [b'l', b'e'] {
        if bytes.get(p) == Some(&optional) {
            next(&mut p)?;
        }
    }
    if !matches!(next(&mut p)?, VsfType::hp(_)) {
        return Err("a header without its provenance hash");
    }
    if bytes.get(p) != Some(&b'k') {
        return Err("not signed: no key in its header");
    }
    let signer: [u8; 32] = match next(&mut p)? {
        VsfType::ke(k) => k.as_slice().try_into().map_err(|_| "a key that is not Ed25519")?,
        _ => return Err("not signed: no Ed25519 key in its header"),
    };
    if bytes.get(p) != Some(&b'g') {
        return Err("not signed: no signature in its header");
    }
    let (eggs, len) = match next(&mut p)? {
        VsfType::gm(b) => (vsf::eggs::eggs_from_bytes(&b)?, b.len()),
        VsfType::ge(b) => (alloc::vec![Egg { scheme: vsf::eggs::SCHEME_ED25519, sig: b.clone() }], b.len()),
        _ => return Err("not signed: no signature in its header"),
    };
    if eggs.iter().all(|e| e.sig.iter().all(|&b| b == 0)) {
        return Err("not signed: its signature is all zeros");
    }
    Ok(Head { signer, eggs, slot: (p - len, len) })
}

/// A capsule taken in as it streams: its first `HEAD` bytes kept, its address and its file hash kept as the bytes pass.
pub struct Intake {
    first: Vec<u8>,
    head: Option<Head>,
    address: blake3::Hasher,
    file: blake3::Hasher,
    len: u64,
}

/// A capsule taken in whole: its address (blake3), the hash its signature covers, its length, its signature, and its first bytes (its declaration and identity read from them: `declaration`, `identity`).
pub struct Taken {
    pub address: [u8; 32],
    pub file_hash: [u8; 32],
    pub len: u64,
    pub head: Head,
    pub first: Vec<u8>,
}

impl Default for Intake {
    fn default() -> Self {
        Self::new()
    }
}

impl Intake {
    pub fn new() -> Intake {
        Intake { first: Vec::with_capacity(HEAD), head: None, address: blake3::Hasher::new(), file: blake3::Hasher::new(), len: 0 }
    }

    /// The capsule's next bytes. Err as soon as its first bytes show it is not a signed capsule (the caller stops the download).
    pub fn update(&mut self, bytes: &[u8]) -> Result<(), &'static str> {
        self.address.update(bytes);
        self.len += bytes.len() as u64;
        if self.head.is_some() {
            self.file.update(bytes);
            return Ok(());
        }
        // WHY: only the first HEAD bytes are kept; the rest of this piece goes to the hash once the head is read.
        let n = (HEAD - self.first.len()).min(bytes.len());
        self.first.extend_from_slice(&bytes[..n]);
        if self.first.len() == HEAD {
            self.read_head()?;
            self.file.update(&bytes[n..]);
        }
        Ok(())
    }

    /// The first bytes held so far.
    pub fn first(&self) -> &[u8] {
        &self.first
    }

    /// The bytes taken so far.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The header read from the first bytes, and they hashed with the signature's value read as zeros.
    fn read_head(&mut self) -> Result<(), &'static str> {
        let h = head(&self.first)?;
        let (start, len) = h.slot;
        const ZEROS: [u8; 64] = [0; 64];
        self.file.update(&self.first[..start]);
        let mut left = len;
        while left > 0 {
            // WHY: a run of up to 64 zeros at a time; the slot is longer than the run (three eggs are 8.6 KB).
            let n = left.min(ZEROS.len());
            self.file.update(&ZEROS[..n]);
            left -= n;
        }
        self.file.update(&self.first[start + len..]);
        self.head = Some(h);
        Ok(())
    }

    /// Every byte has come: the capsule's address, file hash and header.
    pub fn finish(mut self) -> Result<Taken, &'static str> {
        if self.head.is_none() {
            self.read_head()?;
        }
        Ok(Taken { address: *self.address.finalize().as_bytes(), file_hash: *self.file.finalize().as_bytes(), len: self.len, head: self.head.unwrap(), first: self.first })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::string::String;
    use alloc::vec;
    use fgtw::pq::FleetSigner;

    #[test]
    fn urls_split() {
        assert_eq!(split_url("http://kists.fgtw.org/kists.vsf"), Some(("kists.fgtw.org", 80, "/kists.vsf")));
        assert_eq!(split_url("http://a.b:8080"), Some(("a.b", 8080, "/")));
        assert_eq!(split_url("https://kists.fgtw.org/x"), None);
        assert_eq!(split_url("http:///x"), None);
        assert_eq!(split_url("http://a b/x"), None);
        assert_eq!(split_url("http://a:x/"), None);
    }

    /// `bytes` fed in pieces of the sizes given in turn, the body gathered.
    fn run(resp: &[u8], sizes: &[usize]) -> (Result<(), &'static str>, Response, Vec<u8>) {
        let mut r = Response::new();
        let mut body = Vec::new();
        let mut at = 0;
        let mut k = 0;
        while at < resp.len() {
            let n = sizes[k % sizes.len()].min(resp.len() - at);
            k += 1;
            if let Err(e) = r.feed(&resp[at..at + n], &mut |b| {
                body.extend_from_slice(b);
                Ok(())
            }) {
                return (Err(e), r, body);
            }
            at += n;
        }
        let end = if r.done { Ok(()) } else { r.closed() };
        (end, r, body)
    }

    #[test]
    fn a_length_body_in_any_pieces() {
        let resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\ncontent-length: 11\r\nConnection: close\r\n\r\nhello world";
        for sizes in [&[1usize][..], &[3, 7], &[200]] {
            let (end, r, body) = run(resp, sizes);
            assert_eq!(end, Ok(()));
            assert_eq!(r.body, Some(Body::Length(11)));
            assert_eq!(body, b"hello world");
        }
        // Past the length is refused.
        let (end, _, _) = run(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nabc", &[64]);
        assert!(end.is_err());
        // Short of it, the close is an error.
        let (end, _, _) = run(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabc", &[64]);
        assert!(end.is_err());
    }

    #[test]
    fn a_chunked_body_and_a_closed_one() {
        let resp = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;x=y\r\nhello\r\n6\r\n world\r\n0\r\nX-Trailer: 1\r\n\r\n";
        for sizes in [&[1usize][..], &[2, 5, 11], &[512]] {
            let (end, r, body) = run(resp, sizes);
            assert_eq!(end, Ok(()));
            assert!(r.done);
            assert_eq!(body, b"hello world");
        }
        let (end, _, body) = run(b"HTTP/1.0 200 OK\r\n\r\nall of it", &[4]);
        assert_eq!(end, Ok(()));
        assert_eq!(body, b"all of it");
    }

    #[test]
    fn anything_but_200_is_missing_and_its_body_is_not_read() {
        let (end, r, body) = run(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\n\r\nnot found", &[7]);
        assert!(end.is_err());
        assert_eq!(r.status, 404);
        assert!(body.is_empty());
        let (end, _, _) = run(b"SSH-2.0\r\n\r\n", &[64]);
        assert!(end.is_err());
    }

    /// A capsule signed with every egg of a release-style bundle (a test key), its code `code` bytes long.
    fn signed(code: usize, key: &fgtw::pq::SigningBundle) -> Vec<u8> {
        let d = crate::Decl { name: String::from("Test"), wants: vec![(0, String::from("to draw"))] };
        let (k, w) = crate::declaration_sections(&d, Some([5; 32]));
        let mut c = vsf::file_format::VsfSection::new("aarch64");
        c.add_field("code", VsfType::v(b'b', (0..code).map(|i| (i * 7 + i / 251) as u8).collect()));
        let ed = key.keypair().public.to_bytes();
        let unsigned = vsf::VsfBuilder::new()
            .creation_time_oscillations(1)
            .add_section_direct(k)
            .add_section_direct(w.unwrap())
            .add_section_direct(c)
            .signed_only_eggs(VsfType::ke(ed.to_vec()), &fgtw::release::release_slots())
            .build()
            .unwrap();
        fgtw::release::sign_release(unsigned, key).unwrap()
    }

    #[test]
    fn a_capsule_streams_to_the_hashes_vsf_and_fgtw_check() {
        let key = fgtw::pq::SigningBundle::release(&[7u8; 32]);
        let ed = key.keypair().public.to_bytes();
        let cap = signed(200 << 10, &key);
        assert!(fgtw::release::verify_signed_by(&cap, &ed, &key.public()).is_ok());
        let (signer, eggs, file_hash) = vsf::verification::header_eggs(&cap).unwrap();
        for piece in [1usize << 12, 1 << 16, (1 << 16) + 3, 1 << 20] {
            let mut i = Intake::new();
            for c in cap.chunks(piece) {
                i.update(c).unwrap();
            }
            let t = i.finish().unwrap();
            assert_eq!(t.address, *blake3::hash(&cap).as_bytes());
            assert_eq!(t.file_hash, file_hash);
            assert_eq!(t.len, cap.len() as u64);
            assert_eq!(t.head.signer, signer);
            assert_eq!(t.head.eggs, eggs);
            assert!(fgtw::pq::verify_eggs(&t.head.eggs, &key.public(), &t.file_hash, fgtw::release::RELEASE_MASK));
            // The declaration and identity read from the first bytes alone.
            assert_eq!(crate::declaration(&t.first).unwrap().name, "Test");
            assert_eq!(crate::identity(&t.first).unwrap(), (ed, [5; 32]));
        }
        // A byte of its code changed: the address and the file hash move, and the eggs no longer verify.
        let mut bad = cap.clone();
        let at = bad.len() - 100;
        bad[at] ^= 1;
        let mut i = Intake::new();
        i.update(&bad).unwrap();
        let t = i.finish().unwrap();
        assert_ne!(t.address, *blake3::hash(&cap).as_bytes());
        assert!(!fgtw::pq::verify_eggs(&t.head.eggs, &key.public(), &t.file_hash, fgtw::release::RELEASE_MASK));
        // A capsule shorter than HEAD is read whole at the end.
        let small = signed(1 << 10, &key);
        let mut i = Intake::new();
        i.update(&small).unwrap();
        let t = i.finish().unwrap();
        assert_eq!(t.file_hash, vsf::verification::header_eggs(&small).unwrap().2);
        // Bytes that are not a signed capsule are refused once their head is in.
        let mut i = Intake::new();
        assert!(i.update(&vec![0u8; HEAD]).is_err());
    }
}
