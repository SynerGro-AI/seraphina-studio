//! Encoder packets read from an FLV byte stream (after OBS's encoder_packet).
//!
//! The stream delay sits between the encoder and the RTMP sender, and both
//! speak FLV. [`FlvReader`] takes the encoder's bytes in any chunk sizes and
//! hands back the file header once, then one [`EncoderPacket`] per tag. A
//! packet keeps the tag's exact bytes (header, body and the trailing
//! previous-tag-size), so writing packets back in order reproduces the
//! stream byte for byte.

/// One FLV tag: audio (8), video (9) or script data (18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderPacket {
    /// Decode timestamp in milliseconds (FLV's 24-bit time plus its
    /// extension byte).
    pub dts_ms: u32,
    /// The whole tag: 11-byte header, body, 4-byte previous-tag-size.
    pub bytes: Vec<u8>,
}

/// What [`FlvReader::next_item`] returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlvItem {
    /// The FLV header plus the first previous-tag-size (always 0).
    Header(Vec<u8>),
    Packet(EncoderPacket),
}

/// Incremental FLV parser: [`push`](FlvReader::push) bytes, then call
/// [`next_item`](FlvReader::next_item) until it returns `Ok(None)`.
#[derive(Debug, Default)]
pub struct FlvReader {
    buf: Vec<u8>,
    pos: usize,
    header_done: bool,
}

const TAG_HEADER: usize = 11;

/// The largest tag body accepted. FLV allows 16 MiB; a real encoder tag
/// (even a 4K keyframe) is far smaller, so anything bigger means the
/// stream is corrupt and would otherwise be buffered whole first.
pub const MAX_TAG: usize = 8 << 20;

impl FlvReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if self.pos > 0 && self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        } else if self.pos > 1 << 20 {
            self.buf.drain(..self.pos);
            self.pos = 0;
        }
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete item, `Ok(None)` if more bytes are needed, or an
    /// error if the bytes are not FLV.
    pub fn next_item(&mut self) -> Result<Option<FlvItem>, String> {
        let b = &self.buf[self.pos..];
        if !self.header_done {
            if b.len() >= 3 && &b[..3] != b"FLV" {
                return Err("not an FLV stream (no FLV signature)".into());
            }
            if b.len() < 9 {
                return Ok(None);
            }
            let offset = u32::from_be_bytes([b[5], b[6], b[7], b[8]]) as usize;
            if !(9..=1024).contains(&offset) {
                return Err(format!("bad FLV header size {offset}"));
            }
            let n = offset + 4;
            if b.len() < n {
                return Ok(None);
            }
            let h = b[..n].to_vec();
            self.pos += n;
            self.header_done = true;
            return Ok(Some(FlvItem::Header(h)));
        }
        if b.len() < TAG_HEADER {
            return Ok(None);
        }
        let kind = b[0] & 0x1f;
        if !matches!(kind, 8 | 9 | 18) {
            return Err(format!("unknown FLV tag type {kind}"));
        }
        let size = u32::from_be_bytes([0, b[1], b[2], b[3]]) as usize;
        if size > MAX_TAG {
            return Err(format!(
                "FLV tag of {size} bytes is over the {MAX_TAG}-byte limit"
            ));
        }
        if b[8..11] != [0, 0, 0] {
            return Err("FLV tag with a nonzero stream id".into());
        }
        let n = TAG_HEADER + size + 4;
        if b.len() < n {
            return Ok(None);
        }
        let prev = u32::from_be_bytes([b[n - 4], b[n - 3], b[n - 2], b[n - 1]]) as usize;
        if prev != TAG_HEADER + size {
            return Err(format!(
                "FLV previous-tag-size {prev} doesn't match the tag ({})",
                TAG_HEADER + size
            ));
        }
        let dts_ms = u32::from_be_bytes([b[7], b[4], b[5], b[6]]);
        let bytes = b[..n].to_vec();
        self.pos += n;
        Ok(Some(FlvItem::Packet(EncoderPacket { dts_ms, bytes })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> Vec<u8> {
        let mut h = b"FLV\x01\x05\x00\x00\x00\x09".to_vec();
        h.extend_from_slice(&[0, 0, 0, 0]);
        h
    }

    fn tag(kind: u8, dts: u32, body: &[u8]) -> Vec<u8> {
        let s = body.len() as u32;
        let d = dts.to_be_bytes();
        let mut t = vec![kind, (s >> 16) as u8, (s >> 8) as u8, s as u8];
        t.extend_from_slice(&[d[1], d[2], d[3], d[0], 0, 0, 0]);
        t.extend_from_slice(body);
        t.extend_from_slice(&(TAG_HEADER as u32 + s).to_be_bytes());
        t
    }

    fn drain(r: &mut FlvReader) -> Vec<FlvItem> {
        let mut v = Vec::new();
        while let Some(i) = r.next_item().unwrap() {
            v.push(i);
        }
        v
    }

    #[test]
    fn splits_a_stream_into_header_and_tags_byte_for_byte() {
        let mut stream = header();
        let tags = [
            tag(18, 0, b"onMetaData"),
            tag(9, 0, &[0x17, 0, 0, 0, 0, 1, 2]),
            tag(8, 23, &[0xaf, 1, 9]),
            tag(9, 0x0123_4567, &[0x27, 1]),
        ];
        for t in &tags {
            stream.extend_from_slice(t);
        }
        // Feed one byte at a time: nothing may come out early or torn.
        let mut r = FlvReader::new();
        let mut items = Vec::new();
        for b in &stream {
            r.push(std::slice::from_ref(b));
            items.extend(drain(&mut r));
        }
        assert_eq!(items.len(), 5);
        assert_eq!(items[0], FlvItem::Header(header()));
        let mut out = header();
        let mut dts = Vec::new();
        for (i, t) in items[1..].iter().zip(&tags) {
            let FlvItem::Packet(p) = i else {
                panic!("expected a packet, got {i:?}")
            };
            assert_eq!(&p.bytes, t);
            out.extend_from_slice(&p.bytes);
            dts.push(p.dts_ms);
        }
        assert_eq!(out, stream);
        assert_eq!(dts, [0, 0, 23, 0x0123_4567]);
    }

    #[test]
    fn rejects_bytes_that_are_not_flv() {
        let mut r = FlvReader::new();
        r.push(b"RIFF....WAVE");
        assert!(r.next_item().unwrap_err().contains("FLV signature"));
        let mut r = FlvReader::new();
        r.push(&header());
        r.push(&tag(7, 0, b"x"));
        assert_eq!(r.next_item().unwrap(), Some(FlvItem::Header(header())));
        assert!(r.next_item().unwrap_err().contains("tag type 7"));
    }

    /// A corrupt size is refused from the 11-byte header alone, before
    /// anything is buffered for it.
    #[test]
    fn refuses_an_oversized_tag_without_waiting_for_its_body() {
        let mut r = FlvReader::new();
        r.push(&header());
        let s = (MAX_TAG + 1) as u32;
        r.push(&[
            9,
            (s >> 16) as u8,
            (s >> 8) as u8,
            s as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ]);
        assert_eq!(r.next_item().unwrap(), Some(FlvItem::Header(header())));
        let e = r.next_item().unwrap_err();
        assert!(e.contains("over the 8388608-byte limit"), "{e}");
        // the limit itself is allowed (it waits for the body)
        let mut r = FlvReader::new();
        r.push(&header());
        let s = MAX_TAG as u32;
        r.push(&[
            9,
            (s >> 16) as u8,
            (s >> 8) as u8,
            s as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ]);
        r.next_item().unwrap();
        assert_eq!(r.next_item(), Ok(None));
    }

    #[test]
    fn refuses_a_nonzero_stream_id_and_a_wrong_previous_tag_size() {
        let mut bad_id = tag(9, 0, &[0x17, 1]);
        bad_id[10] = 1;
        let mut bad_prev = tag(9, 0, &[0x17, 1]);
        *bad_prev.last_mut().unwrap() += 1;
        for (t, want) in [
            (bad_id, "nonzero stream id"),
            (bad_prev, "previous-tag-size 14"),
        ] {
            let mut r = FlvReader::new();
            r.push(&header());
            r.push(&t);
            r.next_item().unwrap();
            let e = r.next_item().unwrap_err();
            assert!(e.contains(want), "{e}");
        }
    }
}
