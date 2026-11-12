// Modified work: loads the safetensors checkpoint of Hugging Face `transformers` SAM 2
// (Apache-2.0, Copyright The HuggingFace Inc. team). See NOTICE.
//! Checkpoint loading with validation: every tensor is f32 and finite, every lookup checks its
//! shape, so a truncated, mislabelled or poisoned file is an error, not a crash or garbage.

use std::collections::HashMap;
use std::path::Path;

use candle_core::{DType, Device, Tensor};

use crate::{Error, Result};

/// Named tensors of one checkpoint.
pub struct Weights {
    map: HashMap<String, Tensor>,
}

impl Weights {
    /// Read a `.safetensors` file; rejects non-finite values and unsupported dtypes.
    pub fn open(path: &Path) -> Result<Weights> {
        let map = candle_core::safetensors::load(path, &Device::Cpu).map_err(|e| Error::Weights(format!("{}: {e}", path.display())))?;
        Weights::from_map(map)
    }

    /// Read a checkpoint held in memory (the installed file's bytes, already verified by the caller).
    pub fn from_bytes(bytes: &[u8]) -> Result<Weights> {
        let map = candle_core::safetensors::load_buffer(bytes, &Device::Cpu).map_err(|e| Error::Weights(e.to_string()))?;
        Weights::from_map(map)
    }

    /// Validate an already-loaded set of tensors.
    pub fn from_map(map: HashMap<String, Tensor>) -> Result<Weights> {
        let mut out = HashMap::with_capacity(map.len());
        for (name, t) in map {
            let t = match t.dtype() {
                DType::F32 => t,
                DType::F16 | DType::BF16 | DType::F64 => t.to_dtype(DType::F32)?,
                other => return Err(Error::Weights(format!("`{name}` has unsupported dtype {other:?}"))),
            };
            let flat = t.flatten_all()?.to_vec1::<f32>()?;
            if flat.iter().any(|v| !v.is_finite()) {
                return Err(Error::Weights(format!("`{name}` contains NaN or infinity")));
            }
            out.insert(name, t);
        }
        Ok(Weights { map: out })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.map.contains_key(name)
    }

    /// The shape of a tensor, if the checkpoint has it.
    pub fn shape(&self, name: &str) -> Option<Vec<usize>> {
        self.map.get(name).map(|t| t.dims().to_vec())
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The tensor `name`, which must have exactly `shape`.
    pub fn get(&self, name: &str, shape: &[usize]) -> Result<Tensor> {
        let t = self.map.get(name).ok_or_else(|| Error::Weights(format!("missing tensor `{name}`")))?;
        if t.dims() != shape {
            return Err(Error::Weights(format!("`{name}` has shape {:?}, expected {shape:?}", t.dims())));
        }
        Ok(t.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(name: &str, t: Tensor) -> HashMap<String, Tensor> {
        HashMap::from([(name.to_string(), t)])
    }

    #[test]
    fn rejects_missing_wrong_shape_and_non_finite() {
        let ok = Tensor::zeros((2, 3), DType::F32, &Device::Cpu).unwrap();
        let w = Weights::from_map(one("a", ok)).unwrap();
        assert!(w.get("a", &[2, 3]).is_ok());
        assert!(matches!(w.get("a", &[3, 2]), Err(Error::Weights(_))));
        assert!(matches!(w.get("b", &[1]), Err(Error::Weights(_))));
        let bad = Tensor::new(&[1.0f32, f32::NAN], &Device::Cpu).unwrap();
        assert!(matches!(Weights::from_map(one("n", bad)), Err(Error::Weights(_))));
        let inf = Tensor::new(&[f32::INFINITY], &Device::Cpu).unwrap();
        assert!(Weights::from_map(one("i", inf)).is_err());
        let ints = Tensor::new(&[1u32], &Device::Cpu).unwrap();
        assert!(Weights::from_map(one("u", ints)).is_err());
    }

    #[test]
    fn a_truncated_file_is_an_error() {
        let dir = std::env::temp_dir().join(format!("sam2-weights-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("w.safetensors");
        let t = Tensor::zeros((4, 4), DType::F32, &Device::Cpu).unwrap();
        candle_core::safetensors::save(&one("x", t), &p).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        std::fs::write(&p, &bytes[..bytes.len() - 8]).unwrap();
        assert!(matches!(Weights::open(&p), Err(Error::Weights(_))));
        assert!(matches!(Weights::open(&dir.join("absent")), Err(Error::Weights(_))));
        std::fs::remove_dir_all(&dir).ok();
    }
}
