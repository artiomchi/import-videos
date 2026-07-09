//! Minimal, targeted MP4 container parsing (ADR 0002). This is not a
//! general ISO BMFF parser: it walks only the box paths the GoPro
//! pipeline actually needs (`moov/udta/HMMT`, `moov/mvhd`) and skips
//! everything else by seeking past it.

pub mod mp4;
