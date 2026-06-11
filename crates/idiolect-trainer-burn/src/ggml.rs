//! Reader/writer for whisper.cpp GGML model files.
//!
//! This is the deployment seam of the Burn trainer: whisper.cpp cannot load
//! LoRA adapters, so a trained adapter is MERGED into the base weights here
//! and written back as an ordinary `.bin` the daemon's whisper-rs engine
//! loads unchanged. Layout (everything little-endian, no alignment padding),
//! as parsed by `whisper_model_load` in whisper.cpp:
//!
//! ```text
//! u32 magic = 0x67676d6c ("ggml")
//! 11 × i32 hparams: n_vocab, n_audio_ctx, n_audio_state, n_audio_head,
//!                   n_audio_layer, n_text_ctx, n_text_state, n_text_head,
//!                   n_text_layer, n_mels, ftype
//! i32 n_mel, i32 n_fft, then n_mel*n_fft × f32 mel filters
//! i32 stored_vocab, then per token: u32 byte_len + bytes
//! tensors until EOF: i32 n_dims, i32 name_len, i32 ttype,
//!                    n_dims × i32 dims (ne[0] fastest), name bytes, raw data
//! ```

use std::fmt::{Display, Formatter};
use std::io::{Read, Write};
use std::path::Path;

pub const GGML_MAGIC: u32 = 0x6767_6d6c;

/// ggml tensor data types whisper models actually contain.
pub const GGML_TYPE_F32: i32 = 0;
pub const GGML_TYPE_F16: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GgmlHparams {
    pub n_vocab: i32,
    pub n_audio_ctx: i32,
    pub n_audio_state: i32,
    pub n_audio_head: i32,
    pub n_audio_layer: i32,
    pub n_text_ctx: i32,
    pub n_text_state: i32,
    pub n_text_head: i32,
    pub n_text_layer: i32,
    pub n_mels: i32,
    pub ftype: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GgmlMelFilters {
    pub n_mel: i32,
    pub n_fft: i32,
    pub data: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GgmlTensor {
    pub name: String,
    /// ggml dimension order: `dims[0]` varies fastest in `data`.
    pub dims: Vec<i32>,
    pub ttype: i32,
    pub data: Vec<u8>,
}

impl GgmlTensor {
    #[must_use]
    pub fn element_count(&self) -> usize {
        self.dims.iter().map(|&d| d as usize).product()
    }

    /// The tensor's values as f32, converting from f16 when needed.
    pub fn to_f32(&self) -> Result<Vec<f32>, GgmlError> {
        match self.ttype {
            GGML_TYPE_F32 => Ok(self
                .data
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .collect()),
            GGML_TYPE_F16 => Ok(self
                .data
                .chunks_exact(2)
                .map(|b| f16_to_f32(u16::from_le_bytes([b[0], b[1]])))
                .collect()),
            other => Err(GgmlError::new(format!(
                "tensor {:?} has unsupported ggml type {other}",
                self.name
            ))),
        }
    }

    /// Overwrites the tensor's data from f32 values, encoding back to the
    /// tensor's own storage type.
    pub fn set_from_f32(&mut self, values: &[f32]) -> Result<(), GgmlError> {
        if values.len() != self.element_count() {
            return Err(GgmlError::new(format!(
                "tensor {:?} holds {} elements, got {}",
                self.name,
                self.element_count(),
                values.len()
            )));
        }
        match self.ttype {
            GGML_TYPE_F32 => {
                self.data = values.iter().flat_map(|v| v.to_le_bytes()).collect();
                Ok(())
            }
            GGML_TYPE_F16 => {
                self.data = values
                    .iter()
                    .flat_map(|v| f32_to_f16(*v).to_le_bytes())
                    .collect();
                Ok(())
            }
            other => Err(GgmlError::new(format!(
                "tensor {:?} has unsupported ggml type {other}",
                self.name
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GgmlModel {
    pub hparams: GgmlHparams,
    pub filters: GgmlMelFilters,
    pub vocab: Vec<Vec<u8>>,
    pub tensors: Vec<GgmlTensor>,
}

impl GgmlModel {
    pub fn read_file(path: &Path) -> Result<Self, GgmlError> {
        let bytes = std::fs::read(path)
            .map_err(|error| GgmlError::new(format!("reading {}: {error}", path.display())))?;
        Self::read_bytes(&bytes)
    }

    pub fn read_bytes(bytes: &[u8]) -> Result<Self, GgmlError> {
        let mut reader = Cursor { bytes, offset: 0 };
        if reader.u32()? != GGML_MAGIC {
            return Err(GgmlError::new("bad magic: not a ggml whisper model"));
        }
        let hparams = GgmlHparams {
            n_vocab: reader.i32()?,
            n_audio_ctx: reader.i32()?,
            n_audio_state: reader.i32()?,
            n_audio_head: reader.i32()?,
            n_audio_layer: reader.i32()?,
            n_text_ctx: reader.i32()?,
            n_text_state: reader.i32()?,
            n_text_head: reader.i32()?,
            n_text_layer: reader.i32()?,
            n_mels: reader.i32()?,
            ftype: reader.i32()?,
        };
        let n_mel = reader.i32()?;
        let n_fft = reader.i32()?;
        let mut filter_data = Vec::with_capacity((n_mel * n_fft) as usize);
        for _ in 0..(n_mel * n_fft) {
            filter_data.push(reader.f32()?);
        }
        let filters = GgmlMelFilters {
            n_mel,
            n_fft,
            data: filter_data,
        };
        let stored_vocab = reader.i32()?;
        let mut vocab = Vec::with_capacity(stored_vocab.max(0) as usize);
        for _ in 0..stored_vocab {
            let len = reader.u32()? as usize;
            vocab.push(reader.take(len)?.to_vec());
        }
        let mut tensors = Vec::new();
        while !reader.is_at_end() {
            let n_dims = reader.i32()?;
            let name_len = reader.i32()? as usize;
            let ttype = reader.i32()?;
            if !(1..=4).contains(&n_dims) {
                return Err(GgmlError::new(format!("implausible n_dims {n_dims}")));
            }
            let mut dims = Vec::with_capacity(n_dims as usize);
            for _ in 0..n_dims {
                dims.push(reader.i32()?);
            }
            let name = String::from_utf8(reader.take(name_len)?.to_vec())
                .map_err(|error| GgmlError::new(format!("tensor name not utf8: {error}")))?;
            let elements: usize = dims.iter().map(|&d| d as usize).product();
            let byte_len = match ttype {
                GGML_TYPE_F32 => elements * 4,
                GGML_TYPE_F16 => elements * 2,
                other => {
                    return Err(GgmlError::new(format!(
                        "tensor {name:?} has unsupported ggml type {other}"
                    )))
                }
            };
            let data = reader.take(byte_len)?.to_vec();
            tensors.push(GgmlTensor {
                name,
                dims,
                ttype,
                data,
            });
        }
        Ok(Self {
            hparams,
            filters,
            vocab,
            tensors,
        })
    }

    pub fn write_file(&self, path: &Path) -> Result<(), GgmlError> {
        let mut file = std::fs::File::create(path)
            .map_err(|error| GgmlError::new(format!("creating {}: {error}", path.display())))?;
        let bytes = self.to_bytes();
        file.write_all(&bytes)
            .map_err(|error| GgmlError::new(format!("writing {}: {error}", path.display())))?;
        Ok(())
    }

    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&GGML_MAGIC.to_le_bytes());
        for value in [
            self.hparams.n_vocab,
            self.hparams.n_audio_ctx,
            self.hparams.n_audio_state,
            self.hparams.n_audio_head,
            self.hparams.n_audio_layer,
            self.hparams.n_text_ctx,
            self.hparams.n_text_state,
            self.hparams.n_text_head,
            self.hparams.n_text_layer,
            self.hparams.n_mels,
            self.hparams.ftype,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.filters.n_mel.to_le_bytes());
        out.extend_from_slice(&self.filters.n_fft.to_le_bytes());
        for value in &self.filters.data {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&(self.vocab.len() as i32).to_le_bytes());
        for token in &self.vocab {
            out.extend_from_slice(&(token.len() as u32).to_le_bytes());
            out.extend_from_slice(token);
        }
        for tensor in &self.tensors {
            out.extend_from_slice(&(tensor.dims.len() as i32).to_le_bytes());
            out.extend_from_slice(&(tensor.name.len() as i32).to_le_bytes());
            out.extend_from_slice(&tensor.ttype.to_le_bytes());
            for dim in &tensor.dims {
                out.extend_from_slice(&dim.to_le_bytes());
            }
            out.extend_from_slice(tensor.name.as_bytes());
            out.extend_from_slice(&tensor.data);
        }
        out
    }

    pub fn tensor(&self, name: &str) -> Result<&GgmlTensor, GgmlError> {
        self.tensors
            .iter()
            .find(|tensor| tensor.name == name)
            .ok_or_else(|| GgmlError::new(format!("model has no tensor {name:?}")))
    }

    pub fn tensor_mut(&mut self, name: &str) -> Result<&mut GgmlTensor, GgmlError> {
        self.tensors
            .iter_mut()
            .find(|tensor| tensor.name == name)
            .ok_or_else(|| GgmlError::new(format!("model has no tensor {name:?}")))
    }

    /// The vocab as lossy strings (whisper stores raw bytes; some BPE pieces
    /// are not valid UTF-8 on their own).
    #[must_use]
    pub fn vocab_strings(&self) -> Vec<String> {
        self.vocab
            .iter()
            .map(|token| String::from_utf8_lossy(token).into_owned())
            .collect()
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Cursor<'_> {
    fn is_at_end(&self) -> bool {
        self.offset >= self.bytes.len()
    }

    fn take(&mut self, len: usize) -> Result<&[u8], GgmlError> {
        let end = self
            .offset
            .checked_add(len)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| GgmlError::new("model file truncated"))?;
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32, GgmlError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn i32(&mut self) -> Result<i32, GgmlError> {
        let bytes = self.take(4)?;
        Ok(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn f32(&mut self) -> Result<f32, GgmlError> {
        let bytes = self.take(4)?;
        Ok(f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

/// IEEE 754 half → single. (No `half` crate: two tiny functions are cheaper
/// than a dependency.)
#[must_use]
pub fn f16_to_f32(half: u16) -> f32 {
    let sign = u32::from(half >> 15) << 31;
    let exponent = u32::from((half >> 10) & 0x1f);
    let mantissa = u32::from(half & 0x3ff);
    let bits = match (exponent, mantissa) {
        (0, 0) => sign,
        (0, _) => {
            // Subnormal: renormalize. A half subnormal is mantissa × 2⁻²⁴; the
            // top set bit at position b makes the value 2^(b-24) × 1.xxx, i.e.
            // an f32 biased exponent of 103 + b = 113 - shift.
            let shift = mantissa.leading_zeros() - 21;
            let mantissa = (mantissa << shift) & 0x3ff;
            let exponent = 113 - shift;
            sign | (exponent << 23) | (mantissa << 13)
        }
        (0x1f, 0) => sign | 0x7f80_0000,
        (0x1f, _) => sign | 0x7f80_0000 | (mantissa << 13),
        _ => sign | ((exponent + 127 - 15) << 23) | (mantissa << 13),
    };
    f32::from_bits(bits)
}

/// IEEE 754 single → half, round-to-nearest-even.
#[must_use]
pub fn f32_to_f16(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = ((bits >> 31) as u16) << 15;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x7f_ffff;
    if exponent == 0xff {
        // Inf / NaN.
        let payload = if mantissa == 0 { 0 } else { 0x200 };
        return sign | 0x7c00 | payload;
    }
    let unbiased = exponent - 127;
    if unbiased > 15 {
        return sign | 0x7c00; // overflow → inf
    }
    if unbiased >= -14 {
        // Normal half.
        let mantissa16 = mantissa >> 13;
        let round_bit = (mantissa >> 12) & 1;
        let sticky = mantissa & 0xfff;
        let mut half = sign | (((unbiased + 15) as u16) << 10) | (mantissa16 as u16);
        if round_bit == 1 && (sticky != 0 || (mantissa16 & 1) == 1) {
            half += 1; // carries propagate correctly into the exponent
        }
        return half;
    }
    if unbiased >= -25 {
        // Subnormal half: value = full × 2^(unbiased-23), and a half subnormal
        // unit is 2⁻²⁴, so the half mantissa is full >> (-unbiased - 1) with
        // round-to-nearest-even.
        let shift = (-unbiased - 1) as u32; // 14..=24
        let full = mantissa | 0x80_0000;
        let mantissa16 = full >> shift;
        let round_bit = (full >> (shift - 1)) & 1;
        let sticky = full & ((1 << (shift - 1)) - 1);
        let mut half = sign | (mantissa16 as u16);
        if round_bit == 1 && (sticky != 0 || (mantissa16 & 1) == 1) {
            half += 1;
        }
        return half;
    }
    sign // underflow → signed zero
}

#[derive(Debug)]
pub struct GgmlError {
    message: String,
}

impl GgmlError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn from_message(message: impl Into<String>) -> Self {
        Self::new(message)
    }
}

impl Display for GgmlError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for GgmlError {}

// Silences "unused" on `Read` (kept so callers can stream if files grow).
const _: fn() = || {
    fn assert_read<T: Read>() {}
    let _ = assert_read::<std::fs::File>;
};

#[cfg(test)]
mod tests {
    use super::{f16_to_f32, f32_to_f16, GgmlModel};

    #[test]
    fn half_precision_round_trips_through_f32() {
        for half in [0u16, 0x3c00, 0xbc00, 0x7bff, 0x0001, 0x83ff, 0x7c00, 0xfc00] {
            let single = f16_to_f32(half);
            assert_eq!(
                f32_to_f16(single),
                half,
                "half bits {half:#06x} must survive the round trip (f32 = {single})"
            );
        }
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
    }

    #[test]
    fn every_f16_bit_pattern_survives_the_round_trip() {
        // The merge path rewrites f16 weight tensors; conversion must be
        // lossless for every representable half (NaN payloads excepted —
        // compare them as NaN-ness, not bits).
        for half in 0..=u16::MAX {
            let single = f16_to_f32(half);
            let exponent = (half >> 10) & 0x1f;
            let mantissa = half & 0x3ff;
            if exponent == 0x1f && mantissa != 0 {
                assert!(single.is_nan(), "half {half:#06x} should decode to NaN");
                continue;
            }
            assert_eq!(
                f32_to_f16(single),
                half,
                "half bits {half:#06x} must survive the round trip (f32 = {single})"
            );
        }
    }

    #[test]
    fn the_fixture_model_round_trips_byte_identical() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/whisper/ggml-tiny.en.bin");
        let original = std::fs::read(&path).expect("the checked-in fixture model must exist");
        let model = GgmlModel::read_bytes(&original).expect("fixture model should parse");

        assert_eq!(model.hparams.n_vocab, 51_864, "tiny.en vocab");
        assert_eq!(model.hparams.n_mels, 80);
        assert_eq!(model.filters.data.len(), (model.filters.n_mel * model.filters.n_fft) as usize);
        assert!(model.tensors.iter().any(|t| t.name == "encoder.conv1.weight"));
        assert!(model.tensors.iter().any(|t| t.name == "decoder.token_embedding.weight"));

        let rewritten = model.to_bytes();
        assert_eq!(
            rewritten, original,
            "read → write must reproduce the file byte for byte"
        );
    }
}
