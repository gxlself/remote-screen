//! HEVC NAL unit helpers.
//!
//! On the wire a frame is an Annex-B access unit (start-code delimited), with
//! VPS/SPS/PPS prepended on keyframes, so a dump of the stream plays in ffmpeg.
//! Apple decoders want the length-prefixed ("AVCC"/hvcC) layout plus the
//! parameter sets separately; [`hevc_to_avcc`] does that conversion.

/// HEVC NAL unit types used here (ITU-T H.265 table 7-1).
pub mod hevc_type {
    pub const VPS: u8 = 32;
    pub const SPS: u8 = 33;
    pub const PPS: u8 = 34;
    pub const AUD: u8 = 35;
}

/// Splits an Annex-B byte stream into NAL units (start codes and trailing
/// zero bytes removed). Accepts 3- and 4-byte start codes.
pub fn split_annexb(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut nals = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut end = if k + 1 < starts.len() {
            starts[k + 1] - 3
        } else {
            data.len()
        };
        while end > s && data[end - 1] == 0 {
            end -= 1;
        }
        if end > s {
            nals.push(&data[s..end]);
        }
    }
    nals
}

/// `nal_unit_type` from the two-byte HEVC NAL header.
pub fn hevc_nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| (b >> 1) & 0x3f)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HevcAccessUnit {
    pub vps: Option<Vec<u8>>,
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
    /// Remaining NAL units, each prefixed with a 4-byte big-endian length.
    pub avcc: Vec<u8>,
    /// Number of NAL units in `avcc`.
    pub nal_count: usize,
}

impl HevcAccessUnit {
    pub fn has_parameter_sets(&self) -> bool {
        self.vps.is_some() && self.sps.is_some() && self.pps.is_some()
    }
}

/// Converts an Annex-B access unit to length-prefixed form, pulling out
/// parameter sets and dropping access unit delimiters.
pub fn hevc_to_avcc(data: &[u8]) -> HevcAccessUnit {
    let mut au = HevcAccessUnit::default();
    for nal in split_annexb(data) {
        match hevc_nal_type(nal) {
            Some(hevc_type::VPS) => au.vps = Some(nal.to_vec()),
            Some(hevc_type::SPS) => au.sps = Some(nal.to_vec()),
            Some(hevc_type::PPS) => au.pps = Some(nal.to_vec()),
            Some(hevc_type::AUD) => {}
            _ => {
                au.avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                au.avcc.extend_from_slice(nal);
                au.nal_count += 1;
            }
        }
    }
    au
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nal(kind: u8, body: &[u8]) -> Vec<u8> {
        let mut v = vec![kind << 1, 0x01];
        v.extend_from_slice(body);
        v
    }

    #[test]
    fn splits_three_and_four_byte_start_codes() {
        let mut s = vec![0, 0, 0, 1];
        s.extend(nal(32, &[1, 2]));
        s.extend([0, 0, 1]);
        s.extend(nal(1, &[3]));
        s.extend([0, 0, 0, 1]);
        s.extend(nal(19, &[4, 5, 6]));
        let parts = split_annexb(&s);
        assert_eq!(
            parts,
            vec![
                &nal(32, &[1, 2])[..],
                &nal(1, &[3])[..],
                &nal(19, &[4, 5, 6])[..]
            ]
        );
    }

    #[test]
    fn ignores_garbage_before_first_start_code_and_empty_nals() {
        let mut s = vec![9, 9, 0, 0, 1, 0, 0, 1];
        s.extend(nal(1, &[7]));
        assert_eq!(split_annexb(&s), vec![&nal(1, &[7])[..]]);
        assert!(split_annexb(&[1, 2, 3]).is_empty());
    }

    #[test]
    fn keyframe_converts_to_avcc_with_parameter_sets() {
        let (vps, sps, pps) = (nal(32, &[0xA]), nal(33, &[0xB, 0xB]), nal(34, &[0xC]));
        let idr = nal(19, &[1, 2, 3, 4]);
        let mut s = Vec::new();
        for n in [&nal(35, &[0x50]), &vps, &sps, &pps, &idr] {
            s.extend([0, 0, 0, 1]);
            s.extend(n.iter());
        }
        let au = hevc_to_avcc(&s);
        assert!(au.has_parameter_sets());
        assert_eq!(au.vps.as_deref(), Some(&vps[..]));
        assert_eq!(au.sps.as_deref(), Some(&sps[..]));
        assert_eq!(au.pps.as_deref(), Some(&pps[..]));
        assert_eq!(au.nal_count, 1);
        let mut expected = (idr.len() as u32).to_be_bytes().to_vec();
        expected.extend(&idr);
        assert_eq!(au.avcc, expected);
    }

    #[test]
    fn delta_frame_has_no_parameter_sets() {
        let mut s = vec![0, 0, 0, 1];
        s.extend(nal(1, &[9; 10]));
        s.extend([0, 0, 0, 1]);
        s.extend(nal(1, &[8; 3]));
        let au = hevc_to_avcc(&s);
        assert!(!au.has_parameter_sets());
        assert_eq!(au.nal_count, 2);
        assert_eq!(au.avcc.len(), 4 + 12 + 4 + 5);
    }

    #[test]
    fn nal_type_reads_header() {
        assert_eq!(hevc_nal_type(&[0x40, 0x01]), Some(32));
        assert_eq!(hevc_nal_type(&[0x26, 0x01]), Some(19));
        assert_eq!(hevc_nal_type(&[]), None);
    }
}
