# claude-history

Like `history`, but for [Claude Code](https://claude.com/claude-code).

Searches every Bash command Claude has ever run on your behalf — across all
projects, all sessions — by walking the JSONL transcripts under
`~/.claude/projects/`. Prints them numbered, oldest first, the way the
familiar `history` builtin does.

Useful when you've asked Claude to do something a little fiddly in the shell
(convert some images, peel apart a tarball, run a one-off `ffmpeg` incantation)
and now you want to run the same thing again without re-prompting an agent.

## Install

```sh
cargo install --git https://github.com/u6bkep/claude-history
```

Or from a local clone:

```sh
cargo install --path .
```

The binary lands in `~/.cargo/bin/claude-history` — make sure that's on your
`PATH`.

## Usage

```text
claude-history [OPTIONS] [PATTERN]

Arguments:
  [PATTERN]  Substring filter (case-insensitive)

Options:
  -n, --tail <N>   Show only last N entries
  -c, --cwd        Include cwd column
  -r, --reverse    Newest first
  -u, --unique     Deduplicate identical commands (keeps most recent)
  -0, --null       Print commands NUL-separated, no formatting (for piping)
  -h, --help       Print help
```

### Examples

Show everything (numbered, oldest first):

```sh
claude-history
```

Find a command you ran via Claude a couple weeks ago:

```sh
claude-history ffmpeg
```

Last 20 unique commands, newest first, with the cwd they ran in:

```sh
claude-history -u -n 20 -r -c
```

Re-run the most recent matching command. The `-0` flag emits NUL-separated
output so commands with embedded quotes or newlines survive the pipe:

```sh
claude-history -u -n 1 -0 "ffmpeg.*webm" | xargs -0 -I{} sh -c '{}'
```

## Notes

- Output is sorted by transcript timestamp. Timestamps are printed in **UTC**.
- Only `tool_use` blocks with `name == "Bash"` are extracted. Commands Claude
  merely *suggested* in chat (but didn't run) are not included.
- Sub-agent (sidechain) Bash calls are included.
- Each line of the JSONL is checked for the substring `"name":"Bash"` before
  any JSON parsing, so most of the transcript is skipped at memory-copy speed.
  File walks are parallelized with rayon.

## License

MIT
