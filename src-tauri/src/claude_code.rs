// Scans `~/.claude/projects/**/*.jsonl` for Claude Code assistant events,
// dedupes by message id, and reports usage for the active 5-hour block.
//
// Block model: a block begins at the timestamp of the first assistant message
// after a >5-hour gap, and runs for exactly 5 hours from that start. All
// assistant messages whose timestamp falls within `[start, start+5h)` count
// toward the block. The "active" block is the most recent one whose end is
// still in the future.

use std::collections::HashSet;
use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use walkdir::WalkDir;

const BLOCK_DURATION_HOURS: i64 = 5;

#[derive(Debug, Deserialize)]
struct LogLine {
    #[serde(default)]
    timestamp: Option<DateTime<Utc>>,
    #[serde(default)]
    message: Option<MessagePart>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessagePart {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize, Default, Clone)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Debug, Clone)]
struct Event {
    ts: DateTime<Utc>,
    model: String,
    usage: Usage,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct ClaudeCodeSnapshot {
    /// Total billable tokens (input + output + cache_creation; cache_read is free).
    pub block_tokens: u64,
    pub block_input: u64,
    pub block_output: u64,
    pub block_cache_creation: u64,
    pub block_cache_read: u64,
    pub block_message_count: u64,
    pub block_start: Option<DateTime<Utc>>,
    pub block_end: Option<DateTime<Utc>>,
    pub seconds_remaining: Option<i64>,
    pub models: Vec<String>,
}

pub async fn collect_snapshot(anchor: Option<DateTime<Utc>>) -> ClaudeCodeSnapshot {
    let Some(home) = dirs::home_dir() else {
        return ClaudeCodeSnapshot::default();
    };
    let root = home.join(".claude").join("projects");
    if !root.exists() {
        return ClaudeCodeSnapshot::default();
    }

    let files = jsonl_files(&root);
    let mut events: Vec<Event> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for path in files {
        if let Ok(parsed) = read_events(&path, &mut seen).await {
            events.extend(parsed);
        }
    }

    events.sort_by_key(|e| e.ts);

    // If the user has calibrated against the Claude app, honor their pinned
    // block_start until that window naturally expires.
    if let Some(start) = anchor {
        let block_dur = Duration::hours(BLOCK_DURATION_HOURS);
        if Utc::now() < start + block_dur {
            return summarize_anchored(&events, start);
        }
    }
    summarize(&events)
}

fn jsonl_files(root: &PathBuf) -> Vec<PathBuf> {
    WalkDir::new(root)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("jsonl"))
                .unwrap_or(false)
        })
        .map(|e| e.into_path())
        .collect()
}

async fn read_events(
    path: &PathBuf,
    seen: &mut HashSet<String>,
) -> std::io::Result<Vec<Event>> {
    let f = fs::File::open(path).await?;
    let reader = BufReader::new(f);
    let mut lines = reader.lines();
    let mut out = Vec::new();
    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<LogLine>(&line) else {
            continue;
        };
        if parsed.kind.as_deref() != Some("assistant") {
            continue;
        }
        let Some(message) = parsed.message else { continue };
        let Some(usage) = message.usage else { continue };
        let Some(ts) = parsed.timestamp else { continue };
        let Some(id) = message.id else { continue };
        if !seen.insert(id) {
            continue;
        }
        out.push(Event {
            ts,
            model: message.model.unwrap_or_default(),
            usage,
        });
    }
    Ok(out)
}

fn summarize_anchored(events: &[Event], start: DateTime<Utc>) -> ClaudeCodeSnapshot {
    let block_dur = Duration::hours(BLOCK_DURATION_HOURS);
    let end = start + block_dur;
    let now = Utc::now();
    let mut snap = ClaudeCodeSnapshot {
        block_start: Some(start),
        block_end: Some(end),
        seconds_remaining: Some((end - now).num_seconds().max(0)),
        ..Default::default()
    };
    let mut models: HashSet<String> = HashSet::new();
    for ev in events {
        if ev.ts < start || ev.ts >= end {
            continue;
        }
        snap.block_input += ev.usage.input_tokens;
        snap.block_output += ev.usage.output_tokens;
        snap.block_cache_creation += ev.usage.cache_creation_input_tokens;
        snap.block_cache_read += ev.usage.cache_read_input_tokens;
        snap.block_message_count += 1;
        if !ev.model.is_empty() {
            models.insert(ev.model.clone());
        }
    }
    snap.block_tokens =
        snap.block_input + snap.block_output + snap.block_cache_creation;
    snap.models = {
        let mut v: Vec<String> = models.into_iter().collect();
        v.sort();
        v
    };
    snap
}

fn summarize(events: &[Event]) -> ClaudeCodeSnapshot {
    if events.is_empty() {
        return ClaudeCodeSnapshot::default();
    }
    let block_dur = Duration::hours(BLOCK_DURATION_HOURS);

    // Walk events, grouping into blocks. A new block opens whenever the
    // current event lies at or after `block_start + 5h`.
    let mut block_start = events[0].ts;
    let mut current: Vec<&Event> = Vec::new();
    let mut blocks: Vec<(DateTime<Utc>, Vec<&Event>)> = Vec::new();

    for ev in events {
        if ev.ts >= block_start + block_dur {
            blocks.push((block_start, std::mem::take(&mut current)));
            block_start = ev.ts;
        }
        current.push(ev);
    }
    blocks.push((block_start, current));

    // Active block = the last one. If its end is in the past, no block is
    // currently active and we report zeros (but keep the latest block as
    // historical context).
    let (start, evs) = blocks.last().unwrap();
    let end = *start + block_dur;
    let now = Utc::now();
    let active = end > now;

    let mut snap = ClaudeCodeSnapshot {
        block_start: Some(*start),
        block_end: Some(end),
        seconds_remaining: if active {
            Some((end - now).num_seconds())
        } else {
            Some(0)
        },
        ..Default::default()
    };

    let mut models: HashSet<String> = HashSet::new();
    for ev in evs {
        snap.block_input += ev.usage.input_tokens;
        snap.block_output += ev.usage.output_tokens;
        snap.block_cache_creation += ev.usage.cache_creation_input_tokens;
        snap.block_cache_read += ev.usage.cache_read_input_tokens;
        snap.block_message_count += 1;
        if !ev.model.is_empty() {
            models.insert(ev.model.clone());
        }
    }
    snap.block_tokens =
        snap.block_input + snap.block_output + snap.block_cache_creation;
    snap.models = {
        let mut v: Vec<String> = models.into_iter().collect();
        v.sort();
        v
    };
    snap
}
