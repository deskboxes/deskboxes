//! The files the installer carries, appended to its own exe.
//!
//! ```text
//! setup exe = [the program][payload][footer]
//! payload   = u32 LE header length, header (JSON), then the compressed chunks back to back
//! header    = { app_version, files: [{ path, size }], chunks: [{ clen, ulen, crc }] }
//! footer    = "DBXSETUP" | u32 format | u32 0 | u64 payload offset | u64 payload length  (32 bytes)
//! ```
//!
//! The files' bytes are concatenated in header order and cut into chunks of 8 MB, each compressed
//! with Windows' own LZMS compressor (cabinet.dll, `Compress`/`Decompress` in buffer mode), so
//! nothing beyond Windows is needed and extraction can report progress chunk by chunk.
//!
//! The uninstaller is this program without the payload: `program_bytes()` cuts it off.

use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use windows::Win32::Storage::Compression::*;

const MAGIC: &[u8; 8] = b"DBXSETUP";
const FORMAT: u32 = 1;
const FOOTER: usize = 32;
const CHUNK: usize = 8 << 20;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileEntry {
    /// Relative path with backslashes, e.g. `en-us\Microsoft.ui.xaml.dll.mui`.
    pub path: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Chunk {
    pub clen: u64,
    pub ulen: u64,
    pub crc: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Header {
    pub app_version: String,
    pub files: Vec<FileEntry>,
    pub chunks: Vec<Chunk>,
}

impl Header {
    pub fn total(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// The payload of `exe`, if it has one: its header and where the chunk data starts.
pub struct Payload {
    pub exe: PathBuf,
    pub header: Header,
    /// Offset of the first chunk in the exe.
    data_at: u64,
}

fn footer_of(f: &mut std::fs::File) -> std::io::Result<Option<(u64, u64)>> {
    let len = f.metadata()?.len();
    if len < FOOTER as u64 {
        return Ok(None);
    }
    f.seek(SeekFrom::Start(len - FOOTER as u64))?;
    let mut b = [0u8; FOOTER];
    f.read_exact(&mut b)?;
    if &b[0..8] != MAGIC || u32::from_le_bytes(b[8..12].try_into().unwrap()) != FORMAT {
        return Ok(None);
    }
    let off = u64::from_le_bytes(b[16..24].try_into().unwrap());
    let plen = u64::from_le_bytes(b[24..32].try_into().unwrap());
    if off + plen + FOOTER as u64 != len {
        return Ok(None);
    }
    Ok(Some((off, plen)))
}

impl Payload {
    pub fn open(exe: &Path) -> std::io::Result<Option<Payload>> {
        let mut f = std::fs::File::open(exe)?;
        let Some((off, _)) = footer_of(&mut f)? else { return Ok(None) };
        f.seek(SeekFrom::Start(off))?;
        let mut n = [0u8; 4];
        f.read_exact(&mut n)?;
        let n = u32::from_le_bytes(n) as usize;
        let mut h = vec![0u8; n];
        f.read_exact(&mut h)?;
        let header: Header = serde_json::from_slice(&h).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Some(Payload { exe: exe.to_path_buf(), header, data_at: off + 4 + n as u64 }))
    }

    /// Write every file under `dir`. `progress(bytes written so far)` is called after each file.
    /// `write_file` creates one file from its bytes (the caller deals with files in use).
    pub fn extract(&self, dir: &Path, mut write_file: impl FnMut(&Path, &[u8]) -> std::io::Result<()>, mut progress: impl FnMut(u64)) -> std::io::Result<()> {
        let damaged = |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("the setup file is damaged ({what}); download it again"));
        let mut f = std::fs::File::open(&self.exe)?;
        f.seek(SeekFrom::Start(self.data_at))?;
        let dec = Decompressor::new()?;
        let files = &self.header.files;
        let mut ix = 0usize; // the file being filled
        let mut buf: Vec<u8> = Vec::new();
        let mut done = 0u64;
        let mut put = |e: &FileEntry, data: &[u8], done: &mut u64| -> std::io::Result<()> {
            let p = dir.join(&e.path);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)?;
            }
            write_file(&p, data)?;
            *done += e.size;
            progress(*done);
            Ok(())
        };
        // Empty files have no bytes in any chunk.
        while ix < files.len() && files[ix].size == 0 {
            put(&files[ix], &[], &mut done)?;
            ix += 1;
        }
        for c in &self.header.chunks {
            let mut comp = vec![0u8; c.clen as usize];
            f.read_exact(&mut comp)?;
            let raw = dec.decompress(&comp, c.ulen as usize)?;
            drop(comp);
            if crc32(&raw) != c.crc {
                return Err(damaged("checksum mismatch"));
            }
            let mut at = 0usize;
            while at < raw.len() {
                let Some(e) = files.get(ix) else { return Err(damaged("extra data")) };
                let take = (e.size as usize - buf.len()).min(raw.len() - at);
                if buf.is_empty() && take == e.size as usize {
                    // The whole file is inside this chunk: no copy.
                    put(e, &raw[at..at + take], &mut done)?;
                } else {
                    buf.extend_from_slice(&raw[at..at + take]);
                    if buf.len() == e.size as usize {
                        put(e, &buf, &mut done)?;
                        buf = Vec::new();
                    } else {
                        at += take;
                        continue;
                    }
                }
                at += take;
                ix += 1;
                while ix < files.len() && files[ix].size == 0 {
                    put(&files[ix], &[], &mut done)?;
                    ix += 1;
                }
            }
        }
        if ix != files.len() {
            return Err(damaged("data missing"));
        }
        Ok(())
    }
}

/// The running exe without any payload (the uninstaller copy).
pub fn program_bytes(exe: &Path) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(exe)?;
    let len = match footer_of(&mut f)? {
        Some((off, _)) => off,
        None => f.metadata()?.len(),
    };
    f.seek(SeekFrom::Start(0))?;
    let mut v = vec![0u8; len as usize];
    f.read_exact(&mut v)?;
    Ok(v)
}

/// `deskboxes-setup --pack <dir> <out.exe>`: this program + every file under `dir`.
pub fn pack(dir: &Path, out: &Path, app_version: &str) -> std::io::Result<(usize, u64, u64)> {
    let mut paths = Vec::new();
    walk(dir, dir, &mut paths)?;
    paths.sort_by_key(|p| p.to_ascii_lowercase());
    let mut files = Vec::new();
    let mut stream = Vec::new();
    for rel in &paths {
        let data = std::fs::read(dir.join(rel))?;
        files.push(FileEntry { path: rel.clone(), size: data.len() as u64 });
        stream.extend_from_slice(&data);
    }
    // Compress the chunks in parallel (LZMS is slow to compress, fast to decompress).
    let parts: Vec<&[u8]> = stream.chunks(CHUNK).collect();
    let compressed: Vec<std::io::Result<Vec<u8>>> = std::thread::scope(|s| {
        let hs: Vec<_> = parts.iter().map(|p| s.spawn(move || Compressor::new()?.compress(p))).collect();
        hs.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(std::io::Error::other("compressor thread panicked")))).collect()
    });
    let mut chunks = Vec::new();
    let mut data = Vec::new();
    for (p, c) in parts.iter().zip(compressed) {
        let c = c?;
        chunks.push(Chunk { clen: c.len() as u64, ulen: p.len() as u64, crc: crc32(p) });
        data.extend_from_slice(&c);
    }
    let header = Header { app_version: app_version.to_string(), files, chunks };
    let h = serde_json::to_vec(&header).map_err(std::io::Error::other)?;
    let me = std::env::current_exe()?;
    let program = program_bytes(&me)?;
    let mut o = std::io::BufWriter::new(std::fs::File::create(out)?);
    o.write_all(&program)?;
    let off = program.len() as u64;
    o.write_all(&(h.len() as u32).to_le_bytes())?;
    o.write_all(&h)?;
    o.write_all(&data)?;
    let plen = 4 + h.len() as u64 + data.len() as u64;
    o.write_all(MAGIC)?;
    o.write_all(&FORMAT.to_le_bytes())?;
    o.write_all(&0u32.to_le_bytes())?;
    o.write_all(&off.to_le_bytes())?;
    o.write_all(&plen.to_le_bytes())?;
    o.flush()?;
    Ok((header.files.len(), header.total(), plen))
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        if e.file_type()?.is_dir() {
            walk(root, &p, out)?;
        } else {
            out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('/', "\\"));
        }
    }
    Ok(())
}

struct Compressor(COMPRESSOR_HANDLE);
struct Decompressor(DECOMPRESSOR_HANDLE);

impl Compressor {
    fn new() -> std::io::Result<Self> {
        let mut h = COMPRESSOR_HANDLE::default();
        unsafe { CreateCompressor(COMPRESS_ALGORITHM_LZMS, None, &mut h) }.map_err(std::io::Error::other)?;
        Ok(Compressor(h))
    }
    fn compress(&self, data: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut need = 0usize;
        // First call: the size needed (fails with ERROR_INSUFFICIENT_BUFFER).
        let _ = unsafe { Compress(self.0, Some(data.as_ptr() as _), data.len(), None, 0, &mut need) };
        let mut out = vec![0u8; need.max(64)];
        let mut n = 0usize;
        unsafe { Compress(self.0, Some(data.as_ptr() as _), data.len(), Some(out.as_mut_ptr() as _), out.len(), &mut n) }.map_err(std::io::Error::other)?;
        out.truncate(n);
        Ok(out)
    }
}

impl Drop for Compressor {
    fn drop(&mut self) {
        let _ = unsafe { CloseCompressor(self.0) };
    }
}

impl Decompressor {
    fn new() -> std::io::Result<Self> {
        let mut h = DECOMPRESSOR_HANDLE::default();
        unsafe { CreateDecompressor(COMPRESS_ALGORITHM_LZMS, None, &mut h) }.map_err(std::io::Error::other)?;
        Ok(Decompressor(h))
    }
    fn decompress(&self, data: &[u8], ulen: usize) -> std::io::Result<Vec<u8>> {
        let mut out = vec![0u8; ulen];
        let mut n = 0usize;
        unsafe { Decompress(self.0, Some(data.as_ptr() as _), data.len(), Some(out.as_mut_ptr() as _), out.len(), Some(&mut n)) }.map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if n != ulen {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "the setup file is damaged (wrong size)"));
        }
        Ok(out)
    }
}

impl Drop for Decompressor {
    fn drop(&mut self) {
        let _ = unsafe { CloseDecompressor(self.0) };
    }
}

pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *v = c;
        }
        t
    });
    let mut c = !0u32;
    for &b in data {
        c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}
