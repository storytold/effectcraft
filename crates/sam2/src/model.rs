// Modified work: ported from Hugging Face `transformers` `Sam2Model` (Apache-2.0, Copyright The
// HuggingFace Inc. team) to Rust on candle. See NOTICE.
//! The single-image model: encode a frame once, then decode any number of prompts against it.

use std::path::Path;

use candle_core::{Device, Tensor};

use crate::decoder::{DecoderOutput, MaskDecoder};
use crate::neck::{ImageFeatures, VisionEncoder};
use crate::prompt::{INPUT_SIZE, Prompt, PromptEncoder};
use crate::weights::Weights;
use crate::{Error, Result};

/// A frame after the vision encoder: reusable for every prompt on that frame.
pub struct Encoded {
    /// The three neck levels, highest resolution first (`[1, 256, 256, 256]`, `128`, `64`).
    pub features: ImageFeatures,
    /// The decoder's skip features from the two highest-resolution levels.
    pub high: (Tensor, Tensor),
}

pub struct ImageModel {
    pub vision: VisionEncoder,
    pub prompt: PromptEncoder,
    pub decoder: MaskDecoder,
    no_memory: Tensor,
    image_pos: Tensor,
}

impl ImageModel {
    pub fn load(w: &Weights) -> Result<ImageModel> {
        let prompt = PromptEncoder::load(w)?;
        Ok(ImageModel {
            vision: VisionEncoder::load(w)?,
            decoder: MaskDecoder::load(w)?,
            no_memory: w.get("no_memory_embedding", &[1, 1, 256])?.reshape((1, 256, 1, 1))?,
            image_pos: prompt.image_positions()?,
            prompt,
        })
    }

    pub fn open(path: &Path) -> Result<ImageModel> {
        ImageModel::load(&Weights::open(path)?)
    }

    /// Encode a normalised `[1, 3, 1024, 1024]` frame.
    pub fn encode(&self, pixels: &Tensor) -> Result<Encoded> {
        if pixels.dims() != [1, 3, INPUT_SIZE, INPUT_SIZE] {
            return Err(Error::Input(format!("pixels must be [1, 3, {INPUT_SIZE}, {INPUT_SIZE}], got {:?}", pixels.dims())));
        }
        let features = self.vision.forward(pixels)?;
        let (l0, l1) = match (features.features.first(), features.features.get(1)) {
            (Some(a), Some(b)) => (a, b),
            _ => return Err(Error::Compute("vision encoder returned too few levels".into())),
        };
        let high = self.decoder.high_res(l0, l1)?;
        Ok(Encoded { features, high })
    }

    /// Prompt embeddings (sparse tokens, empty dense map) for a click/box prompt.
    pub fn embed_prompt(&self, p: &Prompt) -> Result<(Tensor, Tensor)> {
        Ok((self.prompt.sparse(p)?, self.prompt.dense_empty()?))
    }

    /// Decode masks for a prompt on an encoded frame.
    pub fn segment(&self, enc: &Encoded, p: &Prompt, multimask: bool) -> Result<DecoderOutput> {
        let low = enc.features.features.get(2).ok_or_else(|| Error::Compute("vision encoder returned too few levels".into()))?;
        let image = low.broadcast_add(&self.no_memory)?;
        let (sparse, dense) = self.embed_prompt(p)?;
        self.decoder.forward(&image, &self.image_pos, &sparse, &dense, &enc.high, multimask)
    }

    /// Decode against an already memory-conditioned embedding `[1, 256, g, g]` (video tracking).
    pub fn decode_embedded(&self, image: &Tensor, high: &(Tensor, Tensor), sparse: &Tensor, dense: &Tensor, multimask: bool) -> Result<DecoderOutput> {
        self.decoder.forward(image, &self.image_pos, sparse, dense, high, multimask)
    }

    /// The learned embedding added to a frame's features when there is no memory yet.
    pub fn no_memory(&self) -> &Tensor {
        &self.no_memory
    }

    pub fn device(&self) -> Device {
        Device::Cpu
    }
}
