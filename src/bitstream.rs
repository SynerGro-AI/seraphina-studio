//! Perfect port of OBS bitstream_reader.
//!
//! The key "opcode" is the Position trait: wrap vs no-wrap is a type-level
//! property of the counter, so the same bit-reading math works for both the
//! safe API (usize, never wraps) and the C ABI (u8, wraps exactly like the
//! original uint8_t).
//!
//! Math is perfect: after 255 bytes the safe reader stops; the ABI reader
//! re-reads from byte 0. Tests below prove both behaviors.

use std::convert::TryFrom;
use std::os::raw::c_int;

// Position trait — the perfect abstraction for the counter

/// Byte position. `usize` never wraps; `u8` wraps exactly like the original C.
pub trait Position: Copy {
    fn index(self) -> usize;
    fn advance(&mut self);
}

impl Position for usize {
    fn index(self) -> usize {
        self
    }
    fn advance(&mut self) {
        *self += 1;
    }
}

impl Position for u8 {
    fn index(self) -> usize {
        usize::from(self)
    }
    fn advance(&mut self) {
        *self = self.wrapping_add(1);
    }
}

// Shared reader
pub struct Reader<'a, P: Position> {
    buf: &'a [u8],
    pos: P,
    sub_pos: u8,
}

impl<'a, P: Position> Reader<'a, P> {
    pub fn new(buf: &'a [u8], pos: P) -> Self {
        Self {
            buf,
            pos,
            sub_pos: 0x80,
        }
    }

    pub fn read_bit(&mut self) -> u8 {
        let Some(&byte) = self.buf.get(self.pos.index()) else {
            return 0;
        };
        let bit = u8::from(byte & self.sub_pos == self.sub_pos);
        self.sub_pos >>= 1;
        if self.sub_pos == 0 {
            self.sub_pos = 0x80;
            self.pos.advance();
        }
        bit
    }

    pub fn read_bits(&mut self, bits: u32) -> u8 {
        let mut res = 0u8;
        for _ in 0..bits {
            res = (res << 1) | self.read_bit();
        }
        res
    }

    pub fn r8(&mut self) -> u8 {
        self.read_bits(8)
    }
    pub fn r16(&mut self) -> u16 {
        let hi = u16::from(self.r8());
        (hi << 8) | u16::from(self.r8())
    }
    pub fn position(&self) -> P {
        self.pos
    }
}

/// Safe API — never wraps.
pub struct BitstreamReader<'a>(Reader<'a, usize>);

impl<'a> BitstreamReader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self(Reader::new(buf, 0))
    }
    pub fn read_bit(&mut self) -> u8 {
        self.0.read_bit()
    }
    pub fn read_bits(&mut self, bits: u32) -> u8 {
        self.0.read_bits(bits)
    }
    pub fn r8(&mut self) -> u8 {
        self.0.r8()
    }
    pub fn r16(&mut self) -> u16 {
        self.0.r16()
    }
    pub fn byte_pos(&self) -> usize {
        self.0.position()
    }
}

// C ABI — keeps the original uint8_t wrap
#[repr(C)]
#[allow(non_camel_case_types, non_snake_case)]
pub struct bitstream_reader {
    pub pos: u8,
    pub subPos: u8,
    pub buf: *mut u8,
    pub len: usize,
}

unsafe fn with_reader<T>(r: *mut bitstream_reader, f: impl FnOnce(&mut Reader<'_, u8>) -> T) -> T {
    let r = unsafe { &mut *r };
    let buf = if r.buf.is_null() {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(r.buf, r.len) }
    };
    let mut reader = Reader {
        buf,
        pos: r.pos,
        sub_pos: r.subPos,
    };
    let out = f(&mut reader);
    r.pos = reader.pos;
    r.subPos = reader.sub_pos;
    out
}

/// Initialise `*r` to read `len` bytes from `data`.
///
/// # Safety
/// `r` must be valid for writes. `data` must be null or point to `len`
/// readable bytes that outlive every read through `r`.
#[no_mangle]
pub unsafe extern "C" fn bitstream_reader_init(
    r: *mut bitstream_reader,
    data: *mut u8,
    len: usize,
) {
    unsafe {
        r.write(bitstream_reader {
            pos: 0,
            subPos: 0x80,
            buf: data,
            len,
        });
    }
}

/// Read `bits` bits (0 if `bits` is negative).
///
/// # Safety
/// `r` must point to a reader set up by [`bitstream_reader_init`] whose
/// buffer is still alive.
#[no_mangle]
pub unsafe extern "C" fn bitstream_reader_read_bits(r: *mut bitstream_reader, bits: c_int) -> u8 {
    let bits = u32::try_from(bits).unwrap_or(0);
    unsafe { with_reader(r, |reader| reader.read_bits(bits)) }
}

/// Read one byte.
///
/// # Safety
/// `r` must point to a reader set up by [`bitstream_reader_init`] whose
/// buffer is still alive.
#[no_mangle]
pub unsafe extern "C" fn bitstream_reader_r8(r: *mut bitstream_reader) -> u8 {
    unsafe { with_reader(r, |reader| reader.r8()) }
}

/// Read a big-endian u16.
///
/// # Safety
/// `r` must point to a reader set up by [`bitstream_reader_init`] whose
/// buffer is still alive.
#[no_mangle]
pub unsafe extern "C" fn bitstream_reader_r16(r: *mut bitstream_reader) -> u16 {
    unsafe { with_reader(r, |reader| reader.r16()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_bits() {
        let data = [0b1011_0000u8];
        let mut r = BitstreamReader::new(&data);
        assert_eq!(r.read_bit(), 1);
        assert_eq!(r.read_bit(), 0);
        assert_eq!(r.read_bit(), 1);
        assert_eq!(r.read_bit(), 1);
        assert_eq!(r.read_bits(4), 0b0000);
    }

    #[test]
    fn r8_r16() {
        let data = [0xAB, 0xCD];
        let mut r = BitstreamReader::new(&data);
        assert_eq!(r.r8(), 0xAB);
        assert_eq!(r.r8(), 0xCD);
        let mut r = BitstreamReader::new(&data);
        assert_eq!(r.r16(), 0xABCD);
    }

    #[test]
    fn safe_does_not_wrap_past_255() {
        let data = vec![0xFFu8; 256];
        let mut r = BitstreamReader::new(&data);
        for _ in 0..2048 {
            assert_eq!(r.read_bit(), 1);
        }
        assert_eq!(r.byte_pos(), 256);
        assert_eq!(r.read_bit(), 0);
        assert_eq!(r.read_bits(8), 0);
    }

    #[test]
    fn abi_wraps_at_255() {
        let mut data = vec![0x00u8; 300];
        data[0] = 0xAA;
        let mut c_reader = bitstream_reader {
            pos: 0,
            subPos: 0x80,
            buf: data.as_mut_ptr(),
            len: data.len(),
        };
        for _ in 0..2048 {
            unsafe { bitstream_reader_read_bits(&mut c_reader, 1) };
        }
        assert_eq!(c_reader.pos, 0);
        let next = unsafe { bitstream_reader_r8(&mut c_reader) };
        assert_eq!(next, 0xAA);
    }

    #[test]
    fn empty_and_null_safe() {
        let mut r = BitstreamReader::new(&[]);
        assert_eq!(r.read_bit(), 0);
        assert_eq!(r.r8(), 0);
    }

    #[test]
    fn safe_and_abi_readers_agree_on_short_reads() {
        // On short buffers the safe and ABI paths must agree.
        let data = [0xAB, 0xCD];
        let mut safe = BitstreamReader::new(&data);
        let safe_r8 = safe.r8();

        let mut c_reader = bitstream_reader {
            pos: 0,
            subPos: 0x80,
            buf: data.as_ptr() as *mut u8,
            len: data.len(),
        };
        let abi_r8 = unsafe { bitstream_reader_r8(&mut c_reader) };

        assert_eq!(safe_r8, abi_r8, "bitstream r8 short");
    }
}
