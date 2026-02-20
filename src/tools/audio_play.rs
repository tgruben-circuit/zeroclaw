//! Audio playback tool — play WAV/audio files through speakers.
//!
//! Uses `aplay` (ALSA) with configurable device. Falls back to `paplay` (PulseAudio).
//! Designed for USB speakers on Arduino UNO Q / Raspberry Pi.

use crate::config::AudioConfig;
use crate::tools::traits::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;

pub struct AudioPlayTool {
    config: AudioConfig,
}

impl AudioPlayTool {
    pub fn new(config: AudioConfig) -> Self {
        Self { config }
    }

    /// Play a WAV file through the configured speaker device.
    async fn play_file(&self, file_path: &str) -> anyhow::Result<String> {
        let path = Path::new(file_path);
        if !path.exists() {
            anyhow::bail!("Audio file not found: {}", file_path);
        }

        // Try aplay first (ALSA — works on all Linux)
        let result = tokio::process::Command::new("aplay")
            .args(["-D", &self.config.speaker_device, file_path])
            .output()
            .await?;

        if result.status.success() {
            return Ok(format!("Played: {}", file_path));
        }

        // Fallback: paplay (PulseAudio)
        let fallback = tokio::process::Command::new("paplay")
            .arg(file_path)
            .output()
            .await;

        match fallback {
            Ok(out) if out.status.success() => Ok(format!("Played (pulse): {}", file_path)),
            _ => {
                let stderr = String::from_utf8_lossy(&result.stderr);
                anyhow::bail!(
                    "Audio playback failed on device '{}'. aplay error: {}",
                    self.config.speaker_device,
                    stderr.trim()
                )
            }
        }
    }

    /// Generate and play a test tone (uses speaker-test).
    async fn play_test_tone(&self, frequency: u32, duration_secs: u32) -> anyhow::Result<String> {
        let freq = frequency.clamp(100, 10000);
        let dur = duration_secs.clamp(1, 10);

        let result = tokio::process::Command::new("speaker-test")
            .args([
                "-D",
                &self.config.speaker_device,
                "-t",
                "sine",
                "-f",
                &freq.to_string(),
                "-l",
                "1",
                "-p",
                &dur.to_string(),
            ])
            .output()
            .await?;

        if result.status.success() {
            Ok(format!("Played {}Hz tone for {}s", freq, dur))
        } else {
            let stderr = String::from_utf8_lossy(&result.stderr);
            anyhow::bail!("speaker-test failed: {}", stderr.trim())
        }
    }
}

#[async_trait]
impl Tool for AudioPlayTool {
    fn name(&self) -> &str {
        "audio_play"
    }

    fn description(&self) -> &str {
        "Play audio through the connected speaker. Can play a WAV file by path, \
         or generate a test tone at a given frequency. Use when: user asks to \
         play a sound, test speakers, or play audio."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "Path to a WAV file to play (e.g. /tmp/sound.wav)"
                },
                "test_tone": {
                    "type": "boolean",
                    "description": "Play a test tone instead of a file (default false)"
                },
                "frequency": {
                    "type": "integer",
                    "description": "Test tone frequency in Hz (default 440, range 100-10000)"
                },
                "duration": {
                    "type": "integer",
                    "description": "Test tone duration in seconds (default 2, max 10)"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> anyhow::Result<ToolResult> {
        let is_test = args["test_tone"].as_bool().unwrap_or(false);

        if is_test {
            let freq = args["frequency"].as_u64().unwrap_or(440) as u32;
            let dur = args["duration"].as_u64().unwrap_or(2) as u32;
            match self.play_test_tone(freq, dur).await {
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
        } else if let Some(file) = args["file"].as_str() {
            match self.play_file(file).await {
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
        } else {
            Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(
                    "Provide 'file' (path to WAV) or set 'test_tone' to true".to_string(),
                ),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AudioConfig;

    #[test]
    fn tool_name_and_schema() {
        let tool = AudioPlayTool::new(AudioConfig::default());
        assert_eq!(tool.name(), "audio_play");
        let schema = tool.parameters_schema();
        assert!(schema["properties"]["file"].is_object());
        assert!(schema["properties"]["test_tone"].is_object());
    }

    #[tokio::test]
    async fn missing_file_returns_error() {
        let tool = AudioPlayTool::new(AudioConfig {
            enabled: true,
            speaker_device: "default".to_string(),
            volume: 80,
        });
        let result = tool
            .execute(json!({ "file": "/nonexistent/sound.wav" }))
            .await
            .unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn no_args_returns_error() {
        let tool = AudioPlayTool::new(AudioConfig::default());
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("Provide"));
    }
}
