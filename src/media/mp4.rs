//! A box-header walker over any `Read + Seek`, targeted at exactly two
//! things GoPro chapters carry (design D4):
//!
//! - `moov/udta/HMMT`: HiLight marker millisecond offsets.
//! - `moov/mvhd`: the camera-clock creation time.
//!
//! Nothing outside these paths is interpreted, and no box tree is
//! materialized — each level is scanned linearly, seeking past any box
//! that isn't on the path we need.

use std::io::{Read, Seek, SeekFrom};

use jiff::Timestamp;

/// Seconds between the MP4/QuickTime epoch (1904-01-01 UTC) and the
/// Unix epoch (1970-01-01 UTC): `mvhd` creation times are in the
/// former, `jiff::Timestamp` wants the latter.
const MAC_EPOCH_OFFSET_SECS: i64 = 2_082_844_800;

#[derive(Debug, thiserror::Error)]
pub enum Mp4Error {
    #[error("I/O error reading MP4 data: {0}")]
    Io(#[from] std::io::Error),

    #[error("malformed MP4 box structure: {0}")]
    Malformed(String),

    #[error("required box not found: {0}")]
    BoxNotFound(&'static str),
}

type Result<T> = std::result::Result<T, Mp4Error>;

/// A byte range `[start, end)` within the stream that some box's
/// content occupies (or, at the top level, the whole file).
type Range = (u64, u64);

fn stream_len<R: Read + Seek>(reader: &mut R) -> Result<u64> {
    let len = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;
    Ok(len)
}

/// Scans the sibling boxes within `range` for one matching `fourcc`,
/// returning its content range if found. `Ok(None)` means the range
/// was well-formed (every box header parsed cleanly, sizes accounted
/// for the whole range) but no box with that tag was present — a
/// clean absence, not an error (spec: "missing box ... contributes no
/// markers"). A header that can't be read, or a size that doesn't fit
/// within `range`, is treated as corruption and returned as `Err`.
fn find_box<R: Read + Seek>(
    reader: &mut R,
    range: Range,
    fourcc: &[u8; 4],
) -> Result<Option<Range>> {
    let (start, end) = range;
    let mut pos = start;

    while pos < end {
        if pos + 8 > end {
            return Err(Mp4Error::Malformed(format!(
                "truncated box header at offset {pos}"
            )));
        }
        reader.seek(SeekFrom::Start(pos))?;
        let mut header = [0u8; 8];
        reader.read_exact(&mut header)?;

        let mut size = u32::from_be_bytes(header[0..4].try_into().unwrap()) as u64;
        let box_fourcc: [u8; 4] = header[4..8].try_into().unwrap();
        let mut header_len = 8u64;

        if size == 1 {
            if pos + 16 > end {
                return Err(Mp4Error::Malformed(format!(
                    "truncated 64-bit box size at offset {pos}"
                )));
            }
            let mut ext = [0u8; 8];
            reader.read_exact(&mut ext)?;
            size = u64::from_be_bytes(ext);
            header_len = 16;
        } else if size == 0 {
            // Box extends to the end of its enclosing container.
            size = end - pos;
        }

        if size < header_len || pos + size > end {
            return Err(Mp4Error::Malformed(format!(
                "box at offset {pos} has an invalid size"
            )));
        }

        if box_fourcc == *fourcc {
            return Ok(Some((pos + header_len, pos + size)));
        }
        pos += size;
    }

    Ok(None)
}

/// Reads HiLight marker offsets from `moov/udta/HMMT`: a big-endian
/// u32 count followed by that many big-endian u32 millisecond
/// offsets. Any box missing along the path, or a count of zero, is
/// reported as zero markers rather than an error (spec).
pub fn read_hilights<R: Read + Seek>(reader: &mut R) -> Result<Vec<u32>> {
    let file_len = stream_len(reader)?;

    let Some(moov) = find_box(reader, (0, file_len), b"moov")? else {
        return Ok(Vec::new());
    };
    let Some(udta) = find_box(reader, moov, b"udta")? else {
        return Ok(Vec::new());
    };
    let Some((hmmt_start, hmmt_end)) = find_box(reader, udta, b"HMMT")? else {
        return Ok(Vec::new());
    };
    let hmmt_len = hmmt_end - hmmt_start;

    if hmmt_len < 4 {
        return Ok(Vec::new());
    }

    reader.seek(SeekFrom::Start(hmmt_start))?;
    let mut count_bytes = [0u8; 4];
    reader.read_exact(&mut count_bytes)?;
    let count = u32::from_be_bytes(count_bytes);

    if count == 0 {
        return Ok(Vec::new());
    }

    let needed = count as u64 * 4;
    if 4 + needed > hmmt_len {
        return Err(Mp4Error::Malformed(format!(
            "HMMT declares {count} marker(s) but its box is too small to hold them"
        )));
    }

    let mut offsets = Vec::with_capacity(count as usize);
    let mut buf = [0u8; 4];
    for _ in 0..count {
        reader.read_exact(&mut buf)?;
        offsets.push(u32::from_be_bytes(buf));
    }
    Ok(offsets)
}

/// Reads the camera-clock creation time from `moov/mvhd`: version 0
/// stores it as a u32, version 1 as a u64, both seconds since
/// 1904-01-01 UTC. Unlike `read_hilights`, a missing `moov` or `mvhd`
/// box is an error here — callers fall back to filesystem mtime
/// (design D5) rather than treating "no timestamp" as a valid result.
pub fn read_creation_time<R: Read + Seek>(reader: &mut R) -> Result<Timestamp> {
    let file_len = stream_len(reader)?;

    let moov = find_box(reader, (0, file_len), b"moov")?.ok_or(Mp4Error::BoxNotFound("moov"))?;
    let (mvhd_start, mvhd_end) =
        find_box(reader, moov, b"mvhd")?.ok_or(Mp4Error::BoxNotFound("mvhd"))?;
    let mvhd_len = mvhd_end - mvhd_start;

    reader.seek(SeekFrom::Start(mvhd_start))?;
    let mut version_and_flags = [0u8; 4];
    reader.read_exact(&mut version_and_flags)?;
    let version = version_and_flags[0];

    let creation_time_secs: i64 = match version {
        0 => {
            if mvhd_len < 8 {
                return Err(Mp4Error::Malformed("mvhd too short for version 0".into()));
            }
            let mut buf = [0u8; 4];
            reader.read_exact(&mut buf)?;
            u32::from_be_bytes(buf) as i64
        }
        1 => {
            if mvhd_len < 12 {
                return Err(Mp4Error::Malformed("mvhd too short for version 1".into()));
            }
            let mut buf = [0u8; 8];
            reader.read_exact(&mut buf)?;
            u64::from_be_bytes(buf) as i64
        }
        other => {
            return Err(Mp4Error::Malformed(format!(
                "unsupported mvhd version {other}"
            )));
        }
    };

    let unix_secs = creation_time_secs - MAC_EPOCH_OFFSET_SECS;
    Timestamp::from_second(unix_secs)
        .map_err(|e| Mp4Error::Malformed(format!("creation time out of range: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Assembles one box: 4-byte BE size + 4-byte fourcc + payload.
    fn make_box(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + payload.len());
        buf.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
        buf.extend_from_slice(fourcc);
        buf.extend_from_slice(payload);
        buf
    }

    /// Same as `make_box` but forces the 64-bit extended-size form
    /// (`size == 1` followed by an 8-byte real size), to exercise that
    /// path in `find_box`.
    fn make_box_64(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(16 + payload.len());
        buf.extend_from_slice(&1u32.to_be_bytes());
        buf.extend_from_slice(fourcc);
        buf.extend_from_slice(&((16 + payload.len()) as u64).to_be_bytes());
        buf.extend_from_slice(payload);
        buf
    }

    fn make_container(fourcc: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
        make_box(fourcc, &children.concat())
    }

    fn hmmt_payload(offsets: &[u32]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(4 + offsets.len() * 4);
        payload.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
        for offset in offsets {
            payload.extend_from_slice(&offset.to_be_bytes());
        }
        payload
    }

    fn mvhd_v0(creation_time: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 4]; // version 0 + flags
        payload.extend_from_slice(&creation_time.to_be_bytes());
        make_box(b"mvhd", &payload)
    }

    fn mvhd_v1(creation_time: u64) -> Vec<u8> {
        let mut payload = vec![1u8, 0, 0, 0]; // version 1 + flags
        payload.extend_from_slice(&creation_time.to_be_bytes());
        make_box(b"mvhd", &payload)
    }

    fn cursor(bytes: Vec<u8>) -> Cursor<Vec<u8>> {
        Cursor::new(bytes)
    }

    #[test]
    fn markers_parsed_from_hmmt() {
        let hmmt = make_box(b"HMMT", &hmmt_payload(&[5000, 73000]));
        let udta = make_container(b"udta", &[hmmt]);
        let moov = make_container(b"moov", &[udta]);
        let mut file = cursor(moov);

        let markers = read_hilights(&mut file).unwrap();
        assert_eq!(markers, vec![5000, 73000]);
    }

    #[test]
    fn no_hmmt_box_means_no_markers() {
        let udta = make_container(b"udta", &[]);
        let moov = make_container(b"moov", &[udta]);
        let mut file = cursor(moov);

        assert_eq!(read_hilights(&mut file).unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn zero_count_means_no_markers() {
        let hmmt = make_box(b"HMMT", &hmmt_payload(&[]));
        let udta = make_container(b"udta", &[hmmt]);
        let moov = make_container(b"moov", &[udta]);
        let mut file = cursor(moov);

        assert_eq!(read_hilights(&mut file).unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn missing_moov_means_no_markers() {
        let ftyp = make_box(b"ftyp", b"isom");
        let mut file = cursor(ftyp);

        assert_eq!(read_hilights(&mut file).unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn mvhd_version_0_creation_time() {
        // 3 155 673 600 seconds after 1904-01-01 is 2026-01-01 UTC.
        let moov = make_container(b"moov", &[mvhd_v0(3_155_673_600)]);
        let mut file = cursor(moov);

        let ts = read_creation_time(&mut file).unwrap();
        assert_eq!(
            ts,
            Timestamp::from_second(3_155_673_600 - MAC_EPOCH_OFFSET_SECS).unwrap()
        );
    }

    #[test]
    fn mvhd_version_1_creation_time() {
        let moov = make_container(b"moov", &[mvhd_v1(3_155_673_600)]);
        let mut file = cursor(moov);

        let ts = read_creation_time(&mut file).unwrap();
        assert_eq!(
            ts,
            Timestamp::from_second(3_155_673_600 - MAC_EPOCH_OFFSET_SECS).unwrap()
        );
    }

    #[test]
    fn extended_64_bit_box_size_is_handled() {
        let mvhd = mvhd_v0(3_155_673_600);
        let moov = make_box_64(b"moov", &mvhd);
        let mut file = cursor(moov);

        let ts = read_creation_time(&mut file).unwrap();
        assert_eq!(
            ts,
            Timestamp::from_second(3_155_673_600 - MAC_EPOCH_OFFSET_SECS).unwrap()
        );
    }

    #[test]
    fn missing_mvhd_is_an_error() {
        let moov = make_container(b"moov", &[]);
        let mut file = cursor(moov);

        assert!(matches!(
            read_creation_time(&mut file),
            Err(Mp4Error::BoxNotFound("mvhd"))
        ));
    }

    #[test]
    fn truncated_input_fails_without_panic() {
        // A box claims a size far larger than the buffer actually
        // holds.
        let mut bytes = 5000u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"moov");
        bytes.extend_from_slice(b"short");
        let mut file = cursor(bytes);

        assert!(read_hilights(&mut file).is_err());
    }

    #[test]
    fn garbage_input_fails_without_panic() {
        let mut file = cursor(vec![0xFF; 3]);
        assert!(read_hilights(&mut file).is_err());
        let mut file = cursor(vec![0xFF; 3]);
        assert!(read_creation_time(&mut file).is_err());
    }
}
