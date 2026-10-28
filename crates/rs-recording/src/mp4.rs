//! A small, dependency-free MP4 writer for recorded Annex-B video.

use std::io::{Seek, SeekFrom, Write};

use rsp_core::nal::{hevc_nal_type, hevc_type, split_annexb};

/// Video codecs understood by the MP4 writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mp4Codec {
    Hevc,
    H264,
}

impl Mp4Codec {
    /// Converts the value used by the wire `Codec` enum.
    pub fn from_proto(codec: u8) -> Option<Self> {
        match codec {
            1 => Some(Self::Hevc),
            2 => Some(Self::H264),
            _ => None,
        }
    }
}

/// Errors produced while writing an MP4.
#[derive(Debug, thiserror::Error)]
pub enum Mp4Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("first sample must be a keyframe")]
    NeedKeyframe,
    #[error("keyframe without parameter sets")]
    NeedParameterSets,
    #[error("bad parameter set: {0}")]
    BadParameterSet(&'static str),
    #[error("no samples")]
    NoSamples,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParameterSets {
    vps: Option<Vec<u8>>,
    sps: Vec<u8>,
    pps: Vec<u8>,
}

#[derive(Debug, Default)]
struct FoundParameterSets {
    vps: Option<Vec<u8>>,
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl FoundParameterSets {
    fn any(&self) -> bool {
        self.vps.is_some() || self.sps.is_some() || self.pps.is_some()
    }

    fn complete(&self, codec: Mp4Codec) -> bool {
        match codec {
            Mp4Codec::Hevc => self.vps.is_some() && self.sps.is_some() && self.pps.is_some(),
            Mp4Codec::H264 => self.sps.is_some() && self.pps.is_some(),
        }
    }

    fn merge_into(&self, current: &ParameterSets, codec: Mp4Codec) -> ParameterSets {
        ParameterSets {
            vps: match codec {
                Mp4Codec::Hevc => self.vps.clone().or_else(|| current.vps.clone()),
                Mp4Codec::H264 => None,
            },
            sps: self.sps.clone().unwrap_or_else(|| current.sps.clone()),
            pps: self.pps.clone().unwrap_or_else(|| current.pps.clone()),
        }
    }

    fn into_parameter_sets(self, codec: Mp4Codec) -> Option<ParameterSets> {
        if !self.complete(codec) {
            return None;
        }
        Some(ParameterSets {
            vps: match codec {
                Mp4Codec::Hevc => self.vps,
                Mp4Codec::H264 => None,
            },
            sps: self.sps?,
            pps: self.pps?,
        })
    }
}

#[derive(Debug)]
struct SampleEntry {
    params: ParameterSets,
    width: u32,
    height: u32,
}

#[derive(Debug)]
struct Sample {
    pts_us: u64,
    offset: u64,
    size: u32,
    keyframe: bool,
    description_index: u32,
}

/// Writes a video-only ISO BMFF file.
pub struct Mp4Writer<W: Write + Seek> {
    out: W,
    codec: Mp4Codec,
    width: u32,
    height: u32,
    mdat_start: u64,
    samples: Vec<Sample>,
    entries: Vec<SampleEntry>,
    current_entry: Option<usize>,
}

impl<W: Write + Seek> Mp4Writer<W> {
    /// Writes the file type and an mdat box with a 64-bit size placeholder.
    pub fn new(mut out: W, codec: Mp4Codec, width: u32, height: u32) -> Result<Self, Mp4Error> {
        let ftyp = make_ftyp(codec);
        out.write_all(&ftyp)?;
        let mdat_start = out.stream_position()?;
        out.write_all(&1_u32.to_be_bytes())?;
        out.write_all(b"mdat")?;
        out.write_all(&0_u64.to_be_bytes())?;
        Ok(Self {
            out,
            codec,
            width,
            height,
            mdat_start,
            samples: Vec::new(),
            entries: Vec::new(),
            current_entry: None,
        })
    }

    /// Sets the dimensions to use when a subsequent sample entry is created.
    pub fn set_dimensions(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    /// Appends one Annex-B access unit. Parameter sets and AUDs are kept out of
    /// the sample and are written in the sample description instead.
    pub fn push_sample(
        &mut self,
        pts_us: u64,
        keyframe: bool,
        annexb: &[u8],
    ) -> Result<(), Mp4Error> {
        let (payload, found) = self.convert_sample(annexb)?;
        if payload.is_empty() {
            return Ok(());
        }

        let description_index = if self.samples.is_empty() {
            if !keyframe {
                return Err(Mp4Error::NeedKeyframe);
            }
            let params = found
                .into_parameter_sets(self.codec)
                .ok_or(Mp4Error::NeedParameterSets)?;
            validate_parameter_sets(self.codec, &params)?;
            self.entries.push(SampleEntry {
                params,
                width: self.width,
                height: self.height,
            });
            self.current_entry = Some(0);
            1
        } else {
            let current_index = self.current_entry.ok_or(Mp4Error::NeedParameterSets)?;
            if keyframe && found.any() {
                let current = &self.entries[current_index].params;
                let candidate = found.merge_into(current, self.codec);
                if candidate != *current {
                    validate_parameter_sets(self.codec, &candidate)?;
                    self.entries.push(SampleEntry {
                        params: candidate,
                        width: self.width,
                        height: self.height,
                    });
                    self.current_entry = Some(self.entries.len() - 1);
                }
            }
            let index = self.current_entry.ok_or(Mp4Error::NeedParameterSets)?;
            u32::try_from(index + 1).map_err(|_| Mp4Error::BadParameterSet("entry"))?
        };

        let size = u32::try_from(payload.len()).map_err(|_| Mp4Error::BadParameterSet("nal"))?;
        let offset = self.out.stream_position()?;
        self.out.write_all(&payload)?;
        self.samples.push(Sample {
            pts_us,
            offset,
            size,
            keyframe,
            description_index,
        });
        Ok(())
    }

    /// Completes the mdat box, appends moov, and returns the underlying writer.
    pub fn finish(mut self) -> Result<W, Mp4Error> {
        if self.samples.is_empty() {
            return Err(Mp4Error::NoSamples);
        }
        let mdat_end = self.out.stream_position()?;
        let mdat_size = mdat_end.saturating_sub(self.mdat_start);
        self.out.seek(SeekFrom::Start(self.mdat_start + 8))?;
        self.out.write_all(&mdat_size.to_be_bytes())?;
        self.out.seek(SeekFrom::Start(mdat_end))?;
        let moov = make_moov(self.codec, &self.entries, &self.samples);
        self.out.write_all(&moov)?;
        Ok(self.out)
    }

    /// Number of samples written so far. Access units without a slice are not
    /// counted.
    pub fn sample_count(&self) -> u32 {
        self.samples.len().try_into().unwrap_or(u32::MAX)
    }

    fn convert_sample(&self, annexb: &[u8]) -> Result<(Vec<u8>, FoundParameterSets), Mp4Error> {
        let mut payload = Vec::new();
        let mut found = FoundParameterSets::default();
        for nal in split_annexb(annexb) {
            if nal.is_empty() {
                continue;
            }
            let kind = match self.codec {
                Mp4Codec::Hevc => hevc_nal_type(nal),
                Mp4Codec::H264 => Some(nal[0] & 0x1f),
            };
            let is_parameter = match self.codec {
                Mp4Codec::Hevc => {
                    matches!(kind, Some(hevc_type::VPS | hevc_type::SPS | hevc_type::PPS))
                }
                Mp4Codec::H264 => matches!(kind, Some(7 | 8)),
            };
            let is_aud = match self.codec {
                Mp4Codec::Hevc => kind == Some(hevc_type::AUD),
                Mp4Codec::H264 => kind == Some(9),
            };
            if is_parameter {
                match (self.codec, kind) {
                    (Mp4Codec::Hevc, Some(hevc_type::VPS)) => found.vps = Some(nal.to_vec()),
                    (Mp4Codec::Hevc, Some(hevc_type::SPS)) | (Mp4Codec::H264, Some(7)) => {
                        found.sps = Some(nal.to_vec())
                    }
                    (Mp4Codec::Hevc, Some(hevc_type::PPS)) | (Mp4Codec::H264, Some(8)) => {
                        found.pps = Some(nal.to_vec())
                    }
                    _ => {}
                }
            } else if !is_aud {
                let len = u32::try_from(nal.len()).map_err(|_| Mp4Error::BadParameterSet("nal"))?;
                payload.extend_from_slice(&len.to_be_bytes());
                payload.extend_from_slice(nal);
            }
        }
        Ok((payload, found))
    }
}

fn us_delta_to_ticks(delta_us: u64) -> u32 {
    let ticks = (u128::from(delta_us) * 9 + 50) / 100;
    u32::try_from(ticks.max(1).min(u128::from(u32::MAX))).unwrap_or(u32::MAX)
}

fn validate_parameter_sets(codec: Mp4Codec, params: &ParameterSets) -> Result<(), Mp4Error> {
    match codec {
        Mp4Codec::Hevc => {
            if params.vps.as_deref().is_none_or(|v| v.is_empty()) {
                return Err(Mp4Error::BadParameterSet("vps"));
            }
            if params.pps.is_empty() {
                return Err(Mp4Error::BadParameterSet("pps"));
            }
            let rbsp = remove_emulation_prevention(&params.sps);
            if rbsp.len() < 15 {
                return Err(Mp4Error::BadParameterSet("sps"));
            }
            if params.sps.len() < 2 {
                return Err(Mp4Error::BadParameterSet("sps"));
            }
        }
        Mp4Codec::H264 => {
            if params.sps.len() < 4 {
                return Err(Mp4Error::BadParameterSet("sps"));
            }
            if params.pps.is_empty() {
                return Err(Mp4Error::BadParameterSet("pps"));
            }
        }
    }
    Ok(())
}

fn remove_emulation_prevention(nal: &[u8]) -> Vec<u8> {
    let mut rbsp = Vec::with_capacity(nal.len());
    let mut zeroes = 0_u8;
    for &byte in nal {
        if zeroes >= 2 && byte == 3 {
            zeroes = 0;
            continue;
        }
        rbsp.push(byte);
        if byte == 0 {
            zeroes = zeroes.saturating_add(1);
        } else {
            zeroes = 0;
        }
    }
    rbsp
}

fn make_ftyp(codec: Mp4Codec) -> Vec<u8> {
    let mut payload = Vec::with_capacity(24);
    payload.extend_from_slice(b"isom");
    payload.extend_from_slice(&512_u32.to_be_bytes());
    for brand in [b"isom".as_slice(), b"iso2", b"mp41"] {
        payload.extend_from_slice(brand);
    }
    if codec == Mp4Codec::H264 {
        payload.extend_from_slice(b"avc1");
    }
    make_box(*b"ftyp", payload)
}

fn make_moov(codec: Mp4Codec, entries: &[SampleEntry], samples: &[Sample]) -> Vec<u8> {
    let durations = sample_durations(samples);
    let duration_ticks = durations
        .iter()
        .map(|&(_, delta)| u64::from(delta))
        .sum::<u64>();
    let duration_ms =
        ((u128::from(duration_ticks) * 1000) / 90_000).min(u128::from(u64::MAX)) as u64;

    let mvhd = make_mvhd(duration_ms);
    let tkhd = make_tkhd(
        duration_ms,
        entries.first().map_or(0, |entry| entry.width),
        entries.first().map_or(0, |entry| entry.height),
    );
    let mdhd = make_mdhd(duration_ticks);
    let hdlr = make_hdlr();
    let stbl = make_stbl(codec, entries, samples, &durations);
    let minf = make_box(
        *b"minf",
        [make_vmhd(), make_dinf(), stbl]
            .into_iter()
            .flatten()
            .collect(),
    );
    let mdia = make_box(*b"mdia", [mdhd, hdlr, minf].into_iter().flatten().collect());
    let trak = make_box(*b"trak", [tkhd, mdia].into_iter().flatten().collect());
    make_box(*b"moov", [mvhd, trak].into_iter().flatten().collect())
}

fn make_mvhd(duration_ms: u64) -> Vec<u8> {
    let mut payload = Vec::new();
    if duration_ms <= u64::from(u32::MAX) {
        payload.extend_from_slice(&[0, 0, 0, 0]);
        payload.extend_from_slice(&0_u32.to_be_bytes());
        payload.extend_from_slice(&0_u32.to_be_bytes());
        payload.extend_from_slice(&1000_u32.to_be_bytes());
        payload.extend_from_slice(&(duration_ms as u32).to_be_bytes());
    } else {
        payload.extend_from_slice(&[1, 0, 0, 0]);
        payload.extend_from_slice(&0_u64.to_be_bytes());
        payload.extend_from_slice(&0_u64.to_be_bytes());
        payload.extend_from_slice(&1000_u32.to_be_bytes());
        payload.extend_from_slice(&duration_ms.to_be_bytes());
    }
    payload.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    payload.extend_from_slice(&0x0100_u16.to_be_bytes());
    payload.extend_from_slice(&0_u16.to_be_bytes());
    payload.extend_from_slice(&[0; 8]);
    payload.extend_from_slice(&matrix());
    payload.extend_from_slice(&[0; 24]);
    payload.extend_from_slice(&2_u32.to_be_bytes());
    make_box(*b"mvhd", payload)
}

fn make_tkhd(duration_ms: u64, width: u32, height: u32) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&[0, 0, 0, 3]);
    payload.extend_from_slice(&0_u32.to_be_bytes());
    payload.extend_from_slice(&0_u32.to_be_bytes());
    payload.extend_from_slice(&1_u32.to_be_bytes());
    payload.extend_from_slice(&0_u32.to_be_bytes());
    payload.extend_from_slice(&(duration_ms.min(u64::from(u32::MAX)) as u32).to_be_bytes());
    payload.extend_from_slice(&[0; 8]);
    payload.extend_from_slice(&0_u16.to_be_bytes());
    payload.extend_from_slice(&0_u16.to_be_bytes());
    payload.extend_from_slice(&0x0100_u16.to_be_bytes());
    payload.extend_from_slice(&0_u16.to_be_bytes());
    payload.extend_from_slice(&matrix());
    payload.extend_from_slice(&(width.min(u32::from(u16::MAX)) << 16).to_be_bytes());
    payload.extend_from_slice(&(height.min(u32::from(u16::MAX)) << 16).to_be_bytes());
    make_box(*b"tkhd", payload)
}

fn make_mdhd(duration_ticks: u64) -> Vec<u8> {
    let mut payload = Vec::new();
    if duration_ticks <= u64::from(u32::MAX) {
        payload.extend_from_slice(&[0, 0, 0, 0]);
        payload.extend_from_slice(&0_u32.to_be_bytes());
        payload.extend_from_slice(&0_u32.to_be_bytes());
        payload.extend_from_slice(&90_000_u32.to_be_bytes());
        payload.extend_from_slice(&(duration_ticks as u32).to_be_bytes());
    } else {
        payload.extend_from_slice(&[1, 0, 0, 0]);
        payload.extend_from_slice(&0_u64.to_be_bytes());
        payload.extend_from_slice(&0_u64.to_be_bytes());
        payload.extend_from_slice(&90_000_u32.to_be_bytes());
        payload.extend_from_slice(&duration_ticks.to_be_bytes());
    }
    payload.extend_from_slice(&0x55c4_u16.to_be_bytes());
    payload.extend_from_slice(&0_u16.to_be_bytes());
    make_box(*b"mdhd", payload)
}

fn make_hdlr() -> Vec<u8> {
    let mut payload = Vec::new();
    // version/flags, pre_defined
    payload.extend_from_slice(&[0; 8]);
    payload.extend_from_slice(b"vide");
    payload.extend_from_slice(&[0; 12]);
    payload.extend_from_slice(b"VideoHandler");
    payload.push(0);
    make_box(*b"hdlr", payload)
}

fn make_vmhd() -> Vec<u8> {
    let mut payload = vec![0, 0, 0, 1];
    payload.extend_from_slice(&0_u16.to_be_bytes());
    payload.extend_from_slice(&[0; 6]);
    make_box(*b"vmhd", payload)
}

fn make_dinf() -> Vec<u8> {
    let url = make_box(*b"url ", vec![0, 0, 0, 1]);
    let mut dref_payload = vec![0; 4];
    dref_payload.extend_from_slice(&1_u32.to_be_bytes());
    dref_payload.extend_from_slice(&url);
    let dref = make_box(*b"dref", dref_payload);
    make_box(*b"dinf", dref)
}

fn make_stbl(
    codec: Mp4Codec,
    entries: &[SampleEntry],
    samples: &[Sample],
    durations: &[(u32, u32)],
) -> Vec<u8> {
    let stsd = make_stsd(codec, entries);
    let mut stts_payload = vec![0; 4];
    stts_payload.extend_from_slice(
        &u32::try_from(durations.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    for &(count, delta) in durations {
        stts_payload.extend_from_slice(&count.to_be_bytes());
        stts_payload.extend_from_slice(&delta.to_be_bytes());
    }
    let stts = make_box(*b"stts", stts_payload);

    let keyframes: Vec<u32> = samples
        .iter()
        .enumerate()
        .filter_map(|(index, sample)| {
            sample
                .keyframe
                .then_some(u32::try_from(index + 1).unwrap_or(u32::MAX))
        })
        .collect();
    let mut stss_payload = vec![0; 4];
    stss_payload.extend_from_slice(
        &u32::try_from(keyframes.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    for number in keyframes {
        stss_payload.extend_from_slice(&number.to_be_bytes());
    }
    let stss = make_box(*b"stss", stss_payload);

    let mut stsc_entries = Vec::new();
    for (index, sample) in samples.iter().enumerate() {
        if index == 0 || sample.description_index != samples[index - 1].description_index {
            stsc_entries.push((
                u32::try_from(index + 1).unwrap_or(u32::MAX),
                sample.description_index,
            ));
        }
    }
    let mut stsc_payload = vec![0; 4];
    stsc_payload.extend_from_slice(
        &u32::try_from(stsc_entries.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    for (first_chunk, description_index) in stsc_entries {
        stsc_payload.extend_from_slice(&first_chunk.to_be_bytes());
        stsc_payload.extend_from_slice(&1_u32.to_be_bytes());
        stsc_payload.extend_from_slice(&description_index.to_be_bytes());
    }
    let stsc = make_box(*b"stsc", stsc_payload);

    let mut stsz_payload = vec![0; 4];
    // sample_size 0: every sample has its own entry.
    stsz_payload.extend_from_slice(&0_u32.to_be_bytes());
    stsz_payload.extend_from_slice(
        &u32::try_from(samples.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    for sample in samples {
        stsz_payload.extend_from_slice(&sample.size.to_be_bytes());
    }
    let stsz = make_box(*b"stsz", stsz_payload);

    let mut co64_payload = vec![0; 4];
    co64_payload.extend_from_slice(
        &u32::try_from(samples.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    for sample in samples {
        co64_payload.extend_from_slice(&sample.offset.to_be_bytes());
    }
    let co64 = make_box(*b"co64", co64_payload);

    make_box(
        *b"stbl",
        [stsd, stts, stss, stsc, stsz, co64]
            .into_iter()
            .flatten()
            .collect(),
    )
}

fn make_stsd(codec: Mp4Codec, entries: &[SampleEntry]) -> Vec<u8> {
    let mut payload = vec![0; 4];
    payload.extend_from_slice(
        &u32::try_from(entries.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
    for entry in entries {
        let mut sample_entry = Vec::new();
        sample_entry.extend_from_slice(&[0; 6]);
        sample_entry.extend_from_slice(&1_u16.to_be_bytes());
        sample_entry.extend_from_slice(&[0; 16]);
        sample_entry.extend_from_slice(
            &u16::try_from(entry.width.min(u32::from(u16::MAX)))
                .unwrap_or(u16::MAX)
                .to_be_bytes(),
        );
        sample_entry.extend_from_slice(
            &u16::try_from(entry.height.min(u32::from(u16::MAX)))
                .unwrap_or(u16::MAX)
                .to_be_bytes(),
        );
        sample_entry.extend_from_slice(&0x0048_0000_u32.to_be_bytes());
        sample_entry.extend_from_slice(&0x0048_0000_u32.to_be_bytes());
        sample_entry.extend_from_slice(&0_u32.to_be_bytes());
        sample_entry.extend_from_slice(&1_u16.to_be_bytes());
        sample_entry.extend_from_slice(&[0; 32]);
        sample_entry.extend_from_slice(&0x0018_u16.to_be_bytes());
        sample_entry.extend_from_slice(&0xffff_u16.to_be_bytes());
        let child = match codec {
            Mp4Codec::Hevc => make_hvcc(&entry.params),
            Mp4Codec::H264 => make_avcc(&entry.params),
        };
        sample_entry.extend_from_slice(&child);
        payload.extend_from_slice(&make_box(
            if codec == Mp4Codec::Hevc {
                *b"hvc1"
            } else {
                *b"avc1"
            },
            sample_entry,
        ));
    }
    make_box(*b"stsd", payload)
}

fn make_hvcc(params: &ParameterSets) -> Vec<u8> {
    let sps = params.sps.as_slice();
    let rbsp = remove_emulation_prevention(sps);
    let sub_layers = (rbsp[2] >> 1) & 0x07;
    let temporal_nesting = rbsp[2] & 1;
    let ptl = &rbsp[3..15];
    let profile_idc = ptl[0] & 0x1f;
    let mut payload = Vec::with_capacity(64);
    payload.push(1);
    payload.push(ptl[0]);
    payload.extend_from_slice(&ptl[1..5]);
    payload.extend_from_slice(&ptl[5..11]);
    payload.push(ptl[11]);
    payload.extend_from_slice(&0xf000_u16.to_be_bytes());
    payload.push(0xfc);
    payload.push(0xfd);
    let depth = if profile_idc == 2 { 10 } else { 8 };
    payload.push(0xf8 | (depth - 8));
    payload.push(0xf8 | (depth - 8));
    payload.extend_from_slice(&0_u16.to_be_bytes());
    payload.push((sub_layers + 1) << 3 | temporal_nesting << 2 | 3);
    payload.push(3);
    for (kind, nal) in [
        (hevc_type::VPS, params.vps.as_deref().unwrap_or_default()),
        (hevc_type::SPS, sps),
        (hevc_type::PPS, params.pps.as_slice()),
    ] {
        payload.push(0x80 | kind);
        payload.extend_from_slice(&1_u16.to_be_bytes());
        payload.extend_from_slice(&u16::try_from(nal.len()).unwrap_or(u16::MAX).to_be_bytes());
        payload.extend_from_slice(nal);
    }
    make_box(*b"hvcC", payload)
}

fn make_avcc(params: &ParameterSets) -> Vec<u8> {
    let mut payload = Vec::with_capacity(16 + params.sps.len() + params.pps.len());
    payload.push(1);
    payload.push(params.sps.get(1).copied().unwrap_or(0));
    payload.push(params.sps.get(2).copied().unwrap_or(0));
    payload.push(params.sps.get(3).copied().unwrap_or(0));
    payload.push(0xff);
    payload.push(0xe1);
    payload.extend_from_slice(
        &u16::try_from(params.sps.len())
            .unwrap_or(u16::MAX)
            .to_be_bytes(),
    );
    payload.extend_from_slice(&params.sps);
    payload.push(1);
    payload.extend_from_slice(
        &u16::try_from(params.pps.len())
            .unwrap_or(u16::MAX)
            .to_be_bytes(),
    );
    payload.extend_from_slice(&params.pps);
    make_box(*b"avcC", payload)
}

fn sample_durations(samples: &[Sample]) -> Vec<(u32, u32)> {
    if samples.is_empty() {
        return Vec::new();
    }
    let mut deltas = Vec::with_capacity(samples.len());
    if samples.len() == 1 {
        deltas.push(3000_u32);
    } else {
        for pair in samples.windows(2) {
            let delta_us = pair[1].pts_us.saturating_sub(pair[0].pts_us);
            deltas.push(us_delta_to_ticks(delta_us));
        }
        let last = *deltas.last().unwrap_or(&3000);
        deltas.push(last);
    }
    let mut runs = Vec::new();
    let mut current = deltas[0];
    let mut count = 0_u32;
    for delta in deltas {
        if delta == current && count < u32::MAX {
            count += 1;
        } else {
            runs.push((count, current));
            current = delta;
            count = 1;
        }
    }
    runs.push((count, current));
    runs
}

fn matrix() -> [u8; 36] {
    let mut matrix = [0_u8; 36];
    matrix[0..4].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
    matrix[16..20].copy_from_slice(&0x0001_0000_u32.to_be_bytes());
    matrix[32..36].copy_from_slice(&0x4000_0000_u32.to_be_bytes());
    matrix
}

fn make_box(kind: [u8; 4], payload: Vec<u8>) -> Vec<u8> {
    let size = 8_u64.saturating_add(payload.len() as u64);
    let mut result = Vec::with_capacity((size.min(usize::MAX as u64)) as usize);
    if size <= u64::from(u32::MAX) {
        result.extend_from_slice(&(size as u32).to_be_bytes());
        result.extend_from_slice(&kind);
    } else {
        result.extend_from_slice(&1_u32.to_be_bytes());
        result.extend_from_slice(&kind);
        result.extend_from_slice(&size.to_be_bytes());
    }
    result.extend_from_slice(&payload);
    result
}
