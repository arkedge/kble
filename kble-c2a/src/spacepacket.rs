use anyhow::{anyhow, Result};
use bytes::{Bytes, BytesMut};

const TC_TF_PH_SIZE: usize = 5;
const TC_SEG_HDR_SIZE: usize = 1;
const TC_TF_FECF_SIZE: usize = 2;
pub fn from_tc_tf(mut tc_tf: Bytes) -> Result<Bytes> {
    if tc_tf.len() < TC_TF_PH_SIZE + TC_SEG_HDR_SIZE + TC_TF_FECF_SIZE {
        return Err(anyhow!("TC Transfer Frame is too short: {:02x}", tc_tf));
    }
    let _ = tc_tf.split_off(tc_tf.len() - TC_TF_FECF_SIZE);
    let _ = tc_tf.split_to(TC_TF_PH_SIZE + TC_SEG_HDR_SIZE);
    Ok(tc_tf)
}

const AOS_TF_SIZE: usize = 444;
// Version Number = 0b01
// SCID = 0b0000_0000
// VCID = 0b00_0000
const AOS_TF_PH_VN_SCID_VCID: [u8; 2] = [0x40, 0x00];
#[allow(clippy::unusual_byte_groupings)]
const IDLE_PACKET_PH_EXCEPT_LEN: [u8; 4] = [
    // Version = 0b000
    // Type = 0b0 (telemetry)
    // SH Flag = 0b0
    // APID = 0b111_1111_1111
    0b000_0_0_111,
    0b1111_1111,
    // Seq Flag = 0b11
    // Seq Count = 0
    0b11_000000,
    0,
];
const IDLE_PACKET_PH_LEN_SIZE: usize = 2;
const AOS_TF_CLCW: [u8; 4] = [0x00, 0x00, 0x00, 0x00];
const AOS_TF_MAX_PACKET_SIZE: usize = AOS_TF_SIZE - 12;
pub fn to_aos_tf(frame_count: &mut u32, spacepacket: Bytes) -> Result<BytesMut> {
    if spacepacket.len() > AOS_TF_MAX_PACKET_SIZE {
        return Err(anyhow!(
            "Space Packet is too large: {} bytes",
            spacepacket.len()
        ));
    }

    let mut aos_tf = BytesMut::with_capacity(AOS_TF_SIZE);

    // build AOS TF PH
    aos_tf.extend_from_slice(&AOS_TF_PH_VN_SCID_VCID);
    aos_tf.extend_from_slice(&(*frame_count << 8).to_be_bytes());

    // build M_PDU header
    // first header pointer = 0
    aos_tf.extend_from_slice(&[0x00, 0x00]);

    aos_tf.extend_from_slice(&spacepacket);

    aos_tf.extend_from_slice(&IDLE_PACKET_PH_EXCEPT_LEN);
    let idle_data_len = AOS_TF_SIZE - aos_tf.len() - IDLE_PACKET_PH_LEN_SIZE - AOS_TF_CLCW.len();
    aos_tf.extend_from_slice(&((idle_data_len - 1) as u16).to_be_bytes());
    aos_tf.extend(std::iter::repeat(0u8).take(idle_data_len));

    // add CLCW
    aos_tf.extend_from_slice(&AOS_TF_CLCW);

    debug_assert_eq!(aos_tf.len(), AOS_TF_SIZE);

    *frame_count = frame_count.wrapping_add(1);

    Ok(aos_tf)
}
