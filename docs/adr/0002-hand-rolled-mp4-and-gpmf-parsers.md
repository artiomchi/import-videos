# 0002 — Hand-rolled MP4 and GPMF parsers

- Status: accepted
- Date: 2026-07-09

## Context

The GoPro pipeline needs exactly two things out of a chapter's MP4 container today (`moov/udta/HMMT` HiLight markers, `moov/mvhd` camera-clock creation time), and will need a third (the `gpmd` metadata track's GPMF KLV stream) once `add-gopro-gps` lands. The crates.io options for MP4 parsing are either abandoned, pull in a large dependency surface (demuxing, codec awareness) for a handful of fields we need, or don't expose the specific boxes GoPro uses. GPMF itself is undocumented outside community reverse-engineering; there is no crate for it at all. `ADR 0001` also treats binary parsing as a deliberate learning topic for this project.

## Decision

Write a minimal, targeted box walker (`src/media/mp4.rs`) directly on `std::io::{Read, Seek}`: read an 8-byte header (u32 BE size + fourcc), handle the 64-bit `size == 1` extended form, and descend only into the container path a caller asks for, skipping everything else by seeking past it. It is not a general ISO BMFF parser — it materializes no box tree and understands no boxes outside the ones a caller explicitly walks to. `add-gopro-gps` extends the same walker toward the `gpmd` track and adds a hand-rolled GPMF KLV parser alongside it, rather than replacing this approach.

Errors are typed (`thiserror`, `Mp4Error`), and a missing box along a path is a normal, non-error result (`Ok(None)` / empty output) — only genuinely malformed structure (a truncated header, a size that doesn't fit its container) is an error. This lets the GoPro module treat "no HiLight markers" and "unreadable file" as two distinct, both-safe outcomes (design D7).

## Consequences

- Zero new dependencies for container parsing; the parser's correctness is scoped to exactly the boxes it's tested against (handcrafted fixtures in `src/media/mp4.rs`'s tests), not general MP4 compliance.
- The undocumented, reverse-engineered nature of `HMMT` and GPMF means the parser encodes best-available community knowledge, not a published spec — real-hardware validation (roadmap's HiLight-count-vs-GoPro-Quik smoke test) is load-bearing before the tool is trusted with `delete_source: true`.
- If a future device needs a materially different container format (e.g. a codec requiring real demuxing), that's a new module, not an extension of this one — this walker stays deliberately narrow.
