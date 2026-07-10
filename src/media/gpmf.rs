//! Hand-rolled GPMF KLV parser (ADR 0002, design D1). GPMF telemetry
//! payloads are a flat, self-describing binary format — 4-byte key +
//! 1-byte type + 1-byte element size + 2-byte repeat count, followed by
//! that many elements padded to 4-byte alignment, with nesting (type
//! `0x00`) handled by recursing into the value bytes as another KLV
//! stream. [`KlvIter`] borrows the payload rather than building a tree:
//! payloads are a few KB and read once, so there's nothing to gain from
//! owning the bytes.

use jiff::Timestamp;

#[derive(Debug, thiserror::Error)]
pub enum GpmfError {
    #[error("malformed GPMF data: {0}")]
    Malformed(String),
}

type Result<T> = std::result::Result<T, GpmfError>;

/// One decoded KLV item: its fourcc key, type tag (`0x00` for a nested
/// container), the byte width of one element, how many elements repeat,
/// and the borrowed value bytes (`struct_size * repeat` bytes, before
/// alignment padding).
#[derive(Debug, Clone, Copy)]
pub struct Klv<'a> {
    pub key: [u8; 4],
    pub type_char: u8,
    pub struct_size: u8,
    pub repeat: u16,
    pub value: &'a [u8],
}

/// Iterates the KLV items in one GPMF payload (or one nested
/// container's value bytes), advancing by each item's 4-byte-aligned
/// length. Yields `Err` and stops (rather than looping on garbage) as
/// soon as a header or length doesn't fit the remaining bytes.
#[derive(Debug, Clone, Copy)]
pub struct KlvIter<'a> {
    data: &'a [u8],
    pos: usize,
    failed: bool,
}

impl<'a> KlvIter<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        KlvIter {
            data,
            pos: 0,
            failed: false,
        }
    }
}

impl<'a> Iterator for KlvIter<'a> {
    type Item = Result<Klv<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.pos >= self.data.len() {
            return None;
        }
        if self.pos + 8 > self.data.len() {
            self.failed = true;
            return Some(Err(GpmfError::Malformed(format!(
                "truncated KLV header at offset {}",
                self.pos
            ))));
        }

        let header = &self.data[self.pos..self.pos + 8];
        let key: [u8; 4] = header[0..4].try_into().unwrap();
        let type_char = header[4];
        let struct_size = header[5];
        let repeat = u16::from_be_bytes(header[6..8].try_into().unwrap());

        let value_len = struct_size as usize * repeat as usize;
        let padded_len = value_len.div_ceil(4) * 4;
        let value_start = self.pos + 8;

        if value_start + padded_len > self.data.len() {
            self.failed = true;
            return Some(Err(GpmfError::Malformed(format!(
                "KLV item {:?} at offset {} declares {value_len} byte(s), past the end of its payload",
                String::from_utf8_lossy(&key),
                self.pos
            ))));
        }

        let value = &self.data[value_start..value_start + value_len];
        self.pos = value_start + padded_len;

        Some(Ok(Klv {
            key,
            type_char,
            struct_size,
            repeat,
            value,
        }))
    }
}

impl<'a> Klv<'a> {
    pub fn is_nested(&self) -> bool {
        self.type_char == 0
    }

    /// Iterates this item's value bytes as a nested KLV stream (only
    /// meaningful when [`is_nested`](Self::is_nested) is true).
    pub fn children(&self) -> KlvIter<'a> {
        KlvIter::new(self.value)
    }

    /// Decodes `value` as a sequence of fixed-width integer cells,
    /// widening whatever GPMF integer type `type_char` declares
    /// (`b`/`B`/`s`/`S`/`l`/`L`) to `i64` — enough range for everything
    /// `GPS5`, `GPSF`, `GPSP`, and `SCAL` carry.
    fn cells_i64(&self) -> Result<Vec<i64>> {
        let width = match self.type_char {
            b'b' | b'B' => 1,
            b's' | b'S' => 2,
            b'l' | b'L' => 4,
            other => {
                return Err(GpmfError::Malformed(format!(
                    "unsupported numeric KLV type {:?}",
                    other as char
                )));
            }
        };
        if !self.value.len().is_multiple_of(width) {
            return Err(GpmfError::Malformed(
                "KLV value length is not a multiple of its element width".into(),
            ));
        }
        let signed = matches!(self.type_char, b'b' | b's' | b'l');
        Ok(self
            .value
            .chunks_exact(width)
            .map(|cell| match (width, signed) {
                (1, true) => cell[0] as i8 as i64,
                (1, false) => cell[0] as i64,
                (2, true) => i16::from_be_bytes(cell.try_into().unwrap()) as i64,
                (2, false) => u16::from_be_bytes(cell.try_into().unwrap()) as i64,
                (4, true) => i32::from_be_bytes(cell.try_into().unwrap()) as i64,
                (4, false) => u32::from_be_bytes(cell.try_into().unwrap()) as i64,
                _ => unreachable!("width is always 1, 2, or 4"),
            })
            .collect())
    }

    pub fn as_i32s(&self) -> Result<Vec<i32>> {
        Ok(self.cells_i64()?.into_iter().map(|v| v as i32).collect())
    }

    pub fn as_u32(&self) -> Result<u32> {
        self.cells_i64()?
            .first()
            .map(|&v| v as u32)
            .ok_or_else(|| GpmfError::Malformed("expected at least one value, got none".into()))
    }

    /// Parses a `GPSU` value: `yymmddhhmmss.sss`, ASCII, interpreted as
    /// UTC (spec: "GPSU parsed as UTC").
    pub fn as_utc(&self) -> Result<Timestamp> {
        let raw = std::str::from_utf8(self.value)
            .map_err(|_| GpmfError::Malformed("GPSU is not valid UTF-8".into()))?;
        let s = raw.trim_end_matches('\0');
        if !s.is_ascii() || s.len() < 13 {
            return Err(GpmfError::Malformed(format!(
                "malformed GPSU value {raw:?}"
            )));
        }

        let two_digits = |slice: &str| -> Result<i8> {
            slice
                .parse()
                .map_err(|_| GpmfError::Malformed(format!("malformed GPSU value {raw:?}")))
        };
        let year = 2000 + two_digits(&s[0..2])? as i16;
        let month = two_digits(&s[2..4])?;
        let day = two_digits(&s[4..6])?;
        let hour = two_digits(&s[6..8])?;
        let minute = two_digits(&s[8..10])?;
        let seconds: f64 = s[10..]
            .parse()
            .map_err(|_| GpmfError::Malformed(format!("malformed GPSU value {raw:?}")))?;
        let second = seconds.trunc() as i8;
        let nanos = (seconds.fract() * 1_000_000_000.0).round() as i32;

        jiff::civil::DateTime::new(year, month, day, hour, minute, second, nanos)
            .map_err(|e| GpmfError::Malformed(format!("invalid GPSU date/time {raw:?}: {e}")))?
            .to_zoned(jiff::tz::TimeZone::UTC)
            .map(|z| z.timestamp())
            .map_err(|e| GpmfError::Malformed(format!("GPSU out of range {raw:?}: {e}")))
    }
}

/// One scaled `GPS5` sample, in stream order (design D1): latitude and
/// longitude in degrees, altitude in meters, 2D/3D speed in m/s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GpsSample {
    pub lat: f64,
    pub lon: f64,
    pub alt: f64,
    pub speed_2d: f64,
    pub speed_3d: f64,
}

/// One telemetry payload's decoded GPS stream (design D1/D3).
#[derive(Debug, Clone, PartialEq)]
pub struct GpsPayload {
    pub samples: Vec<GpsSample>,
    pub utc: Option<Timestamp>,
    pub fix: u32,
    pub precision: u32,
}

impl GpsPayload {
    /// Fix-quality gate (design D3, spec: "Fix-quality gating"): at
    /// least a 2D lock, and DOP (`GPSP` is DOP * 100) no worse than 5.0.
    pub fn usable(&self) -> bool {
        self.fix >= 2 && self.precision <= 500
    }
}

/// Recursively locates the `STRM` container holding a `GPS5` key
/// (design D1): the sibling items at that level are `SCAL`/`GPSU`/
/// `GPSF`/`GPSP`/`GPS5` for the same stream, so once `GPS5` is found at
/// a level, that level's items are the answer. Other streams
/// (accelerometer, gyro, ...) are skipped without inspection once ruled
/// out (spec: "Unknown streams skipped").
fn find_gps_stream<'a>(items: KlvIter<'a>) -> Result<Option<Vec<Klv<'a>>>> {
    let mut collected = Vec::new();
    for klv in items {
        collected.push(klv?);
    }
    if collected.iter().any(|klv| klv.key == *b"GPS5") {
        return Ok(Some(collected));
    }
    for klv in &collected {
        if klv.is_nested()
            && let Some(found) = find_gps_stream(klv.children())?
        {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

/// Parses one GPMF payload's GPS stream (spec: "GPMF KLV parsing").
/// `Ok(None)` means the payload parsed cleanly but carried no `GPS5`
/// stream; malformed KLV (truncated values, lengths past the payload)
/// is `Err`, never a panic.
pub fn parse_gps_payload(data: &[u8]) -> Result<Option<GpsPayload>> {
    let Some(fields) = find_gps_stream(KlvIter::new(data))? else {
        return Ok(None);
    };

    let mut scal: Option<Vec<i32>> = None;
    let mut utc = None;
    let mut fix = 0u32;
    let mut precision = u32::MAX;
    let mut raw_gps5: Option<Vec<i32>> = None;

    for klv in &fields {
        match &klv.key {
            b"SCAL" => scal = Some(klv.as_i32s()?),
            b"GPSU" => utc = Some(klv.as_utc()?),
            b"GPSF" => fix = klv.as_u32()?,
            b"GPSP" => precision = klv.as_u32()?,
            b"GPS5" => raw_gps5 = Some(klv.as_i32s()?),
            _ => {}
        }
    }

    let Some(raw) = raw_gps5 else {
        return Ok(None);
    };
    if raw.len() % 5 != 0 {
        return Err(GpmfError::Malformed(format!(
            "GPS5 carries {} value(s), not a multiple of 5",
            raw.len()
        )));
    }
    let scal = scal.unwrap_or_else(|| vec![1; 5]);
    if scal.len() < 5 {
        return Err(GpmfError::Malformed(format!(
            "SCAL has {} divisor(s), GPS5 needs 5",
            scal.len()
        )));
    }
    if scal[0..5].contains(&0) {
        return Err(GpmfError::Malformed("SCAL divisor is zero".into()));
    }

    let samples = raw
        .chunks_exact(5)
        .map(|c| GpsSample {
            lat: c[0] as f64 / scal[0] as f64,
            lon: c[1] as f64 / scal[1] as f64,
            alt: c[2] as f64 / scal[2] as f64,
            speed_2d: c[3] as f64 / scal[3] as f64,
            speed_3d: c[4] as f64 / scal[4] as f64,
        })
        .collect();

    Ok(Some(GpsPayload {
        samples,
        utc,
        fix,
        precision,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be_i32s(vals: &[i32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    /// Assembles one KLV item: 4-byte key, 1-byte type, 1-byte element
    /// size, 2-byte BE repeat count, value bytes, zero-padded to a
    /// 4-byte boundary. `value.len()` must be a multiple of
    /// `struct_size`.
    fn klv_item(key: &[u8; 4], type_char: u8, struct_size: u8, value: &[u8]) -> Vec<u8> {
        assert_eq!(value.len() % struct_size as usize, 0);
        let repeat = (value.len() / struct_size as usize) as u16;
        let mut buf = Vec::with_capacity(8 + value.len());
        buf.extend_from_slice(key);
        buf.push(type_char);
        buf.push(struct_size);
        buf.extend_from_slice(&repeat.to_be_bytes());
        buf.extend_from_slice(value);
        while buf.len() % 4 != 0 {
            buf.push(0);
        }
        buf
    }

    /// A nested container: `struct_size` 1, `repeat` = the (already
    /// 4-byte-aligned) length of its concatenated children.
    fn nested(key: &[u8; 4], children: &[Vec<u8>]) -> Vec<u8> {
        let payload = children.concat();
        klv_item(key, 0, 1, &payload)
    }

    fn gps_strm(extra_before: &[Vec<u8>]) -> Vec<u8> {
        let scal = klv_item(
            b"SCAL",
            b'l',
            4,
            &be_i32s(&[10_000_000, 10_000_000, 1000, 1000, 1000]),
        );
        let gpsu = klv_item(b"GPSU", b'U', 16, b"260709074103.250");
        let gpsf = klv_item(b"GPSF", b'L', 4, &3u32.to_be_bytes());
        let gpsp = klv_item(b"GPSP", b'S', 2, &150u16.to_be_bytes());
        let gps5 = klv_item(
            b"GPS5",
            b'l',
            4,
            &be_i32s(&[515_012_340, -1_234_567, 100_000, 5000, 5500]),
        );
        let mut children = extra_before.to_vec();
        children.extend([scal, gpsu, gpsf, gpsp, gps5]);
        nested(b"STRM", &children)
    }

    #[test]
    fn scal_scales_gps5_samples() {
        let devc = nested(b"DEVC", &[gps_strm(&[])]);
        let payload = parse_gps_payload(&devc).unwrap().unwrap();
        assert_eq!(payload.samples.len(), 1);
        assert!((payload.samples[0].lat - 51.5012340).abs() < 1e-9);
    }

    #[test]
    fn gpsu_parsed_as_utc() {
        let gpsu = klv_item(b"GPSU", b'U', 16, b"260709074103.250");
        let klv = KlvIter::new(&gpsu).next().unwrap().unwrap();
        let ts = klv.as_utc().unwrap();
        assert_eq!(ts, "2026-07-09T07:41:03.250Z".parse::<Timestamp>().unwrap());
    }

    #[test]
    fn fix_and_precision_extracted() {
        let devc = nested(b"DEVC", &[gps_strm(&[])]);
        let payload = parse_gps_payload(&devc).unwrap().unwrap();
        assert_eq!(payload.fix, 3);
        assert_eq!(payload.precision, 150);
        assert!(payload.usable());
    }

    #[test]
    fn deeply_nested_containers_are_traversed() {
        // GPS5 sits three levels down: DEVC -> WRAP -> STRM.
        let wrap = nested(b"WRAP", &[gps_strm(&[])]);
        let devc = nested(b"DEVC", &[wrap]);
        let payload = parse_gps_payload(&devc).unwrap().unwrap();
        assert_eq!(payload.samples.len(), 1);
    }

    #[test]
    fn unknown_streams_are_skipped() {
        // An ACCL stream ahead of the GPS stream, plus an unrecognized
        // top-level key, must not affect parsing.
        let accl = nested(
            b"STRM",
            &[
                klv_item(b"STNM", b'c', 1, b"Accelerometer"),
                klv_item(b"ACCL", b's', 2, &1i16.to_be_bytes()),
            ],
        );
        let unknown = klv_item(b"XYZW", b'L', 4, &7u32.to_be_bytes());
        let devc = nested(b"DEVC", &[unknown, accl, gps_strm(&[])]);

        let payload = parse_gps_payload(&devc).unwrap().unwrap();
        assert_eq!(payload.samples.len(), 1);
        assert!((payload.samples[0].lat - 51.5012340).abs() < 1e-9);
    }

    #[test]
    fn payload_without_gps_stream_is_none() {
        let accl = nested(b"STRM", &[klv_item(b"ACCL", b's', 2, &1i16.to_be_bytes())]);
        let devc = nested(b"DEVC", &[accl]);
        assert!(parse_gps_payload(&devc).unwrap().is_none());
    }

    #[test]
    fn truncated_value_fails_without_panic() {
        // Header declares a 20-byte value but the buffer holds none.
        let mut bytes = b"GPS5".to_vec();
        bytes.push(b'l');
        bytes.push(4);
        bytes.extend_from_slice(&5u16.to_be_bytes());
        // No value bytes at all follow.
        assert!(parse_gps_payload(&bytes).is_err());
    }

    #[test]
    fn garbage_input_fails_without_panic() {
        assert!(parse_gps_payload(&[0xFF; 3]).is_err());
    }

    #[test]
    fn zero_scal_divisor_is_an_error() {
        let scal = klv_item(b"SCAL", b'l', 4, &be_i32s(&[0, 1, 1, 1, 1]));
        let gps5 = klv_item(b"GPS5", b'l', 4, &be_i32s(&[1, 1, 1, 1, 1]));
        let strm = nested(b"STRM", &[scal, gps5]);
        let devc = nested(b"DEVC", &[strm]);
        assert!(parse_gps_payload(&devc).is_err());
    }
}
