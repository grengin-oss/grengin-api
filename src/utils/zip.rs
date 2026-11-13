// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use flate2::read::DeflateDecoder;
use std::io::{self, Read};

const LOCAL_HEADER_SIG: u32 = 0x04034B50;
const CENTRAL_DIR_SIG: u32 = 0x02014B50;
const EOCD_SIG: u32 = 0x06054B50;

const METHOD_STORED: u16 = 0;
const METHOD_DEFLATED: u16 = 8;

#[derive(Debug)]
pub enum ZipError {
    InvalidFormat(String),
    Io(io::Error),
    UnsupportedCompression(u16),
    EntryTooLarge(String),
}

impl std::fmt::Display for ZipError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ZipError::InvalidFormat(s) => write!(f, "invalid ZIP: {s}"),
            ZipError::Io(e) => write!(f, "I/O error: {e}"),
            ZipError::UnsupportedCompression(m) => write!(f, "unsupported compression method {m}"),
            ZipError::EntryTooLarge(name) => {
                write!(f, "{name} is larger than {MAX_ENTRY_BYTES} bytes")
            }
        }
    }
}

impl From<io::Error> for ZipError {
    fn from(e: io::Error) -> Self {
        ZipError::Io(e)
    }
}

pub const MAX_ENTRY_BYTES: u64 = 4 * 1024 * 1024;

pub struct ZipEntry<'a> {
    name: String,
    is_dir: bool,
    compression: u16,
    compressed: &'a [u8],
}

impl ZipEntry<'_> {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    pub fn read_to_string(&self, buf: &mut String) -> Result<usize, ZipError> {
        let data = self.decompress()?;
        let s = std::str::from_utf8(&data)
            .map_err(|e| ZipError::Io(io::Error::new(io::ErrorKind::InvalidData, e)))?;
        buf.push_str(s);
        Ok(s.len())
    }

    fn decompress(&self) -> Result<Vec<u8>, ZipError> {
        let mut out = Vec::new();
        match self.compression {
            METHOD_STORED => {
                self.compressed
                    .take(MAX_ENTRY_BYTES + 1)
                    .read_to_end(&mut out)?;
            }
            METHOD_DEFLATED => {
                DeflateDecoder::new(self.compressed)
                    .take(MAX_ENTRY_BYTES + 1)
                    .read_to_end(&mut out)?;
            }
            m => return Err(ZipError::UnsupportedCompression(m)),
        }
        if out.len() as u64 > MAX_ENTRY_BYTES {
            return Err(ZipError::EntryTooLarge(self.name.clone()));
        }
        Ok(out)
    }
}

pub struct ZipArchive<'a> {
    entries: Vec<ZipEntry<'a>>,
}

impl<'a> ZipArchive<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, ZipError> {
        let eocd = find_eocd(data)?;
        let total = eocd.total_entries as usize;
        let mut entries = Vec::with_capacity(total);
        let mut pos = eocd.cd_offset as usize;

        for _ in 0..total {
            if pos + 46 > data.len() {
                return Err(ZipError::InvalidFormat(
                    "central directory entry truncated".into(),
                ));
            }

            let sig = u32_le(&data[pos..]);
            if sig != CENTRAL_DIR_SIG {
                return Err(ZipError::InvalidFormat(format!(
                    "expected central dir signature, got 0x{sig:08X}"
                )));
            }

            let compression = u16_le(&data[pos + 10..]);
            let compressed_sz = u32_le(&data[pos + 20..]) as usize;
            let filename_len = u16_le(&data[pos + 28..]) as usize;
            let extra_len = u16_le(&data[pos + 30..]) as usize;
            let comment_len = u16_le(&data[pos + 32..]) as usize;
            let lh_offset = u32_le(&data[pos + 42..]) as usize;

            pos += 46;
            if pos + filename_len > data.len() {
                return Err(ZipError::InvalidFormat("filename truncated".into()));
            }
            let name = String::from_utf8_lossy(&data[pos..pos + filename_len]).into_owned();
            pos += filename_len + extra_len + comment_len;

            let is_dir = name.ends_with('/');

            if lh_offset + 30 > data.len() {
                return Err(ZipError::InvalidFormat("local header out of bounds".into()));
            }
            if u32_le(&data[lh_offset..]) != LOCAL_HEADER_SIG {
                return Err(ZipError::InvalidFormat(
                    "local header signature mismatch".into(),
                ));
            }
            let lh_name_len = u16_le(&data[lh_offset + 26..]) as usize;
            let lh_extra_len = u16_le(&data[lh_offset + 28..]) as usize;
            let data_start = lh_offset + 30 + lh_name_len + lh_extra_len;

            if data_start + compressed_sz > data.len() {
                return Err(ZipError::InvalidFormat("file data out of bounds".into()));
            }

            entries.push(ZipEntry {
                name,
                is_dir,
                compression,
                compressed: &data[data_start..data_start + compressed_sz],
            });
        }

        Ok(Self { entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn by_index(&self, i: usize) -> Result<&ZipEntry<'a>, ZipError> {
        self.entries
            .get(i)
            .ok_or_else(|| ZipError::InvalidFormat(format!("entry index {i} out of range")))
    }
}

struct Eocd {
    total_entries: u16,
    cd_offset: u32,
}

fn find_eocd(data: &[u8]) -> Result<Eocd, ZipError> {
    if data.len() < 22 {
        return Err(ZipError::InvalidFormat(
            "data too small to contain EOCD".into(),
        ));
    }
    let min_start = data.len().saturating_sub(65535 + 22);
    for i in (min_start..=data.len() - 22).rev() {
        if u32_le(&data[i..]) == EOCD_SIG {
            return Ok(Eocd {
                total_entries: u16_le(&data[i + 10..]),
                cd_offset: u32_le(&data[i + 16..]),
            });
        }
    }
    Err(ZipError::InvalidFormat("EOCD record not found".into()))
}

#[inline]
fn u16_le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

#[inline]
fn u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal stored-compression ZIP from a list of (name, data) pairs.
    fn make_stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut local: Vec<u8> = Vec::new();
        let mut central: Vec<u8> = Vec::new();
        let mut offsets: Vec<u32> = Vec::new();

        for (name, data) in entries {
            let nb = name.as_bytes();
            offsets.push(local.len() as u32);
            local.extend_from_slice(&0x04034B50u32.to_le_bytes());
            local.extend_from_slice(&20u16.to_le_bytes()); // version needed
            local.extend_from_slice(&0u16.to_le_bytes()); // flags
            local.extend_from_slice(&0u16.to_le_bytes()); // compression: stored
            local.extend_from_slice(&0u32.to_le_bytes()); // mod time+date
            local.extend_from_slice(&0u32.to_le_bytes()); // CRC-32
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(nb.len() as u16).to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes()); // extra len
            local.extend_from_slice(nb);
            local.extend_from_slice(data);
        }

        let cd_offset = local.len() as u32;

        for ((name, data), &lh_off) in entries.iter().zip(offsets.iter()) {
            let nb = name.as_bytes();
            central.extend_from_slice(&0x02014B50u32.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&0u16.to_le_bytes()); // compression
            central.extend_from_slice(&0u32.to_le_bytes()); // mod time+date
            central.extend_from_slice(&0u32.to_le_bytes()); // CRC-32
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(nb.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra len
            central.extend_from_slice(&0u16.to_le_bytes()); // comment len
            central.extend_from_slice(&0u16.to_le_bytes()); // disk start
            central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            central.extend_from_slice(&lh_off.to_le_bytes());
            central.extend_from_slice(nb);
        }

        let cd_size = central.len() as u32;
        let n = entries.len() as u16;

        let mut eocd: Vec<u8> = Vec::new();
        eocd.extend_from_slice(&0x06054B50u32.to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes()); // disk number
        eocd.extend_from_slice(&0u16.to_le_bytes()); // cd disk
        eocd.extend_from_slice(&n.to_le_bytes()); // entries on disk
        eocd.extend_from_slice(&n.to_le_bytes()); // total entries
        eocd.extend_from_slice(&cd_size.to_le_bytes());
        eocd.extend_from_slice(&cd_offset.to_le_bytes());
        eocd.extend_from_slice(&0u16.to_le_bytes()); // comment len

        let mut out = local;
        out.extend_from_slice(&central);
        out.extend_from_slice(&eocd);
        out
    }

    #[test]
    fn reads_stored_entries() {
        let zip = make_stored_zip(&[
            ("readme.md", b"# Hello\nWorld"),
            ("dir/", b""),
            ("other.txt", b"ignored"),
        ]);
        let archive = ZipArchive::new(&zip).unwrap();
        assert_eq!(archive.len(), 3);

        let entry = archive.by_index(0).unwrap();
        assert_eq!(entry.name(), "readme.md");
        assert!(!entry.is_dir());
        let mut buf = String::new();
        entry.read_to_string(&mut buf).unwrap();
        assert_eq!(buf, "# Hello\nWorld");

        assert!(archive.by_index(1).unwrap().is_dir());
    }

    fn make_deflated_bomb(name: &str, inflated_len: usize) -> Vec<u8> {
        use flate2::{Compression, write::DeflateEncoder};
        use std::io::Write;
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&vec![b'a'; inflated_len]).unwrap();
        let deflated = encoder.finish().unwrap();
        let mut zip = make_stored_zip(&[(name, &deflated)]);
        let cd_compression = zip.len() - 22 - 46 - name.len() + 10;
        zip[cd_compression..cd_compression + 2].copy_from_slice(&METHOD_DEFLATED.to_le_bytes());
        zip
    }

    #[test]
    fn deflated_entry_inflating_past_the_cap_is_rejected() {
        let zip = make_deflated_bomb("bomb.md", MAX_ENTRY_BYTES as usize + 1);
        let archive = ZipArchive::new(&zip).unwrap();
        let mut buf = String::new();
        assert!(matches!(
            archive.by_index(0).unwrap().read_to_string(&mut buf),
            Err(ZipError::EntryTooLarge(_))
        ));
        assert!(buf.is_empty());
    }

    #[test]
    fn deflated_entry_within_the_cap_is_read() {
        let zip = make_deflated_bomb("ok.md", 1024);
        let archive = ZipArchive::new(&zip).unwrap();
        let mut buf = String::new();
        archive
            .by_index(0)
            .unwrap()
            .read_to_string(&mut buf)
            .unwrap();
        assert_eq!(buf.len(), 1024);
    }

    #[test]
    fn rejects_invalid_data() {
        assert!(ZipArchive::new(b"not a zip file").is_err());
    }

    #[test]
    fn empty_zip() {
        let zip = make_stored_zip(&[]);
        let archive = ZipArchive::new(&zip).unwrap();
        assert!(archive.is_empty());
    }

    #[test]
    fn out_of_range_index() {
        let zip = make_stored_zip(&[("a.md", b"hi")]);
        let archive = ZipArchive::new(&zip).unwrap();
        assert!(archive.by_index(1).is_err());
    }
}
