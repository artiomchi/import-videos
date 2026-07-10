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

/// Parses one box header at `pos` within `range`, returning its fourcc
/// and content range. Shared by `find_box` (first match) and
/// `find_all_boxes` (every match, design D2: "iterate sibling boxes")
/// so both walk the same header-parsing logic. A header that can't be
/// read, or a size that doesn't fit within `range`, is corruption
/// (`Err`); the caller decides what "no match" means.
fn read_box_header<R: Read + Seek>(reader: &mut R, pos: u64, end: u64) -> Result<([u8; 4], Range)> {
    if pos + 8 > end {
        return Err(Mp4Error::Malformed(format!(
            "truncated box header at offset {pos}"
        )));
    }
    reader.seek(SeekFrom::Start(pos))?;
    let mut header = [0u8; 8];
    reader.read_exact(&mut header)?;

    let mut size = u32::from_be_bytes(header[0..4].try_into().unwrap()) as u64;
    let fourcc: [u8; 4] = header[4..8].try_into().unwrap();
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

    Ok((fourcc, (pos + header_len, pos + size)))
}

/// Scans the sibling boxes within `range` for one matching `fourcc`,
/// returning its content range if found. `Ok(None)` means the range
/// was well-formed (every box header parsed cleanly, sizes accounted
/// for the whole range) but no box with that tag was present — a
/// clean absence, not an error (spec: "missing box ... contributes no
/// markers").
fn find_box<R: Read + Seek>(
    reader: &mut R,
    range: Range,
    fourcc: &[u8; 4],
) -> Result<Option<Range>> {
    let (start, end) = range;
    let mut pos = start;

    while pos < end {
        let (box_fourcc, content) = read_box_header(reader, pos, end)?;
        if box_fourcc == *fourcc {
            return Ok(Some(content));
        }
        pos = content.1;
    }

    Ok(None)
}

/// Like `find_box`, but returns every sibling box matching `fourcc`
/// (design D2: locating the `gpmd` track among a `moov` with several
/// `trak` boxes) instead of stopping at the first.
fn find_all_boxes<R: Read + Seek>(
    reader: &mut R,
    range: Range,
    fourcc: &[u8; 4],
) -> Result<Vec<Range>> {
    let (start, end) = range;
    let mut pos = start;
    let mut found = Vec::new();

    while pos < end {
        let (box_fourcc, content) = read_box_header(reader, pos, end)?;
        if box_fourcc == *fourcc {
            found.push(content);
        }
        pos = content.1;
    }

    Ok(found)
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

/// One `gpmd` telemetry sample's location and timing (design D2): an
/// index entry only — payload bytes are fetched on demand by
/// `read_gpmd_payload`, never bulk-loaded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpmdSample {
    pub offset: u64,
    pub size: u32,
    pub time_s: f64,
    pub duration_s: f64,
}

/// Locates the `gpmd` telemetry track and builds its per-sample index
/// (design D2, spec: "GPMF track discovery", "Telemetry sample
/// index"). `Ok(None)` means no track in `moov` has a `meta` handler
/// with a `gpmd` `stsd` entry — a clean "no telemetry" result, not an
/// error. Once such a track is found, a malformed sample table is
/// `Err` rather than a silent absence.
pub fn read_gpmd_index<R: Read + Seek>(reader: &mut R) -> Result<Option<Vec<GpmdSample>>> {
    let file_len = stream_len(reader)?;
    let Some(moov) = find_box(reader, (0, file_len), b"moov")? else {
        return Ok(None);
    };

    for trak in find_all_boxes(reader, moov, b"trak")? {
        let Some((mdia, stbl)) = locate_gpmd_track(reader, trak)? else {
            continue;
        };
        let timescale = read_mdhd_timescale(reader, mdia)?;
        return build_gpmd_index(reader, stbl, timescale).map(Some);
    }
    Ok(None)
}

/// Reads one telemetry sample's payload bytes at its indexed offset —
/// read-only over `reader`, nothing is buffered beyond one sample
/// (design D2).
pub fn read_gpmd_payload<R: Read + Seek>(reader: &mut R, sample: &GpmdSample) -> Result<Vec<u8>> {
    reader.seek(SeekFrom::Start(sample.offset))?;
    let mut buf = vec![0u8; sample.size as usize];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

/// Returns `(mdia, stbl)` for `trak` iff its `mdia/hdlr` handler type
/// is `meta` and `mdia/minf/stbl/stsd`'s first entry format is `gpmd`
/// — the stsd check is what actually distinguishes the telemetry track
/// from other `meta`-handler tracks (design D2).
fn locate_gpmd_track<R: Read + Seek>(
    reader: &mut R,
    trak: Range,
) -> Result<Option<(Range, Range)>> {
    let Some(mdia) = find_box(reader, trak, b"mdia")? else {
        return Ok(None);
    };
    let Some(hdlr) = find_box(reader, mdia, b"hdlr")? else {
        return Ok(None);
    };
    if read_handler_type(reader, hdlr)? != *b"meta" {
        return Ok(None);
    }
    let Some(minf) = find_box(reader, mdia, b"minf")? else {
        return Ok(None);
    };
    let Some(stbl) = find_box(reader, minf, b"stbl")? else {
        return Ok(None);
    };
    let Some(stsd) = find_box(reader, stbl, b"stsd")? else {
        return Ok(None);
    };
    if read_stsd_first_format(reader, stsd)? != *b"gpmd" {
        return Ok(None);
    }
    Ok(Some((mdia, stbl)))
}

/// `hdlr`'s handler type sits right after the 4-byte version/flags and
/// 4-byte (unused) predefined component type.
fn read_handler_type<R: Read + Seek>(reader: &mut R, hdlr: Range) -> Result<[u8; 4]> {
    let (start, end) = hdlr;
    if end.saturating_sub(start) < 12 {
        return Err(Mp4Error::Malformed("hdlr box too short".into()));
    }
    reader.seek(SeekFrom::Start(start + 8))?;
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

/// `stsd`'s first sample entry's format fourcc: version/flags(4) +
/// entry_count(4) + entry_size(4) + format(4).
fn read_stsd_first_format<R: Read + Seek>(reader: &mut R, stsd: Range) -> Result<[u8; 4]> {
    let (start, end) = stsd;
    if end.saturating_sub(start) < 16 {
        return Err(Mp4Error::Malformed(
            "stsd box too short for an entry".into(),
        ));
    }
    reader.seek(SeekFrom::Start(start + 12))?;
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(buf)
}

/// `mdia/mdhd`'s timescale: units per second that `stts` deltas are
/// expressed in. Its offset depends on whether creation/modification
/// times are 32- or 64-bit (version 0 vs. 1).
fn read_mdhd_timescale<R: Read + Seek>(reader: &mut R, mdia: Range) -> Result<u32> {
    let (start, end) = find_box(reader, mdia, b"mdhd")?.ok_or(Mp4Error::BoxNotFound("mdhd"))?;
    reader.seek(SeekFrom::Start(start))?;
    let mut version = [0u8; 1];
    reader.read_exact(&mut version)?;
    let ts_pos = if version[0] == 1 {
        start + 4 + 16
    } else {
        start + 4 + 8
    };
    if ts_pos + 4 > end {
        return Err(Mp4Error::Malformed("mdhd too short for timescale".into()));
    }
    reader.seek(SeekFrom::Start(ts_pos))?;
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_be_bytes(buf))
}

fn build_gpmd_index<R: Read + Seek>(
    reader: &mut R,
    stbl: Range,
    timescale: u32,
) -> Result<Vec<GpmdSample>> {
    if timescale == 0 {
        return Err(Mp4Error::Malformed("mdhd timescale is zero".into()));
    }
    let sizes = read_stsz(reader, stbl)?;
    let stsc = read_stsc(reader, stbl)?;
    let chunk_offsets = read_chunk_offsets(reader, stbl)?;
    let stts = read_stts(reader, stbl)?;

    let offsets = place_samples_in_chunks(&stsc, &chunk_offsets, &sizes)?;
    let times = accumulate_sample_times(&stts, timescale, sizes.len())?;

    Ok(sizes
        .into_iter()
        .zip(offsets)
        .zip(times)
        .map(|((size, offset), (time_s, duration_s))| GpmdSample {
            offset,
            size,
            time_s,
            duration_s,
        })
        .collect())
}

/// `stsz`: a shared `sample_size` for every sample, or (`sample_size ==
/// 0`) one 4-byte size per sample.
fn read_stsz<R: Read + Seek>(reader: &mut R, stbl: Range) -> Result<Vec<u32>> {
    let (start, end) = find_box(reader, stbl, b"stsz")?.ok_or(Mp4Error::BoxNotFound("stsz"))?;
    if end.saturating_sub(start) < 12 {
        return Err(Mp4Error::Malformed("stsz box too short".into()));
    }
    reader.seek(SeekFrom::Start(start + 4))?;
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    let sample_size = u32::from_be_bytes(buf[0..4].try_into().unwrap());
    let sample_count = u32::from_be_bytes(buf[4..8].try_into().unwrap()) as usize;

    if sample_size != 0 {
        return Ok(vec![sample_size; sample_count]);
    }

    let needed = sample_count as u64 * 4;
    if start + 12 + needed > end {
        return Err(Mp4Error::Malformed(
            "stsz declares more samples than its box holds".into(),
        ));
    }
    let mut sizes = Vec::with_capacity(sample_count);
    let mut buf4 = [0u8; 4];
    for _ in 0..sample_count {
        reader.read_exact(&mut buf4)?;
        sizes.push(u32::from_be_bytes(buf4));
    }
    Ok(sizes)
}

/// `stsc`: `(first_chunk, samples_per_chunk, sample_description_index)`
/// triples, one-based `first_chunk`.
fn read_stsc<R: Read + Seek>(reader: &mut R, stbl: Range) -> Result<Vec<(u32, u32, u32)>> {
    let (start, end) = find_box(reader, stbl, b"stsc")?.ok_or(Mp4Error::BoxNotFound("stsc"))?;
    if end.saturating_sub(start) < 8 {
        return Err(Mp4Error::Malformed("stsc box too short".into()));
    }
    reader.seek(SeekFrom::Start(start + 4))?;
    let mut cnt = [0u8; 4];
    reader.read_exact(&mut cnt)?;
    let entry_count = u32::from_be_bytes(cnt) as usize;

    let needed = entry_count as u64 * 12;
    if start + 8 + needed > end {
        return Err(Mp4Error::Malformed(
            "stsc declares more entries than its box holds".into(),
        ));
    }
    let mut entries = Vec::with_capacity(entry_count);
    let mut buf = [0u8; 12];
    for _ in 0..entry_count {
        reader.read_exact(&mut buf)?;
        entries.push((
            u32::from_be_bytes(buf[0..4].try_into().unwrap()),
            u32::from_be_bytes(buf[4..8].try_into().unwrap()),
            u32::from_be_bytes(buf[8..12].try_into().unwrap()),
        ));
    }
    Ok(entries)
}

/// Absolute chunk offsets from `co64` (64-bit) if present, else `stco`
/// (32-bit).
fn read_chunk_offsets<R: Read + Seek>(reader: &mut R, stbl: Range) -> Result<Vec<u64>> {
    if let Some((start, end)) = find_box(reader, stbl, b"co64")? {
        if end.saturating_sub(start) < 8 {
            return Err(Mp4Error::Malformed("co64 box too short".into()));
        }
        reader.seek(SeekFrom::Start(start + 4))?;
        let mut cnt = [0u8; 4];
        reader.read_exact(&mut cnt)?;
        let entry_count = u32::from_be_bytes(cnt) as usize;
        let needed = entry_count as u64 * 8;
        if start + 8 + needed > end {
            return Err(Mp4Error::Malformed(
                "co64 declares more entries than its box holds".into(),
            ));
        }
        let mut offsets = Vec::with_capacity(entry_count);
        let mut buf = [0u8; 8];
        for _ in 0..entry_count {
            reader.read_exact(&mut buf)?;
            offsets.push(u64::from_be_bytes(buf));
        }
        return Ok(offsets);
    }

    let (start, end) = find_box(reader, stbl, b"stco")?.ok_or(Mp4Error::BoxNotFound("stco"))?;
    if end.saturating_sub(start) < 8 {
        return Err(Mp4Error::Malformed("stco box too short".into()));
    }
    reader.seek(SeekFrom::Start(start + 4))?;
    let mut cnt = [0u8; 4];
    reader.read_exact(&mut cnt)?;
    let entry_count = u32::from_be_bytes(cnt) as usize;
    let needed = entry_count as u64 * 4;
    if start + 8 + needed > end {
        return Err(Mp4Error::Malformed(
            "stco declares more entries than its box holds".into(),
        ));
    }
    let mut offsets = Vec::with_capacity(entry_count);
    let mut buf = [0u8; 4];
    for _ in 0..entry_count {
        reader.read_exact(&mut buf)?;
        offsets.push(u32::from_be_bytes(buf) as u64);
    }
    Ok(offsets)
}

/// `stts`: `(sample_count, sample_delta)` pairs, delta in `mdhd`
/// timescale units.
fn read_stts<R: Read + Seek>(reader: &mut R, stbl: Range) -> Result<Vec<(u32, u32)>> {
    let (start, end) = find_box(reader, stbl, b"stts")?.ok_or(Mp4Error::BoxNotFound("stts"))?;
    if end.saturating_sub(start) < 8 {
        return Err(Mp4Error::Malformed("stts box too short".into()));
    }
    reader.seek(SeekFrom::Start(start + 4))?;
    let mut cnt = [0u8; 4];
    reader.read_exact(&mut cnt)?;
    let entry_count = u32::from_be_bytes(cnt) as usize;
    let needed = entry_count as u64 * 8;
    if start + 8 + needed > end {
        return Err(Mp4Error::Malformed(
            "stts declares more entries than its box holds".into(),
        ));
    }
    let mut entries = Vec::with_capacity(entry_count);
    let mut buf = [0u8; 8];
    for _ in 0..entry_count {
        reader.read_exact(&mut buf)?;
        entries.push((
            u32::from_be_bytes(buf[0..4].try_into().unwrap()),
            u32::from_be_bytes(buf[4..8].try_into().unwrap()),
        ));
    }
    Ok(entries)
}

/// Walks chunks in order, assigning each the next `samples_per_chunk`
/// samples per `stsc` (honoring the sample-to-chunk mapping rather
/// than assuming one sample per chunk, design D2) and computing each
/// sample's absolute file offset from its chunk's base offset plus the
/// running size of samples already placed in that chunk.
fn place_samples_in_chunks(
    stsc: &[(u32, u32, u32)],
    chunk_offsets: &[u64],
    sizes: &[u32],
) -> Result<Vec<u64>> {
    let mut offsets = Vec::with_capacity(sizes.len());
    let mut sample_idx = 0usize;

    for (chunk_i, &chunk_offset) in chunk_offsets.iter().enumerate() {
        let chunk_number = chunk_i as u32 + 1;
        let samples_per_chunk = stsc
            .iter()
            .rev()
            .find(|(first_chunk, _, _)| *first_chunk <= chunk_number)
            .map(|(_, spc, _)| *spc)
            .unwrap_or(0);

        let mut pos = chunk_offset;
        for _ in 0..samples_per_chunk {
            let Some(&size) = sizes.get(sample_idx) else {
                return Err(Mp4Error::Malformed(
                    "stsc/stco place more samples than stsz declares".into(),
                ));
            };
            offsets.push(pos);
            pos += size as u64;
            sample_idx += 1;
        }
    }

    if sample_idx != sizes.len() {
        return Err(Mp4Error::Malformed(format!(
            "stsc/stco placed {sample_idx} sample(s) but stsz declares {}",
            sizes.len()
        )));
    }
    Ok(offsets)
}

/// Expands `stts`'s run-length entries into a cumulative
/// `(start_time_s, duration_s)` per sample.
fn accumulate_sample_times(
    stts: &[(u32, u32)],
    timescale: u32,
    n_samples: usize,
) -> Result<Vec<(f64, f64)>> {
    let mut times = Vec::with_capacity(n_samples);
    let mut cumulative_units: u64 = 0;
    for &(count, delta) in stts {
        for _ in 0..count {
            times.push((
                cumulative_units as f64 / timescale as f64,
                delta as f64 / timescale as f64,
            ));
            cumulative_units += delta as u64;
        }
    }
    if times.len() != n_samples {
        return Err(Mp4Error::Malformed(format!(
            "stts declares {} sample(s) but stsz declares {n_samples}",
            times.len()
        )));
    }
    Ok(times)
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

    // --- gpmd track discovery / sample index (design D2) ---

    fn hdlr(handler_type: &[u8; 4]) -> Vec<u8> {
        let mut payload = vec![0u8; 8]; // version+flags, pre_defined
        payload.extend_from_slice(handler_type);
        payload.extend_from_slice(&[0u8; 12]); // reserved
        make_box(b"hdlr", &payload)
    }

    fn stsd(format: &[u8; 4]) -> Vec<u8> {
        let mut entry = Vec::new();
        entry.extend_from_slice(&16u32.to_be_bytes()); // entry size
        entry.extend_from_slice(format);
        entry.extend_from_slice(&[0u8; 8]); // reserved + data_reference_index
        let mut payload = vec![0u8; 4]; // version+flags
        payload.extend_from_slice(&1u32.to_be_bytes()); // entry_count
        payload.extend_from_slice(&entry);
        make_box(b"stsd", &payload)
    }

    fn mdhd(timescale: u32) -> Vec<u8> {
        let mut payload = vec![0u8; 4]; // version 0 + flags
        payload.extend_from_slice(&[0u8; 4]); // creation_time
        payload.extend_from_slice(&[0u8; 4]); // modification_time
        payload.extend_from_slice(&timescale.to_be_bytes());
        payload.extend_from_slice(&[0u8; 4]); // duration
        make_box(b"mdhd", &payload)
    }

    fn stsz(sizes: &[u32]) -> Vec<u8> {
        let mut payload = vec![0u8; 4]; // version+flags
        payload.extend_from_slice(&0u32.to_be_bytes()); // sample_size == 0
        payload.extend_from_slice(&(sizes.len() as u32).to_be_bytes());
        for size in sizes {
            payload.extend_from_slice(&size.to_be_bytes());
        }
        make_box(b"stsz", &payload)
    }

    fn stsc(entries: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut payload = vec![0u8; 4];
        payload.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (first_chunk, samples_per_chunk, sdi) in entries {
            payload.extend_from_slice(&first_chunk.to_be_bytes());
            payload.extend_from_slice(&samples_per_chunk.to_be_bytes());
            payload.extend_from_slice(&sdi.to_be_bytes());
        }
        make_box(b"stsc", &payload)
    }

    fn stco(offsets: &[u32]) -> Vec<u8> {
        let mut payload = vec![0u8; 4];
        payload.extend_from_slice(&(offsets.len() as u32).to_be_bytes());
        for offset in offsets {
            payload.extend_from_slice(&offset.to_be_bytes());
        }
        make_box(b"stco", &payload)
    }

    fn stts(entries: &[(u32, u32)]) -> Vec<u8> {
        let mut payload = vec![0u8; 4];
        payload.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (count, delta) in entries {
            payload.extend_from_slice(&count.to_be_bytes());
            payload.extend_from_slice(&delta.to_be_bytes());
        }
        make_box(b"stts", &payload)
    }

    fn gpmd_trak(
        timescale: u32,
        sizes: &[u32],
        stsc_entries: &[(u32, u32, u32)],
        offsets: &[u32],
    ) -> Vec<u8> {
        let stbl = make_container(
            b"stbl",
            &[
                stsd(b"gpmd"),
                stsz(sizes),
                stsc(stsc_entries),
                stco(offsets),
                stts(&[(sizes.len() as u32, timescale / sizes.len().max(1) as u32)]),
            ],
        );
        let minf = make_container(b"minf", &[stbl]);
        let mdia = make_container(b"mdia", &[hdlr(b"meta"), mdhd(timescale), minf]);
        make_container(b"trak", &[mdia])
    }

    fn other_trak(handler_type: &[u8; 4], stsd_format: &[u8; 4]) -> Vec<u8> {
        let stbl = make_container(b"stbl", &[stsd(stsd_format)]);
        let minf = make_container(b"minf", &[stbl]);
        let mdia = make_container(b"mdia", &[hdlr(handler_type), mdhd(1000), minf]);
        make_container(b"trak", &[mdia])
    }

    #[test]
    fn gpmd_track_found_among_other_tracks() {
        let video = other_trak(b"vide", b"avc1");
        let audio = other_trak(b"soun", b"mp4a");
        let other_meta = other_trak(b"meta", b"fdsc");
        let gpmd = gpmd_trak(1000, &[10, 12], &[(1, 1, 1)], &[100, 200]);
        let moov = make_container(b"moov", &[video, audio, other_meta, gpmd]);
        let mut file = cursor(moov);

        let index = read_gpmd_index(&mut file).unwrap().unwrap();
        assert_eq!(index.len(), 2);
    }

    #[test]
    fn file_without_gpmd_track_yields_none() {
        let video = other_trak(b"vide", b"avc1");
        let moov = make_container(b"moov", &[video]);
        let mut file = cursor(moov);

        assert!(read_gpmd_index(&mut file).unwrap().is_none());
    }

    #[test]
    fn index_built_from_sample_tables_across_multiple_chunks() {
        // 3 samples across 2 chunks (chunk 1 holds 2, chunk 2 holds 1),
        // 1000-unit timescale, 1000-unit durations (spec: "Index built
        // from sample tables").
        let stbl = make_container(
            b"stbl",
            &[
                stsd(b"gpmd"),
                stsz(&[10, 20, 30]),
                stsc(&[(1, 2, 1), (2, 1, 1)]),
                stco(&[1000, 2000]),
                stts(&[(3, 1000)]),
            ],
        );
        let minf = make_container(b"minf", &[stbl]);
        let mdia = make_container(b"mdia", &[hdlr(b"meta"), mdhd(1000), minf]);
        let trak = make_container(b"trak", &[mdia]);
        let moov = make_container(b"moov", &[trak]);
        let mut file = cursor(moov);

        let index = read_gpmd_index(&mut file).unwrap().unwrap();
        assert_eq!(index.len(), 3);
        assert_eq!(index[0].offset, 1000);
        assert_eq!(index[0].size, 10);
        assert_eq!(index[0].time_s, 0.0);
        assert_eq!(index[1].offset, 1010);
        assert_eq!(index[1].time_s, 1.0);
        assert_eq!(
            index[2].offset, 2000,
            "chunk 2's sample starts at its own chunk offset"
        );
        assert_eq!(index[2].time_s, 2.0);
    }

    #[test]
    fn corrupt_sample_table_fails_cleanly() {
        // stsz declares 3 samples but stsc/stco can only place 1.
        let stbl = make_container(
            b"stbl",
            &[
                stsd(b"gpmd"),
                stsz(&[10, 20, 30]),
                stsc(&[(1, 1, 1)]),
                stco(&[1000]),
                stts(&[(3, 1000)]),
            ],
        );
        let minf = make_container(b"minf", &[stbl]);
        let mdia = make_container(b"mdia", &[hdlr(b"meta"), mdhd(1000), minf]);
        let trak = make_container(b"trak", &[mdia]);
        let moov = make_container(b"moov", &[trak]);
        let mut file = cursor(moov);

        assert!(read_gpmd_index(&mut file).is_err());
    }

    #[test]
    fn payload_read_at_indexed_offset() {
        // A distinctive sentinel for the `stco` entry, unlikely to
        // collide with any other all-zero field in the box tree, so it
        // can be found and patched to the real trailing offset once
        // that's known.
        const SENTINEL: u32 = 0xAB19_2F03;

        let stbl = make_container(
            b"stbl",
            &[
                stsd(b"gpmd"),
                stsz(&[5]),
                stsc(&[(1, 1, 1)]),
                stco(&[SENTINEL]),
                stts(&[(1, 1000)]),
            ],
        );
        let minf = make_container(b"minf", &[stbl]);
        let mdia = make_container(b"mdia", &[hdlr(b"meta"), mdhd(1000), minf]);
        let trak = make_container(b"trak", &[mdia]);
        let mut moov_bytes = make_container(b"moov", &[trak]);

        let payload = b"hello";
        let payload_offset = moov_bytes.len() as u32;
        moov_bytes.extend_from_slice(payload);

        let marker = SENTINEL.to_be_bytes();
        let pos = moov_bytes
            .windows(4)
            .position(|w| w == marker)
            .expect("stco sentinel not found");
        moov_bytes[pos..pos + 4].copy_from_slice(&payload_offset.to_be_bytes());

        let mut file = cursor(moov_bytes);
        let index = read_gpmd_index(&mut file).unwrap().unwrap();
        assert_eq!(index.len(), 1);
        let bytes = read_gpmd_payload(&mut file, &index[0]).unwrap();
        assert_eq!(bytes, payload);
    }
}
