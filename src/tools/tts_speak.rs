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
            url.push_str(&format!(
                "&voice={}",
                urlencoding::encode(&self.tts_config.voice)
            ));
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
                Ok(out) if out.status.success() => {}
                _ => {
                    let stderr = String::from_utf8_lossy(&play_result.stderr);
                    anyhow::bail!("Audio playback failed: {}", stderr.trim());
                }
            }
        }

        Ok(format!(
            "Spoke: \"{}\"",
            if text.len() > 80 {
                format!("{}...", &text[..77])
            } else {
                text.to_string()
            }
        ))
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
        let text = match args["text"].as_str() {
            Some(t) => t,
            None => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some("Missing 'text' parameter".to_string()),
                });
            }
        };

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
            TtsConfig {
                enabled: true,
                endpoint: "http://localhost:9999".into(),
                ..TtsConfig::default()
            },
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
