use anyhow::Result;
use bytes::{Buf, Bytes, BytesMut};
use tokio_util::codec::Decoder;
use tracing::warn;

const FRAME_SIZE: usize = 444;
// Version Number: 2'b01
// Rest          : 6'bXXXXXX
const HEADER_MASK: u8 = 0b1100_0000;
const HEADER_PATTERN: u8 = 0b0100_0000;
// Control Word Type: 1'b1
// CLCW Version     : 2'b00
// Status Field     : 3'bXXX
// COP in Effect    : 2'b01
// VCID             : 6'b000000
// Spare            : 2'b00
const TRAILER_SIZE: usize = 4;
const TRAILER_MASK: [u8; 2] = [0b1110_0011, 0b1111_1111];
const TRAILER_PATTERN: [u8; 2] = [0b00000001, 0b00000000];

#[derive(Debug, Default)]
pub struct AosTransferFrameCodec {
    buf: BytesMut,
}

impl AosTransferFrameCodec {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AosTransferFrameCodec {
    fn find_primary_header(&self) -> Option<usize> {
        self.buf
            .iter()
            .position(|b| *b & HEADER_MASK == HEADER_PATTERN)
    }

    fn is_trailer_matched(&self) -> bool {
        let trailer_pos = FRAME_SIZE - TRAILER_SIZE;
        let trailer_bytes = [self.buf[trailer_pos], self.buf[trailer_pos + 1]];
        trailer_bytes
            .iter()
            .zip(TRAILER_MASK.iter().zip(TRAILER_PATTERN))
            .all(|(b, (mask, pattern))| b & mask == pattern)
    }
}

impl Decoder for AosTransferFrameCodec {
    type Item = Bytes;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        self.buf.extend_from_slice(src);
        src.clear();
        if self.buf.is_empty() {
            return Ok(None);
        }
        while let Some(ph_pos) = self.find_primary_header() {
            if ph_pos > 0 {
                warn!("Leading junk data: {:02x?}", &self.buf[..ph_pos]);
                self.buf.advance(ph_pos);
            }
            if self.buf.len() < FRAME_SIZE {
                // insufficient buffer
                return Ok(None);
            }
            if self.is_trailer_matched() {
                let frame = self.buf.split_to(FRAME_SIZE);
                return Ok(Some(frame.into()));
            } else {
                warn!("Trailer mismatched: {:02x?}", &self.buf[..FRAME_SIZE]);
                self.buf.advance(1);
            }
        }
        warn!("No primary header found in {} bytes", self.buf.len());
        self.buf.clear();
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;

    use super::*;
    const TRANSFER_FRAME: [u8; 444] = [
        0x54, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0d, 0x10, 0xc0, 0x00, 0x01, 0xa9, 0x00,
        0x00, 0x00, 0x00, 0x0b, 0xf0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x0b, 0x3f, 0xf1, 0xb2, 0x2d, 0x0e, 0x56, 0x04,
        0x19, 0x01, 0x01, 0x00, 0x28, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02,
        0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0xa0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x64, 0x73, 0x64, 0x64, 0x64, 0x64,
        0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x64, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    ];

    #[test]
    fn test_complete_frame() {
        let mut codec = AosTransferFrameCodec::new();
        let mut complete_bytes = BytesMut::from(&TRANSFER_FRAME[..]);
        let actual = codec.decode(&mut complete_bytes).unwrap().unwrap();
        assert_eq!(actual, TRANSFER_FRAME.as_slice());
        let actual = codec.decode(&mut BytesMut::new()).unwrap();
        assert_eq!(actual, None);
    }

    #[test]
    fn test_incomplete_frame() {
        let mut codec = AosTransferFrameCodec::new();
        let mut tail = BytesMut::from(&TRANSFER_FRAME[..]);
        let mut head = tail.split_to(220);
        assert_eq!(codec.decode(&mut head).unwrap(), None);
        assert_eq!(
            codec.decode(&mut tail).unwrap().unwrap(),
            TRANSFER_FRAME.as_slice()
        );
        assert_eq!(codec.decode(&mut BytesMut::new()).unwrap(), None);
    }

    #[test]
    fn test_contiguous_frame() {
        let mut codec = AosTransferFrameCodec::new();
        let mut double_frames = BytesMut::from(&TRANSFER_FRAME[..]);
        double_frames.extend_from_slice(&TRANSFER_FRAME[..]);
        let actual = codec.decode(&mut double_frames).unwrap().unwrap();
        assert_eq!(actual, TRANSFER_FRAME.as_slice());
        let actual = codec.decode(&mut BytesMut::new()).unwrap().unwrap();
        assert_eq!(actual, TRANSFER_FRAME.as_slice());
        let actual = codec.decode(&mut BytesMut::new()).unwrap();
        assert_eq!(actual, None);
    }

    #[test]
    fn test_leading_junk_data() {
        let mut codec = AosTransferFrameCodec::new();
        let mut input = BytesMut::from(&b"JUNKDATA"[..]);
        input.extend_from_slice(&TRANSFER_FRAME[..]);
        assert_eq!(
            codec.decode(&mut input).unwrap().unwrap(),
            TRANSFER_FRAME.as_slice()
        );
        assert_eq!(codec.decode(&mut BytesMut::new()).unwrap(), None);
    }
}
