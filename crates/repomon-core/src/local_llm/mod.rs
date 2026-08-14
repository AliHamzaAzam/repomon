//! Local LLM inference subsystem for repomon.
//!
//! Provides pure-Rust, zero-idle-memory local LLM capabilities across macOS, Linux, and Windows
//! using `candle` and quantized GGUF models. Tensors and model weights are loaded on demand per
//! invocation and immediately dropped upon completion, ensuring zero resident background RAM.

use std::fs::File;
use std::path::PathBuf;

use candle_core::quantized::gguf_file;
use candle_core::{Device, Tensor};
use candle_transformers::generation::LogitsProcessor;
use candle_transformers::models::quantized_qwen2::ModelWeights as Qwen2Model;
use hf_hub::api::sync::Api;
use hf_hub::Repo;
use serde::{Deserialize, Serialize};
use tokenizers::Tokenizer;

use crate::error::{Error, Result};

pub const DEFAULT_MODEL_REPO: &str = "Qwen/Qwen2.5-0.5B-Instruct-GGUF";
pub const DEFAULT_MODEL_FILE: &str = "qwen2.5-0.5b-instruct-q4_k_m.gguf";
pub const DEFAULT_TOKENIZER_REPO: &str = "Qwen/Qwen2.5-0.5B-Instruct";
pub const DEFAULT_TOKENIZER_FILE: &str = "tokenizer.json";

/// Configuration for local LLM inference tasks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalLlmConfig {
    pub model_repo: String,
    pub model_file: String,
    pub tokenizer_repo: String,
    pub tokenizer_file: String,
    pub temperature: f64,
    pub max_tokens: usize,
    pub enabled: bool,
}

impl Default for LocalLlmConfig {
    fn default() -> Self {
        Self {
            model_repo: DEFAULT_MODEL_REPO.to_string(),
            model_file: DEFAULT_MODEL_FILE.to_string(),
            tokenizer_repo: DEFAULT_TOKENIZER_REPO.to_string(),
            tokenizer_file: DEFAULT_TOKENIZER_FILE.to_string(),
            temperature: 0.0,
            max_tokens: 16,
            enabled: true,
        }
    }
}

/// Resolves model and tokenizer files from HuggingFace cache or downloads them if not present.
pub fn resolve_model_files(config: &LocalLlmConfig) -> Result<(PathBuf, PathBuf)> {
    let api = Api::new().map_err(|e| Error::LocalLlm(format!("HF Hub API initialization failed: {e}")))?;
    let model_repo = api.repo(Repo::model(config.model_repo.clone()));
    let model_path = model_repo
        .get(&config.model_file)
        .map_err(|e| Error::LocalLlm(format!("Failed to retrieve model weight {}: {e}", config.model_file)))?;

    let tokenizer_repo = api.repo(Repo::model(config.tokenizer_repo.clone()));
    let tokenizer_path = tokenizer_repo
        .get(&config.tokenizer_file)
        .map_err(|e| Error::LocalLlm(format!("Failed to retrieve tokenizer {}: {e}", config.tokenizer_file)))?;

    Ok((model_path, tokenizer_path))
}

/// Runs a single one-shot inference against the local GGUF model and drops all weights upon return.
///
/// Guaranteed 0 MB resident RAM when idle.
pub fn generate_oneshot(prompt: &str, config: Option<&LocalLlmConfig>) -> Result<String> {
    let default_cfg = LocalLlmConfig::default();
    let cfg = config.unwrap_or(&default_cfg);

    if !cfg.enabled {
        return Err(Error::LocalLlm("Local LLM subsystem is disabled".into()));
    }

    let (model_path, tokenizer_path) = resolve_model_files(cfg)?;
    let device = Device::Cpu;

    let mut file = File::open(&model_path)
        .map_err(|e| Error::LocalLlm(format!("Failed to open model file at {}: {e}", model_path.display())))?;
    let content = gguf_file::Content::read(&mut file)
        .map_err(|e| Error::LocalLlm(format!("Failed to read GGUF content: {e}")))?;
    let mut model = Qwen2Model::from_gguf(content, &mut file, &device)
        .map_err(|e| Error::LocalLlm(format!("Failed to build Qwen2 model from GGUF: {e}")))?;

    let tokenizer = Tokenizer::from_file(&tokenizer_path)
        .map_err(|e| Error::LocalLlm(format!("Failed to load tokenizer: {e}")))?;

    let tokens = tokenizer
        .encode(prompt, true)
        .map_err(|e| Error::LocalLlm(format!("Failed to tokenize prompt: {e}")))?;
    let prompt_tokens = tokens.get_ids().to_vec();
    if prompt_tokens.is_empty() {
        return Err(Error::LocalLlm("Prompt produced 0 tokens".into()));
    }

    let mut logits_processor = LogitsProcessor::new(
        299792458,
        if cfg.temperature > 0.0 { Some(cfg.temperature) } else { None },
        None,
    );

    let eos_token = tokenizer.token_to_id("<|im_end|>").unwrap_or(151645);
    let mut generated_tokens: Vec<u32> = Vec::new();

    let input = Tensor::new(&prompt_tokens[..], &device)
        .and_then(|t| t.unsqueeze(0))
        .map_err(|e| Error::LocalLlm(format!("Tensor creation failed: {e}")))?;

    let logits = model
        .forward(&input, 0)
        .and_then(|l| l.squeeze(0))
        .map_err(|e| Error::LocalLlm(format!("Initial model forward pass failed: {e}")))?;

    let mut next_token = logits_processor
        .sample(&logits)
        .map_err(|e| Error::LocalLlm(format!("Logits sampling failed: {e}")))?;

    for index in 0..cfg.max_tokens {
        if next_token == eos_token {
            break;
        }
        generated_tokens.push(next_token);

        let step_input = Tensor::new(&[next_token], &device)
            .and_then(|t| t.unsqueeze(0))
            .map_err(|e| Error::LocalLlm(format!("Step tensor creation failed: {e}")))?;

        let step_logits = model
            .forward(&step_input, prompt_tokens.len() + index)
            .and_then(|l| l.squeeze(0))
            .map_err(|e| Error::LocalLlm(format!("Step forward pass failed: {e}")))?;

        next_token = logits_processor
            .sample(&step_logits)
            .map_err(|e| Error::LocalLlm(format!("Step sampling failed: {e}")))?;
    }

    // Explicit drop ensures zero lingering GPU/CPU memory allocations:
    drop(model);

    let decoded = tokenizer
        .decode(&generated_tokens, false)
        .map_err(|e| Error::LocalLlm(format!("Failed to decode output tokens: {e}")))?;

    Ok(decoded.trim().to_string())
}

/// Sanitizes a raw LLM output into a clean, concise 2-4 word kebab-case slug.
pub fn sanitize_session_slug(raw: &str) -> Option<String> {
    let text = raw.trim();
    if text.is_empty() {
        return None;
    }

    // Take the first non-empty line
    let first_line = text.lines().find(|l| !l.trim().is_empty())?.trim();

    // Strip common wrapping quotes or markdown formatting
    let cleaned = first_line
        .trim_matches(|c| c == '`' || c == '"' || c == '\'' || c == '#' || c == '*' || c == ':')
        .trim();

    let mut slug = String::with_capacity(cleaned.len());
    let mut prev_is_hyphen = true; // prevent leading hyphen

    for ch in cleaned.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_is_hyphen = false;
        } else if (ch.is_whitespace() || ch == '-' || ch == '_' || ch == '/') && !prev_is_hyphen {
            slug.push('-');
            prev_is_hyphen = true;
        }
    }

    // Strip trailing hyphen
    while slug.ends_with('-') {
        slug.pop();
    }

    if slug.len() < 2 {
        return None;
    }

    // Limit maximum length to 36 characters
    if slug.len() > 36 {
        if let Some(idx) = slug[..36].rfind('-') {
            slug.truncate(idx);
        } else {
            slug.truncate(36);
        }
    }

    if slug.len() >= 2 {
        Some(slug)
    } else {
        None
    }
}

/// Formats a user prompt into a ChatML session naming prompt.
pub fn format_naming_prompt(user_prompt: &str) -> String {
    // Truncate long prompts to first 400 characters to keep prompt small and fast
    let sample = if user_prompt.len() > 400 {
        &user_prompt[..400]
    } else {
        user_prompt
    };

    format!(
        "<|im_start|>system\nYou are a developer session namer. Convert the user request into a concise 2-4 word lowercase kebab-case slug (e.g. fix-auth-tokens, redesign-fleet-sidebar, add-mcp-tools). Output ONLY the slug without any other words.<|im_end|>\n<|im_start|>user\nTask: {sample}<|im_end|>\n<|im_start|>assistant\n"
    )
}

/// Generates a concise, meaningful session slug from a developer prompt using the local LLM.
pub fn generate_session_slug(user_prompt: &str) -> Result<String> {
    let prompt = format_naming_prompt(user_prompt);
    let mut cfg = LocalLlmConfig::default();
    cfg.max_tokens = 14;

    let raw = generate_oneshot(&prompt, Some(&cfg))?;
    sanitize_session_slug(&raw)
        .ok_or_else(|| Error::LocalLlm(format!("Failed to parse clean slug from response: '{raw}'")))
}

/// Asynchronously generates a session slug on a blocking threadpool.
pub async fn generate_session_slug_async(user_prompt: String) -> Result<String> {
    tokio::task::spawn_blocking(move || generate_session_slug(&user_prompt))
        .await
        .map_err(|e| Error::LocalLlm(format!("Local LLM spawn_blocking task panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slug_sanitization() {
        assert_eq!(sanitize_session_slug("fix-auth-tokens"), Some("fix-auth-tokens".into()));
        assert_eq!(sanitize_session_slug("\"redesign-fleet-sidebar\""), Some("redesign-fleet-sidebar".into()));
        assert_eq!(sanitize_session_slug("`fix_login_flow`"), Some("fix-login-flow".into()));
        assert_eq!(sanitize_session_slug("Fix the desktop UI tab labels"), Some("fix-the-desktop-ui-tab-labels".into()));
        assert_eq!(sanitize_session_slug("  \n\nfix-claude-code\nSome explanation"), Some("fix-claude-code".into()));
        assert_eq!(sanitize_session_slug(""), None);
        assert_eq!(sanitize_session_slug("---"), None);
    }

    #[test]
    fn test_format_naming_prompt() {
        let p = format_naming_prompt("Add tests for local LLM");
        assert!(p.contains("<|im_start|>system"));
        assert!(p.contains("Task: Add tests for local LLM"));
        assert!(p.ends_with("<|im_start|>assistant\n"));
    }
}
