# Audio Play Tool — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add an `audio_play` tool to ZeroClaw core so the agent can play WAV/audio files through USB speakers on `hannah.local` (Arduino UNO Q). This is the foundation for future TTS-from-API integration.

**Architecture:** New `AudioConfig` in config schema, new `AudioPlayTool` implementing the `Tool` trait, registered in `all_tools()` when `audio.enabled = true`. The tool shells out to `aplay` (ALSA) with a configurable device name, with `paplay` (PulseAudio) as fallback. Follows the same pattern as `BrowserConfig`/`BrowserTool` gating.

**Tech Stack:** Rust, `tokio::process::Command` for `aplay`/`paplay`, ALSA on Debian Linux (Arduino UNO Q).

---

## Hardware Discovery Notes (hannah.local)

**Board:** Arduino UNO Q (SKU ABX00162-ABX00173)
- Qualcomm Dragonwing QRB2210 MPU, quad-core Cortex-A53 @ 2.0 GHz
- Debian Linux OS, 2/4 GB LPDDR4, 16/32 GB eMMC
- USB-C with host/device role switching
- Native audio on JMISC header (Headphone OUT / Line OUT / Ear OUT)
- Wi-Fi 5 dual-band, Bluetooth 5.1

**Datasheet:** https://docs.arduino.cc/resources/datasheets/ABX00162-ABX00173-datasheet.pdf

**Audio devices detected via `aplay -l`:**

| Card | ALSA Device | Hardware | Role |
|------|-------------|----------|------|
| 0 | `plughw:0,0` | reSpeaker XVF3800 4-Mic Array (Seeed) | Microphone input |
| 1 | `plughw:1,0` | GEMBIRD USB Speaker (VID 1908:1331) | **Speaker output** |
| 2 | `plughw:2,0` | Arduino-Imola-HPH-LOUT (onboard Qualcomm codec) | Headphone/Line out (JMISC) |

**Verified working:**
- `speaker-test -D plughw:1,0 -t sine -f 440 -l 1 -p 2` → 440 Hz tone plays
- `aplay -D plughw:1,0 /tmp/test_tone.wav` → WAV playback works
- Volume control: `amixer -c 1 sset 'PCM' 80%` → works

**SSH access:** `arduino@hannah.local`

---

## Task 1: Add `AudioConfig` to config schema

**Files:**
- Modify: `src/config/schema.rs`

**Step 1: Add the `AudioConfig` struct and field**

After the `HardwareConfig` struct (around line 198), add:

```rust
// ── Audio ────────────────────────────────────────────────────────

/// Audio playback configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioConfig {
    /// Enable audio playback tools.
    #[serde(default)]
    pub enabled: bool,

    /// ALSA playback device (e.g. "plughw:1,0", "default").
    /// Found via `aplay -l` on the target device.
    #[serde(default = "AudioConfig::default_device")]
    pub speaker_device: String,

    /// Volume (0–100). Applied via amixer on playback.
    #[serde(default = "AudioConfig::default_volume")]
    pub volume: u8,
}

impl AudioConfig {
    fn default_device() -> String {
        "default".to_string()
    }
    fn default_volume() -> u8 {
        80
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            speaker_device: Self::default_device(),
            volume: Self::default_volume(),
        }
    }
}
```

Add the field to `Config` struct (after `pub hardware: HardwareConfig,`, around line 143):

```rust
    #[serde(default)]
    pub audio: AudioConfig,
```

**Step 2: Verify it compiles**

Run: `cargo check 2>&1 | tail -5`
Expected: compiles with no errors

**Step 3: Commit**

```bash
git add src/config/schema.rs
git commit -m "feat(config): add AudioConfig for speaker playback"
```

---

## Task 2: Create `audio_play` tool

**Files:**
- Create: `src/tools/audio_play.rs`
- Modify: `src/tools/mod.rs`

**Step 1: Create the tool file**

Create `src/tools/audio_play.rs`:

```rust
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
```

**Step 2: Register in `src/tools/mod.rs`**

Add the module declaration (after `pub mod web_search_tool;`, line 29):

```rust
pub mod audio_play;
```

Add the re-export (after `pub use web_search_tool::WebSearchTool;`, line 62):

```rust
pub use audio_play::AudioPlayTool;
```

Add to `all_tools_with_runtime()` — after the web search block (around line 204), before the vision tools block:

```rust
    // Audio playback tool (enabled via [audio] config)
    if root_config.audio.enabled {
        tools.push(Box::new(AudioPlayTool::new(root_config.audio.clone())));
    }
```

**Step 3: Run tests**

Run: `cargo test --lib tools::audio_play -- --nocapture 2>&1 | tail -10`
Expected: 3 tests pass

**Step 4: Verify full build**

Run: `cargo check 2>&1 | tail -5`
Expected: compiles clean

**Step 5: Commit**

```bash
git add src/tools/audio_play.rs src/tools/mod.rs
git commit -m "feat(tools): add audio_play tool for speaker playback"
```

---

## Task 3: Register tool description in agent loop

**Files:**
- Modify: `src/agent/loop_.rs`

**Step 1: Add tool description for LLM**

In `src/agent/loop_.rs`, after the peripheral tool_descs block (after the closing `}` around line 1333), add:

```rust
    if config.audio.enabled {
        tool_descs.push((
            "audio_play",
            "Play audio through connected speaker. Use when: user asks to play a sound, test speakers, or play audio. Can play WAV files or generate test tones.",
        ));
    }
```

**Step 2: Verify build**

Run: `cargo check 2>&1 | tail -5`
Expected: compiles clean

**Step 3: Commit**

```bash
git add src/agent/loop_.rs
git commit -m "feat(agent): register audio_play tool description"
```

---

## Task 4: Add config.toml documentation

**Files:**
- Modify: `README.md` (in the Configuration section)

**Step 1: Add `[audio]` example to the config block**

In `README.md`, in the Configuration section (around the `[browser]` block), add:

```toml
[audio]
enabled = false                # opt-in audio playback
speaker_device = "plughw:1,0"  # ALSA device (find with: aplay -l)
volume = 80                    # 0-100
```

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: add [audio] config section to README"
```

---

## Task 5: Deploy and test on hannah.local

**Step 1: Build for the target**

```bash
cargo build --release --locked
```

Or cross-compile for aarch64 if building on Mac:

```bash
CC_aarch64_unknown_linux_gnu=aarch64-unknown-linux-gnu-gcc \
  cargo build --release --target aarch64-unknown-linux-gnu
```

**Step 2: Deploy to hannah**

```bash
scp target/aarch64-unknown-linux-gnu/release/zeroclaw arduino@hannah.local:~/
ssh arduino@hannah.local "sudo mv ~/zeroclaw /usr/local/bin/"
```

**Step 3: Add audio config on hannah**

```bash
ssh arduino@hannah.local
# Edit config
nano ~/.zeroclaw/config.toml
```

Add:

```toml
[audio]
enabled = true
speaker_device = "plughw:1,0"
volume = 80
```

**Step 4: Test via CLI agent**

```bash
ssh arduino@hannah.local
zeroclaw agent -m "Play a test tone"
zeroclaw agent -m "Play a 880Hz tone for 3 seconds"
```

Expected: sound plays through GEMBIRD USB speaker.

**Step 5: Test via Telegram**

Message the bot: "Play a test tone on the speaker"

Expected: agent calls `audio_play` with `test_tone: true`, sound plays, agent confirms.

---

## Task 6: Update zeroclaw.md notes

**Files:**
- Modify: `/Users/toddgruben/Projects/notes/zeroclaw.md`

Add an "Audio Playback" section documenting:
- Hardware discovery results (3 audio cards, GEMBIRD on card 1)
- Config snippet
- Working ALSA commands
- UNO Q datasheet link and audio capabilities

---

## Future (not in this plan)

- TTS API integration: receive audio stream from API → save to WAV → play via `audio_play`
- Volume control tool (`audio_volume`)
- Audio input/recording via reSpeaker mic array
- Streaming playback (pipe audio directly without temp file)
