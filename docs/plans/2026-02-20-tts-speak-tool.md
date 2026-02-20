# TTS Speak Tool — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `tts_speak` tool that calls the Spark Voice Server TTS API, downloads the WAV, and plays it through hannah's USB speakers.

**Architecture:** New `TtsConfig` in config schema, new `TtsSpeakTool` implementing `Tool`. Uses `reqwest` to POST to `/tts`, saves WAV to temp file, plays via `aplay` using the `AudioConfig.speaker_device`. Registered in `all_tools()` when `tts.enabled = true`.

**Tech Stack:** Rust, `reqwest` (already a dependency), `tokio::process::Command` for `aplay`.

**Server:** Spark Voice Server at `192.168.1.205:8800` — Chatterbox TTS on DGX Spark (NVIDIA GB10, 122 GB GPU). Endpoint: `POST /tts?text=...&voice=...&exaggeration=0.5&cfg_weight=0.5` → returns `audio/wav`.

---

## Task 1: Add TtsConfig to config schema

**Files:** `src/config/schema.rs`

After the `AudioConfig` impl blocks, add:

```rust
// ── TTS ──────────────────────────────────────────────────────────

/// Text-to-speech configuration (remote TTS server).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    /// Enable TTS speak tool.
    #[serde(default)]
    pub enabled: bool,

    /// TTS server base URL (e.g. "http://192.168.1.205:8800").
    #[serde(default)]
    pub endpoint: String,

    /// Voice name for voice cloning (empty = default voice).
    #[serde(default)]
    pub voice: String,

    /// Expressiveness (0.0–2.0, default 0.5).
    #[serde(default = "TtsConfig::default_exaggeration")]
    pub exaggeration: f32,

    /// CFG weight (0.0–1.0, default 0.5).
    #[serde(default = "TtsConfig::default_cfg_weight")]
    pub cfg_weight: f32,
}

impl TtsConfig {
    fn default_exaggeration() -> f32 {
        0.5
    }
    fn default_cfg_weight() -> f32 {
        0.5
    }
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: String::new(),
            voice: String::new(),
            exaggeration: Self::default_exaggeration(),
            cfg_weight: Self::default_cfg_weight(),
        }
    }
}
```

Add field to `Config` struct (after `pub audio: AudioConfig,`):

```rust
    /// Text-to-speech server configuration.
    #[serde(default)]
    pub tts: TtsConfig,
```

Fix any compilation errors (manual `Config` construction sites may need `tts: TtsConfig::default()`).

Verify: `cargo check`

Commit: `feat(config): add TtsConfig for remote TTS server`

---

## Task 2: Create tts_speak tool

**Files:** Create `src/tools/tts_speak.rs`, modify `src/tools/mod.rs`

### src/tools/tts_speak.rs

```rust
//! TTS speak tool — synthesize speech via remote TTS server and play through speaker.
//!
//! Calls the Spark Voice Server `/tts` endpoint, saves the WAV response,
//! and plays it via `aplay` using the configured speaker device.

use crate::config::{AudioConfig, TtsConfig};
use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;

pub struct TtsSpeakTool {
    tts_config: TtsConfig,
    speaker_device: String,
}

impl TtsSpeakTool {
    pub fn new(tts_config: TtsConfig, audio_config: &AudioConfig) -> Self {
        Self {
            tts_config,
            speaker_device: audio_config.speaker_device.clone(),
        }
    }

    async fn speak(&self, text: &str) -> anyhow::Result<String> {
        if text.is_empty() {
            anyhow::bail!("Cannot speak empty text");
        }
        if text.len() > 2000 {
            anyhow::bail!("Text too long (max 2000 characters)");
        }
        if self.tts_config.endpoint.is_empty() {
            anyhow::bail!("TTS endpoint not configured");
        }

        // Build TTS request URL
        let mut url = format!(
            "{}/tts?text={}&exaggeration={}&cfg_weight={}",
            self.tts_config.endpoint.trim_end_matches('/'),
            urlencoding::encode(text),
            self.tts_config.exaggeration,
            self.tts_config.cfg_weight,
        );
        if !self.tts_config.voice.is_empty() {
            url.push_str(&format!("&voice={}", urlencoding::encode(&self.tts_config.voice)));
        }

        // Fetch WAV from TTS server
        let client = reqwest::Client::new();
        let response = client
            .post(&url)
            .timeout(std::time::Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("TTS request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            anyhow::bail!("TTS server returned {}: {}", status, body);
        }

        let wav_bytes = response
            .bytes()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to read TTS audio: {}", e))?;

        // Save to temp file
        let tmp_path = PathBuf::from("/tmp/zeroclaw_tts_speech.wav");
        tokio::fs::write(&tmp_path, &wav_bytes).await?;

        // Play via aplay
        let play_result = tokio::process::Command::new("aplay")
            .args(["-D", &self.speaker_device, tmp_path.to_str().unwrap()])
            .output()
            .await?;

        if !play_result.status.success() {
            // Fallback: paplay
            let fallback = tokio::process::Command::new("paplay")
                .arg(tmp_path.to_str().unwrap())
                .output()
                .await;

            match fallback {
                Ok(out) if out.status.success() => {},
                _ => {
                    let stderr = String::from_utf8_lossy(&play_result.stderr);
                    anyhow::bail!("Audio playback failed: {}", stderr.trim());
                }
            }
        }

        Ok(format!("Spoke: \"{}\"", if text.len() > 80 {
            format!("{}...", &text[..77])
        } else {
            text.to_string()
        }))
    }
}

#[async_trait]
impl Tool for TtsSpeakTool {
    fn name(&self) -> &str {
        "tts_speak"
    }

    fn description(&self) -> &str {
        "Speak text out loud through the speaker using text-to-speech. \
         Use when: user asks you to say something, speak, announce, or \
         when a voice response is appropriate."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to speak out loud (max 2000 chars)"
                }
            },
            "required": ["text"]
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let text = args["text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'text' parameter"))?;

        match self.speak(text).await {
            Ok(msg) => Ok(ToolResult {
                success: true,
                output: msg,
                error: None,
            }),
            Err(e) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(e.to_string()),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AudioConfig, TtsConfig};

    #[test]
    fn tool_name_and_schema() {
        let tool = TtsSpeakTool::new(TtsConfig::default(), &AudioConfig::default());
        assert_eq!(tool.name(), "tts_speak");
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["text"].is_object());
        assert_eq!(schema["required"][0], "text");
    }

    #[tokio::test]
    async fn empty_text_returns_error() {
        let tool = TtsSpeakTool::new(
            TtsConfig { enabled: true, endpoint: "http://localhost:9999".into(), ..TtsConfig::default() },
            &AudioConfig::default(),
        );
        let result = tool.execute(json!({ "text": "" })).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("empty"));
    }

    #[tokio::test]
    async fn no_endpoint_returns_error() {
        let tool = TtsSpeakTool::new(TtsConfig::default(), &AudioConfig::default());
        let result = tool.execute(json!({ "text": "hello" })).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("endpoint"));
    }

    #[tokio::test]
    async fn missing_text_param_returns_error() {
        let tool = TtsSpeakTool::new(TtsConfig::default(), &AudioConfig::default());
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("Missing"));
    }
}
```

### src/tools/mod.rs changes

1. After `pub mod audio_play;` add: `pub mod tts_speak;`
2. After `pub use audio_play::AudioPlayTool;` add: `pub use tts_speak::TtsSpeakTool;`
3. In `all_tools_with_runtime()`, after the `audio_play` block add:

```rust
    // TTS speak tool (enabled via [tts] config, requires [audio] for speaker device)
    if root_config.tts.enabled && !root_config.tts.endpoint.is_empty() {
        tools.push(Box::new(TtsSpeakTool::new(
            root_config.tts.clone(),
            &root_config.audio,
        )));
    }
```

Verify: `cargo test --lib tools::tts_speak` (4 tests pass), `cargo check`

Commit: `feat(tools): add tts_speak tool for remote TTS playback`

---

## Task 3: Register tool description in agent loop

**Files:** `src/agent/loop_.rs`

After the `audio_play` tool_descs block (search for `"audio_play"`), add:

```rust
    if config.tts.enabled && !config.tts.endpoint.is_empty() {
        tool_descs.push((
            "tts_speak",
            "Speak text out loud through the speaker using text-to-speech. Use when: user asks you to say something, speak, announce, or when a voice response is appropriate.",
        ));
    }
```

Verify: `cargo check`

Commit: `feat(agent): register tts_speak tool description`

---

## Task 4: Add config docs and deploy

**Files:** `README.md`

After the `[audio]` config block, add:

```toml
[tts]
enabled = false                          # opt-in text-to-speech
endpoint = "http://192.168.1.205:8800"   # Spark Voice Server
voice = ""                               # voice name (empty = default)
exaggeration = 0.5                       # 0.0-2.0
cfg_weight = 0.5                         # 0.0-1.0
```

Commit: `docs: add [tts] config section to README`

---

## Task 5: Build, deploy, and test on hannah.local

1. Cross-compile: `PATH="$HOME/.rustup/toolchains/1.92.0-aarch64-apple-darwin/bin:$PATH" CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-musl-gcc CC_aarch64_unknown_linux_musl=aarch64-linux-musl-gcc cargo build --release --target aarch64-unknown-linux-musl --no-default-features`
2. Deploy: `scp target/aarch64-unknown-linux-musl/release/zeroclaw arduino@hannah.local:~/zeroclaw_new && ssh arduino@hannah.local 'mv ~/zeroclaw ~/zeroclaw_old2 && mv ~/zeroclaw_new ~/zeroclaw && chmod +x ~/zeroclaw'`
3. Add config on hannah:
```toml
[tts]
enabled = true
endpoint = "http://192.168.1.205:8800"
voice = ""
exaggeration = 0.5
cfg_weight = 0.5
```
4. Add `tts_speak` to auto_approve
5. Test: `~/zeroclaw agent -m "Say hello world out loud"`
