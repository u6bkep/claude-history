use std::fs::File;
use std::io::{self, BufRead, BufReader, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use rayon::prelude::*;
use serde::Deserialize;
use walkdir::WalkDir;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Parser, Debug)]
#[command(about = "Show Bash commands run by Claude Code (numbered, oldest first).")]
struct Args {
    /// Substring filter (case-insensitive)
    pattern: Option<String>,

    /// Show only last N entries
    #[arg(short = 'n', long = "tail", value_name = "N")]
    tail: Option<usize>,

    /// Include cwd column
    #[arg(short = 'c', long = "cwd")]
    cwd: bool,

    /// Newest first
    #[arg(short = 'r', long = "reverse")]
    reverse: bool,

    /// Deduplicate identical commands (keeps most recent)
    #[arg(short = 'u', long = "unique")]
    unique: bool,

    /// Print commands NUL-separated, no formatting (for piping)
    #[arg(short = '0', long = "null")]
    null: bool,

    /// Path to the Claude config directory (containing `projects/`).
    /// Defaults to $CLAUDE_CONFIG_DIR if set, otherwise ~/.claude.
    #[arg(long = "claude-dir", value_name = "DIR", env = "CLAUDE_CONFIG_DIR")]
    claude_dir: Option<PathBuf>,

    /// When to colorize output: auto (TTY only), always, or never.
    /// Honors NO_COLOR in `auto` mode.
    #[arg(long = "color", value_enum, default_value = "auto", value_name = "WHEN")]
    color: ColorMode,
}

const C_NUM: &str = "\x1b[2m"; // dim
const C_TS: &str = "\x1b[32m"; // green
const C_CWD: &str = "\x1b[34m"; // blue
const C_MATCH: &str = "\x1b[1;31m"; // bold red (matches grep)
const RESET: &str = "\x1b[0m";

fn use_color(mode: ColorMode) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => {
            if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
                return false;
            }
            io::stdout().is_terminal()
        }
    }
}

/// Wrap every ASCII-case-insensitive occurrence of `needle` in `text` with
/// `on`/`off`. Skips matches that don't fall on UTF-8 char boundaries.
fn highlight(text: &str, needle: &str, on: &str, off: &str) -> String {
    if needle.is_empty() {
        return text.to_owned();
    }
    let n = needle.as_bytes();
    let nlen = n.len();
    let bytes = text.as_bytes();
    if bytes.len() < nlen {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + 16);
    let mut i = 0;
    let mut last = 0;
    while i + nlen <= bytes.len() {
        if bytes[i..i + nlen].eq_ignore_ascii_case(n)
            && text.is_char_boundary(i)
            && text.is_char_boundary(i + nlen)
        {
            out.push_str(&text[last..i]);
            out.push_str(on);
            out.push_str(&text[i..i + nlen]);
            out.push_str(off);
            i += nlen;
            last = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[last..]);
    out
}

#[derive(Deserialize)]
struct Record<'a> {
    #[serde(borrow, default)]
    timestamp: Option<&'a str>,
    #[serde(borrow, default)]
    cwd: Option<&'a str>,
    #[serde(borrow, default)]
    message: Option<Message<'a>>,
}

#[derive(Deserialize)]
struct Message<'a> {
    #[serde(borrow, default)]
    content: Option<Vec<Block<'a>>>,
}

#[derive(Deserialize)]
struct Block<'a> {
    #[serde(borrow, default, rename = "type")]
    ty: Option<&'a str>,
    #[serde(borrow, default)]
    name: Option<&'a str>,
    #[serde(borrow, default)]
    input: Option<Input<'a>>,
}

#[derive(Deserialize)]
struct Input<'a> {
    #[serde(borrow, default)]
    command: Option<std::borrow::Cow<'a, str>>,
}

struct Entry {
    ts: String,
    cwd: String,
    cmd: String,
}

fn process_file(path: &std::path::Path) -> Vec<Entry> {
    let Ok(file) = File::open(path) else {
        return Vec::new();
    };
    let reader = BufReader::with_capacity(256 * 1024, file);
    let needle = memchr::memmem::Finder::new(br#""name":"Bash""#);
    let mut out = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if needle.find(line.as_bytes()).is_none() {
            continue;
        }
        let Ok(rec): Result<Record, _> = serde_json::from_str(&line) else {
            continue;
        };
        let ts = rec.timestamp.unwrap_or("").to_owned();
        let cwd = rec.cwd.unwrap_or("").to_owned();
        let Some(msg) = rec.message else { continue };
        let Some(content) = msg.content else { continue };
        for block in content {
            if block.ty != Some("tool_use") || block.name != Some("Bash") {
                continue;
            }
            let Some(input) = block.input else { continue };
            let Some(cmd) = input.command else { continue };
            if cmd.is_empty() {
                continue;
            }
            out.push(Entry {
                ts: ts.clone(),
                cwd: cwd.clone(),
                cmd: cmd.into_owned(),
            });
        }
    }
    out
}

fn fmt_ts(ts: &str) -> String {
    // ISO 8601 like "2026-05-15T15:01:02.062Z" -> "2026-05-15 15:01:02" (UTC, no conversion).
    if ts.len() >= 19 && ts.is_char_boundary(19) && ts.as_bytes()[10] == b'T' {
        let head = &ts[..10];
        let tail = &ts[11..19];
        format!("{head} {tail}")
    } else {
        " ".repeat(19)
    }
}

fn main() -> ExitCode {
    let args = Args::parse();

    let claude_dir = args.claude_dir.clone().unwrap_or_else(|| {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        home.join(".claude")
    });
    let projects = claude_dir.join("projects");
    if !projects.is_dir() {
        eprintln!("claude-history: not found: {}", projects.display());
        return ExitCode::from(1);
    }

    let files: Vec<PathBuf> = WalkDir::new(&projects)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .map(|e| e.into_path())
        .collect();

    let mut entries: Vec<Entry> = files
        .par_iter()
        .flat_map_iter(|p| process_file(p).into_iter())
        .collect();

    entries.sort_by(|a, b| a.ts.cmp(&b.ts));

    if let Some(pat) = &args.pattern {
        let needle = pat.to_lowercase();
        entries.retain(|e| e.cmd.to_lowercase().contains(&needle));
    }

    if args.unique {
        use std::collections::HashMap;
        let mut seen: HashMap<String, usize> = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            seen.insert(e.cmd.clone(), i);
        }
        let mut keep: Vec<usize> = seen.into_values().collect();
        keep.sort_unstable();
        let mut out = Vec::with_capacity(keep.len());
        for i in keep {
            out.push(Entry {
                ts: std::mem::take(&mut entries[i].ts),
                cwd: std::mem::take(&mut entries[i].cwd),
                cmd: std::mem::take(&mut entries[i].cmd),
            });
        }
        entries = out;
    }

    if let Some(n) = args.tail
        && entries.len() > n
    {
        entries.drain(..entries.len() - n);
    }

    if args.reverse {
        entries.reverse();
    }

    let stdout = io::stdout();
    let mut out = stdout.lock();

    if args.null {
        for e in &entries {
            let _ = out.write_all(e.cmd.as_bytes());
            let _ = out.write_all(&[0]);
        }
        return ExitCode::SUCCESS;
    }

    if entries.is_empty() {
        return ExitCode::SUCCESS;
    }

    let color = use_color(args.color);
    let (c_num, c_ts, c_cwd, c_match, reset) = if color {
        (C_NUM, C_TS, C_CWD, C_MATCH, RESET)
    } else {
        ("", "", "", "", "")
    };

    let width = entries.len().to_string().len();
    for (i, e) in entries.iter().enumerate() {
        let ts = fmt_ts(&e.ts);
        let mut cmd: String = e.cmd.replace('\n', " \\n ");
        if color && let Some(pat) = &args.pattern {
            cmd = highlight(&cmd, pat, c_match, reset);
        }
        let n = i + 1;
        let res = if args.cwd {
            writeln!(
                out,
                "{c_num}{n:>width$}{reset}  {c_ts}{ts}{reset}  {c_cwd}{cwd}{reset}  {cmd}",
                cwd = e.cwd,
            )
        } else {
            writeln!(out, "{c_num}{n:>width$}{reset}  {c_ts}{ts}{reset}  {cmd}")
        };
        if res.is_err() {
            break;
        }
    }
    ExitCode::SUCCESS
}
