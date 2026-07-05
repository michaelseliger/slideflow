//! The real embedder: `intfloat/multilingual-e5-small` run on CPU via candle.
//!
//! Gated behind the `embeddings` cargo feature so the default engine build pulls
//! no ML dependencies. Loads the model from a directory (the desktop host is
//! responsible for downloading + verifying those files — **core never touches the
//! network**). E5 requires task prefixes and uses masked mean pooling followed by
//! L2 normalization, exactly matching the `sentence-transformers` reference.

use std::path::Path;

use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

use super::Embedder;
use crate::embed::store::l2_normalize;
use crate::error::{Error, Result};

/// Stable model identifier persisted with every vector (embeddings `model_id`).
pub const MODEL_ID: &str = "intfloat/multilingual-e5-small";
/// E5 truncates inputs at 512 tokens.
const MAX_TOKENS: usize = 512;

/// The three files [`E5Embedder::load`] expects inside its model directory.
pub const MODEL_FILES: [&str; 3] = ["model.safetensors", "tokenizer.json", "config.json"];

/// CPU-resident E5 embedder.
pub struct E5Embedder {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
    dims: usize,
}

impl E5Embedder {
    /// Load the model, tokenizer and config from `dir` (`model.safetensors`,
    /// `tokenizer.json`, `config.json`). Loading mmaps ~450 MB of weights, so call
    /// this off the UI thread.
    pub fn load(dir: &Path) -> Result<Self> {
        let config_str = std::fs::read_to_string(dir.join("config.json"))
            .map_err(|e| Error::Embedding(format!("reading config.json: {e}")))?;
        let config: Config = serde_json::from_str(&config_str)
            .map_err(|e| Error::Embedding(format!("parsing config.json: {e}")))?;
        let dims = config.hidden_size;

        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| Error::Embedding(format!("loading tokenizer: {e}")))?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            ..Default::default()
        }));
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_TOKENS,
                ..Default::default()
            }))
            .map_err(|e| Error::Embedding(format!("configuring truncation: {e}")))?;

        let device = Device::Cpu;
        let weights = dir.join("model.safetensors");
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights], DTYPE, &device)
                .map_err(|e| Error::Embedding(format!("mmap safetensors: {e}")))?
        };
        let model = BertModel::load(vb, &config)
            .map_err(|e| Error::Embedding(format!("building model: {e}")))?;

        Ok(E5Embedder { model, tokenizer, device, dims })
    }

    /// Tokenize + run the model over a batch, returning masked-mean-pooled,
    /// L2-normalized vectors (one per input string).
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let encodings = self
            .tokenizer
            .encode_batch(texts.to_vec(), true)
            .map_err(|e| Error::Embedding(format!("tokenizing: {e}")))?;

        let mut id_rows = Vec::with_capacity(encodings.len());
        let mut mask_rows = Vec::with_capacity(encodings.len());
        for enc in &encodings {
            id_rows.push(tensor(enc.get_ids(), &self.device)?);
            mask_rows.push(tensor(enc.get_attention_mask(), &self.device)?);
        }
        let input_ids = Tensor::stack(&id_rows, 0).map_err(emb)?;
        let attention_mask = Tensor::stack(&mask_rows, 0).map_err(emb)?;
        let token_type_ids = input_ids.zeros_like().map_err(emb)?;

        // [batch, seq, hidden]
        let hidden = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))
            .map_err(emb)?;

        // Masked mean pooling: sum(hidden * mask) / sum(mask). Matches the
        // sentence-transformers reference and E5's expected pooling.
        let mask_f = attention_mask
            .to_dtype(DTYPE)
            .and_then(|m| m.unsqueeze(2))
            .map_err(emb)?;
        let summed = hidden.broadcast_mul(&mask_f).and_then(|x| x.sum(1)).map_err(emb)?;
        let counts = mask_f.sum(1).map_err(emb)?;
        let pooled = summed.broadcast_div(&counts).map_err(emb)?;

        let mut vectors = pooled.to_vec2::<f32>().map_err(emb)?;
        for v in &mut vectors {
            l2_normalize(v);
        }
        Ok(vectors)
    }
}

impl Embedder for E5Embedder {
    fn id(&self) -> &str {
        MODEL_ID
    }
    fn dims(&self) -> usize {
        self.dims
    }
    fn embed_passages(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let prefixed: Vec<String> = texts.iter().map(|t| format!("passage: {t}")).collect();
        self.embed(&prefixed)
    }
    fn embed_query(&self, query: &str) -> Result<Vec<f32>> {
        let out = self.embed(&[format!("query: {query}")])?;
        Ok(out.into_iter().next().unwrap_or_default())
    }
}

fn tensor(ids: &[u32], device: &Device) -> Result<Tensor> {
    Tensor::new(ids, device).map_err(emb)
}

fn emb(e: candle_core::Error) -> Error {
    Error::Embedding(e.to_string())
}
