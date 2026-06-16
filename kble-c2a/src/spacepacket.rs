use anyhow::{anyhow, Result};
use bytes::{Bytes, BytesMut};

const TC_TF_PH_SIZE: usize = 5;
const TC_SEG_HDR_SIZE: usize = 1;
const TC_TF_FECF_SIZE: usize = 2;
pub fn from_tc_tf(mut tc_tf: Bytes) -> Result<Bytes> {
    if tc_tf.len() < TC_TF_PH_SIZE + TC_SEG_HDR_SIZE + TC_TF_FECF_SIZE {
        return Err(anyhow!("TC Transfer Frame is too short: {tc_tf:02x}"));
    }
    let _ = tc_tf.split_off(tc_tf.len() - TC_TF_FECF_SIZE);
    let _ = tc_tf.split_to(TC_TF_PH_SIZE + TC_SEG_HDR_SIZE);
    Ok(tc_tf)
}

const AOS_TF_SIZE: usize = 444;
const AOS_TF_PH_SIZE: usize = 6;
const M_PDU_HDR_SIZE: usize = 2;
const AOS_TF_CLCW_SIZE: usize = 4;
// M_PDU Data Zone = AOS_TF_SIZE - PH - M_PDU_Hdr - CLCW
const M_PDU_DATA_ZONE_SIZE: usize =
    AOS_TF_SIZE - AOS_TF_PH_SIZE - M_PDU_HDR_SIZE - AOS_TF_CLCW_SIZE;
// CCSDS 133.0-B-1: Space Packet min = 6B Primary Header + 1B Data Field
const MIN_SPACE_PACKET_SIZE: usize = 7;
// Max SP bytes that fit in one Data Zone while still leaving room for a minimum Idle Packet
const MAX_SP_IN_FRAME_WITH_IDLE: usize = M_PDU_DATA_ZONE_SIZE - MIN_SPACE_PACKET_SIZE;
// CCSDS 732.0-B-4 §4.1.4.4.4: FHP value indicating no new packet header in this M_PDU
const FHP_NO_HDR: u16 = 0x07FF;

// Version Number = 0b01, SCID = 0, VCID = 0
const AOS_TF_PH_VN_SCID_VCID: [u8; 2] = [0x40, 0x00];
const AOS_TF_CLCW: [u8; 4] = [0x00, 0x00, 0x00, 0x00];

#[allow(clippy::unusual_byte_groupings)]
const IDLE_PACKET_PH_EXCEPT_LEN: [u8; 4] = [
    // Version=000, Type=0 (TLM), Sec Hdr Flag=0, APID=0x7FF (all ones)
    0b000_0_0_111,
    0b1111_1111,
    // Seq Flags=11 (unsegmented), Seq Count=0
    0b11_000000,
    0,
];

/// Build a CCSDS-compliant Idle Packet (CCSDS 133.0-B-1, APID = 0x7FF) of exactly
/// `idle_len` bytes. `idle_len` must be >= `MIN_SPACE_PACKET_SIZE` (7).
fn build_idle_packet(idle_len: usize) -> BytesMut {
    debug_assert!(idle_len >= MIN_SPACE_PACKET_SIZE);
    let data_field_len = idle_len - 6; // subtract 4B PH_EXCEPT_LEN + 2B length field
    let mut buf = BytesMut::with_capacity(idle_len);
    buf.extend_from_slice(&IDLE_PACKET_PH_EXCEPT_LEN);
    // PacketDataLength = (Data Field length) - 1  (CCSDS 133.0-B-1 §4.1.3.5.4)
    buf.extend_from_slice(&((data_field_len - 1) as u16).to_be_bytes());
    buf.extend(std::iter::repeat_n(0u8, data_field_len));
    debug_assert_eq!(buf.len(), idle_len);
    buf
}

/// Assemble a complete 444-byte AOS Transfer Frame from a pre-built 432-byte Data Zone.
/// Advances `frame_count` by one.
fn assemble_tf(frame_count: &mut u32, fhp: u16, data_zone: &[u8]) -> BytesMut {
    debug_assert_eq!(data_zone.len(), M_PDU_DATA_ZONE_SIZE);
    let mut tf = BytesMut::with_capacity(AOS_TF_SIZE);
    // AOS TF Primary Header (CCSDS 732.0-B-4 §4.1.2)
    tf.extend_from_slice(&AOS_TF_PH_VN_SCID_VCID);
    tf.extend_from_slice(&(*frame_count << 8).to_be_bytes());
    // M_PDU Header: First Header Pointer (CCSDS 732.0-B-4 §4.1.4.4)
    tf.extend_from_slice(&fhp.to_be_bytes());
    // M_PDU Data Zone
    tf.extend_from_slice(data_zone);
    // CLCW
    tf.extend_from_slice(&AOS_TF_CLCW);
    debug_assert_eq!(tf.len(), AOS_TF_SIZE);
    *frame_count = frame_count.wrapping_add(1);
    tf
}

/// Wrap one Space Packet into one or more AOS Transfer Frames with M_PDU encapsulation,
/// implementing CCSDS 732.0-B-4 §4.1.4 packet spanning via First Header Pointer.
///
/// Each output frame is exactly `AOS_TF_SIZE` (444) bytes. When the Space Packet exceeds
/// the M_PDU Data Zone capacity (432 bytes), it is fragmented across multiple frames.
/// The final frame always contains a trailing Idle Packet to fill the Data Zone.
/// `frame_count` is incremented once per emitted frame.
pub fn to_aos_tfs(frame_count: &mut u32, spacepacket: Bytes) -> Result<Vec<BytesMut>> {
    let l = spacepacket.len();
    if l < MIN_SPACE_PACKET_SIZE {
        return Err(anyhow!("Space Packet is too short: {l} bytes"));
    }

    let mut frames = Vec::new();

    // Case A: SP fits in a single Data Zone alongside a trailing Idle Packet
    if l <= MAX_SP_IN_FRAME_WITH_IDLE {
        let idle_len = M_PDU_DATA_ZONE_SIZE - l;
        let mut dz = BytesMut::with_capacity(M_PDU_DATA_ZONE_SIZE);
        dz.extend_from_slice(&spacepacket);
        dz.extend_from_slice(&build_idle_packet(idle_len));
        frames.push(assemble_tf(frame_count, 0, &dz));
        return Ok(frames);
    }

    let r = l % M_PDU_DATA_ZONE_SIZE;
    // Pathological: remainder would leave < 7 bytes for a minimum Idle Packet in the tail frame.
    // Resolved by prepending a minimum Idle Packet to the first frame, which shifts the SP start
    // by 7 bytes and moves the tail remainder from [426..=431] into [1..=6].
    let pathological = r > MAX_SP_IN_FRAME_WITH_IDLE; // r ∈ [426..=431]

    let mut sp_offset = 0usize;

    if pathological {
        // Case C: TF0 = [Idle(7) | SP[0..425]], FHP = 0  (Idle header is first new header)
        let sp_in_tf0 = MAX_SP_IN_FRAME_WITH_IDLE;
        let mut dz = BytesMut::with_capacity(M_PDU_DATA_ZONE_SIZE);
        dz.extend_from_slice(&build_idle_packet(MIN_SPACE_PACKET_SIZE));
        dz.extend_from_slice(&spacepacket[..sp_in_tf0]);
        debug_assert_eq!(dz.len(), M_PDU_DATA_ZONE_SIZE);
        frames.push(assemble_tf(frame_count, 0, &dz));
        sp_offset = sp_in_tf0;
    }

    // Case B (and the continuation chain after Case C TF0):
    // Emit 432-byte SP chunks with FHP = 0 (first chunk, Case B only) or FHP_NO_HDR (continuation).
    // The final partial chunk is paired with a trailing Idle Packet.
    loop {
        let remaining = l - sp_offset;

        if remaining == 0 {
            // SP ended exactly on a Data Zone boundary: emit a dedicated Idle-only frame.
            let dz = build_idle_packet(M_PDU_DATA_ZONE_SIZE);
            frames.push(assemble_tf(frame_count, 0, &dz));
            break;
        } else if remaining > MAX_SP_IN_FRAME_WITH_IDLE {
            // Emit a full 432-byte SP chunk.
            // FHP = 0 for the very first chunk (new SP header at offset 0, Case B only);
            // FHP_NO_HDR for all subsequent chunks (SP header was in an earlier frame).
            let fhp = if sp_offset == 0 { 0u16 } else { FHP_NO_HDR };
            let chunk_end = sp_offset + M_PDU_DATA_ZONE_SIZE;
            debug_assert!(chunk_end <= l, "chunk_end must not exceed SP length here");
            let mut dz = BytesMut::with_capacity(M_PDU_DATA_ZONE_SIZE);
            dz.extend_from_slice(&spacepacket[sp_offset..chunk_end]);
            frames.push(assemble_tf(frame_count, fhp, &dz));
            sp_offset = chunk_end;
        } else {
            // Final partial chunk: SP fragment + trailing Idle Packet.
            // FHP points to the Idle Packet header, which is the first new header in this frame
            // (the SP header was in an earlier frame, so it is not "new" here).
            let idle_len = M_PDU_DATA_ZONE_SIZE - remaining;
            let fhp = remaining as u16;
            let mut dz = BytesMut::with_capacity(M_PDU_DATA_ZONE_SIZE);
            dz.extend_from_slice(&spacepacket[sp_offset..l]);
            dz.extend_from_slice(&build_idle_packet(idle_len));
            frames.push(assemble_tf(frame_count, fhp, &dz));
            break;
        }
    }

    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `total_len`-byte CCSDS Space Packet with a valid Primary Header and a
    /// deterministic counter body. Used as test input without referencing any specific
    /// protocol implementation.
    ///
    /// Layout: PH[0..6) | body[6..total_len)
    /// - PH: version=0, type=0 (TLM), sec_hdr_flag=1, APID=0x042
    ///   seq_flags=11 (unsegmented), seq_count=0
    ///   PacketDataLength = total_len - 7  (= body_len - 1)
    fn make_sp(total_len: usize) -> Bytes {
        assert!(total_len >= MIN_SPACE_PACKET_SIZE, "total_len must be >= 7");
        let mut v = BytesMut::with_capacity(total_len);
        // Version(3b)=000, Type(1b)=0, SecHdrFlag(1b)=1, APID(11b)=0x042
        v.extend_from_slice(&[0x08, 0x42]);
        // SeqFlags(2b)=11, SeqCount(14b)=0
        v.extend_from_slice(&[0xC0, 0x00]);
        // PacketDataLength = total_len - 7
        v.extend_from_slice(&((total_len - 7) as u16).to_be_bytes());
        // Deterministic body: counter mod 256
        for i in 0..(total_len - 6) {
            v.extend_from_slice(&[(i & 0xFF) as u8]);
        }
        v.freeze()
    }

    /// Extract the M_PDU Data Zone (bytes [8..440)) from an AOS Transfer Frame.
    fn data_zone(tf: &BytesMut) -> &[u8] {
        assert_eq!(tf.len(), AOS_TF_SIZE);
        &tf[AOS_TF_PH_SIZE + M_PDU_HDR_SIZE..AOS_TF_PH_SIZE + M_PDU_HDR_SIZE + M_PDU_DATA_ZONE_SIZE]
    }

    /// Extract the First Header Pointer from an AOS Transfer Frame.
    fn fhp(tf: &BytesMut) -> u16 {
        u16::from_be_bytes([tf[AOS_TF_PH_SIZE], tf[AOS_TF_PH_SIZE + 1]])
    }

    /// Collect SP bytes from the M_PDU Data Zones and verify they match `expected`.
    ///
    /// `sp_offset_in_first_dz`: bytes to skip at the start of the first frame's Data Zone
    /// before SP bytes begin (0 for Case B; MIN_SPACE_PACKET_SIZE=7 for Case C
    /// where a prepended Idle Packet occupies the first 7 bytes of TF0).
    fn assert_sp_round_trip(frames: &[BytesMut], expected: &Bytes, sp_offset_in_first_dz: usize) {
        let total_sp = expected.len();
        let mut collected = BytesMut::with_capacity(total_sp);
        let mut sp_remaining = total_sp;

        for (i, tf) in frames.iter().enumerate() {
            if sp_remaining == 0 {
                break;
            }
            let dz = data_zone(tf);
            let start = if i == 0 { sp_offset_in_first_dz } else { 0 };
            let available = M_PDU_DATA_ZONE_SIZE - start;
            let take = sp_remaining.min(available);
            collected.extend_from_slice(&dz[start..start + take]);
            sp_remaining -= take;
        }

        assert_eq!(sp_remaining, 0, "SP not fully contained in provided frames");
        assert_eq!(
            collected.freeze(),
            *expected,
            "round-trip SP bytes mismatch"
        );
    }

    /// Common checks applied to every emitted frame list.
    fn assert_common(frames: &[BytesMut], expected_count: usize, initial_fc: u32) {
        assert_eq!(frames.len(), expected_count, "frame count mismatch");
        for (i, tf) in frames.iter().enumerate() {
            assert_eq!(tf.len(), AOS_TF_SIZE, "frame {i} has wrong size");
            // AOS TF PH: VN/SCID/VCID prefix
            assert_eq!(
                &tf[..2],
                &AOS_TF_PH_VN_SCID_VCID,
                "frame {i} has wrong PH prefix"
            );
            // CLCW at tail
            assert_eq!(&tf[440..444], &AOS_TF_CLCW, "frame {i} has wrong CLCW");
        }
        // frame_count advances by one per emitted frame
        // (verified by the caller, which passes the resulting frame_count value)
        let _ = initial_fc;
    }

    /// Verify that `slice` (of exactly `idle_len` bytes) is a CCSDS 133.0-B-1 Idle Packet.
    fn assert_idle_slice_valid(slice: &[u8]) {
        let idle_len = slice.len();
        assert!(
            idle_len >= MIN_SPACE_PACKET_SIZE,
            "Idle Packet too short: {idle_len} bytes"
        );
        let apid = u16::from_be_bytes([slice[0] & 0x07, slice[1]]);
        assert_eq!(apid, 0x07FF, "Idle Packet APID mismatch (got {apid:#06x})");
        let pkt_data_len = u16::from_be_bytes([slice[4], slice[5]]) as usize;
        // PacketDataLength = (Data Field length) - 1  (CCSDS 133.0-B-1 §4.1.3.5.4)
        assert_eq!(
            pkt_data_len,
            idle_len - 7,
            "Idle Packet DataLength field: got {pkt_data_len}, want {}",
            idle_len - 7
        );
    }

    /// Verify that the Idle Packet occupying `dz[sp_bytes_in_dz..]` is CCSDS compliant.
    /// Use this for frames where the Idle Packet fills the entire remainder of the Data Zone.
    fn assert_idle_packet_valid(tf: &BytesMut, sp_bytes_in_dz: usize) {
        let dz = data_zone(tf);
        assert_idle_slice_valid(&dz[sp_bytes_in_dz..]);
    }

    // ── Case A (single frame, various sizes) ──────────────────────────────────────

    #[test]
    fn sp_min_size() {
        let sp = make_sp(7);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 1, 0);
        assert_eq!(fc, 1);
        assert_eq!(fhp(&frames[0]), 0);
        assert_idle_packet_valid(&frames[0], 7);
        assert_sp_round_trip(&frames, &sp, 0);
    }

    #[test]
    fn sp_event_class() {
        let sp = make_sp(44);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 1, 0);
        assert_eq!(fc, 1);
        assert_eq!(fhp(&frames[0]), 0);
        assert_idle_packet_valid(&frames[0], 44);
        assert_sp_round_trip(&frames, &sp, 0);
    }

    #[test]
    fn sp_hk_small() {
        // Also used as the bit-identical golden test (see sp_bit_identical_to_legacy below)
        let sp = make_sp(136);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 1, 0);
        assert_eq!(fc, 1);
        assert_eq!(fhp(&frames[0]), 0);
        assert_idle_packet_valid(&frames[0], 136);
        assert_sp_round_trip(&frames, &sp, 0);
    }

    #[test]
    fn sp_single_frame_boundary() {
        // L = 425: Idle Packet data field = 1 byte (CCSDS minimum)
        let sp = make_sp(MAX_SP_IN_FRAME_WITH_IDLE);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 1, 0);
        assert_eq!(fc, 1);
        assert_eq!(fhp(&frames[0]), 0);
        assert_idle_packet_valid(&frames[0], MAX_SP_IN_FRAME_WITH_IDLE);
        // Idle data field is exactly 1 byte (PacketDataLength = 0)
        let dz = data_zone(&frames[0]);
        assert_eq!(dz[MAX_SP_IN_FRAME_WITH_IDLE + 4 + 1], 0x00); // low byte of len = 0 → data field 1B
        assert_sp_round_trip(&frames, &sp, 0);
    }

    /// Verify that frames for SP ≤ 425 bytes are byte-for-byte identical to the output
    /// of the previous single-frame implementation.
    #[test]
    fn sp_bit_identical_to_legacy() {
        let sp = make_sp(136);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_eq!(frames.len(), 1);

        // Reconstruct the expected frame manually using the legacy algorithm's logic:
        //   PH(6) | M_PDU_Hdr(FHP=0, 2) | SP(136) | IDLE_PH_EXCEPT_LEN(4) |
        //   IdleLen(2, value=289) | IdleData(290×0x00) | CLCW(4)
        let idle_data_len: usize = 426 - 136; // = 290
        let mut expected = BytesMut::new();
        expected.extend_from_slice(&AOS_TF_PH_VN_SCID_VCID);
        expected.extend_from_slice(&(0u32 << 8u32).to_be_bytes());
        expected.extend_from_slice(&0u16.to_be_bytes()); // FHP = 0
        expected.extend_from_slice(&sp);
        expected.extend_from_slice(&IDLE_PACKET_PH_EXCEPT_LEN);
        expected.extend_from_slice(&((idle_data_len - 1) as u16).to_be_bytes());
        expected.extend(std::iter::repeat_n(0u8, idle_data_len));
        expected.extend_from_slice(&AOS_TF_CLCW);

        assert_eq!(
            frames[0], expected,
            "output must be byte-for-byte identical to legacy"
        );
    }

    // ── Case B (multi-frame, non-pathological) ────────────────────────────────────

    #[test]
    fn sp_exact_data_zone() {
        // L = 432: SP fills Data Zone exactly → extra Idle-only TF
        let sp = make_sp(M_PDU_DATA_ZONE_SIZE);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 2, 0);
        assert_eq!(fc, 2);
        // TF0: full SP, FHP = 0
        assert_eq!(fhp(&frames[0]), 0);
        assert_eq!(data_zone(&frames[0]), sp.as_ref());
        // TF1: Idle-only, FHP = 0
        assert_eq!(fhp(&frames[1]), 0);
        assert_idle_packet_valid(&frames[1], 0);
        assert_sp_round_trip(&frames, &sp, 0);
    }

    #[test]
    fn sp_just_over_boundary() {
        // L = 433: 432 + 1
        let sp = make_sp(433);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 2, 0);
        assert_eq!(fc, 2);
        assert_eq!(fhp(&frames[0]), 0);
        assert_eq!(fhp(&frames[1]), 1); // Idle Packet starts at offset 1
        assert_idle_packet_valid(&frames[1], 1);
        assert_sp_round_trip(&frames, &sp, 0);
    }

    #[test]
    fn sp_hk_large() {
        // L = 649: 432 + 217  (representative large-HK size)
        let sp = make_sp(649);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 2, 0);
        assert_eq!(fc, 2);
        assert_eq!(fhp(&frames[0]), 0);
        assert_eq!(fhp(&frames[1]), 649 - 432); // = 217
        assert_idle_packet_valid(&frames[1], 649 - 432);
        assert_sp_round_trip(&frames, &sp, 0);
    }

    #[test]
    fn sp_double_data_zone() {
        // L = 864 = 432 × 2: two full chunks + Idle-only TF
        let sp = make_sp(864);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 3, 0);
        assert_eq!(fc, 3);
        assert_eq!(fhp(&frames[0]), 0);
        assert_eq!(fhp(&frames[1]), FHP_NO_HDR);
        assert_eq!(fhp(&frames[2]), 0); // Idle-only
        assert_idle_packet_valid(&frames[2], 0);
        assert_sp_round_trip(&frames, &sp, 0);
    }

    #[test]
    fn sp_memory_dump_like() {
        // L = 1024: 432 + 432 + 160  (memory/NVM dump class)
        let sp = make_sp(1024);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 3, 0);
        assert_eq!(fc, 3);
        assert_eq!(fhp(&frames[0]), 0);
        assert_eq!(fhp(&frames[1]), FHP_NO_HDR);
        assert_eq!(fhp(&frames[2]), 1024 - 2 * 432); // = 160
        assert_idle_packet_valid(&frames[2], 160);
        assert_sp_round_trip(&frames, &sp, 0);
    }

    #[test]
    fn sp_large_dump() {
        // L = 2048: 432 × 4 + 320  (large dump, verifies 4+ continuation frames)
        let sp = make_sp(2048);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 5, 0);
        assert_eq!(fc, 5);
        assert_eq!(fhp(&frames[0]), 0);
        for (i, frame) in frames.iter().enumerate().skip(1).take(3) {
            assert_eq!(fhp(frame), FHP_NO_HDR, "frame {i} should be continuation");
        }
        assert_eq!(fhp(&frames[4]), 2048 - 4 * 432); // = 320
        assert_sp_round_trip(&frames, &sp, 0);
    }

    // ── Case C (pathological: L mod 432 ∈ [426..=431]) ──────────────────────────

    #[test]
    fn sp_pathological_lower() {
        // L = 858: 858 mod 432 = 426 (lower end of pathological band)
        let sp = make_sp(858);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 3, 0);
        assert_eq!(fc, 3);
        // TF0: [Idle(7) | SP[0..425]], FHP = 0
        assert_eq!(fhp(&frames[0]), 0);
        let dz0 = data_zone(&frames[0]);
        // First 7 bytes must be a valid minimum Idle Packet
        assert_idle_slice_valid(&dz0[..MIN_SPACE_PACKET_SIZE]);
        // Remaining bytes [7..432) must be SP[0..425]
        assert_eq!(&dz0[MIN_SPACE_PACKET_SIZE..], &sp[..425]);
        // TF1: SP[425..857], FHP = FHP_NO_HDR
        assert_eq!(fhp(&frames[1]), FHP_NO_HDR);
        assert_eq!(data_zone(&frames[1]), &sp[425..857]);
        // TF2: SP[857..858] + Idle(431), FHP = 1
        assert_eq!(fhp(&frames[2]), 1);
        assert_idle_packet_valid(&frames[2], 1);
        assert_sp_round_trip(&frames, &sp, MIN_SPACE_PACKET_SIZE);
    }

    #[test]
    fn sp_pathological_upper() {
        // L = 863: 863 mod 432 = 431 (upper end of pathological band)
        let sp = make_sp(863);
        let mut fc = 0u32;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_common(&frames, 3, 0);
        assert_eq!(fc, 3);
        assert_eq!(fhp(&frames[0]), 0);
        assert_eq!(fhp(&frames[1]), FHP_NO_HDR);
        let tail_sp_bytes = 863 - 425 - 432; // = 6
        assert_eq!(fhp(&frames[2]), tail_sp_bytes as u16);
        assert_idle_packet_valid(&frames[2], tail_sp_bytes);
        assert_sp_round_trip(&frames, &sp, MIN_SPACE_PACKET_SIZE);
    }

    // ── Edge cases ────────────────────────────────────────────────────────────────

    #[test]
    fn sp_too_short_is_rejected() {
        let sp = Bytes::from(vec![0u8; 6]); // 6 bytes: below MIN_SPACE_PACKET_SIZE
        let mut fc = 0u32;
        let err = to_aos_tfs(&mut fc, sp).unwrap_err();
        assert!(
            err.to_string().contains("too short"),
            "unexpected error message: {err}"
        );
        assert_eq!(fc, 0, "frame_count must not advance on error");
    }

    #[test]
    fn frame_count_wraps() {
        // frame_count = u32::MAX with a 2-frame SP: should wrap to 1 afterwards
        let sp = make_sp(649);
        let mut fc = u32::MAX;
        let frames = to_aos_tfs(&mut fc, sp.clone()).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(fc, 1, "frame_count should have wrapped from u32::MAX to 1");
    }
}
