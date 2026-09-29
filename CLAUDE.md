# CLAUDE.md

Guidance for Claude Code (and other AI assistants) working in this repository.

## What this project is

A Rust service, packaged as a Docker image, that runs forever and once per day (at `COMMIT_TIME`, UTC)
makes 1–5 random commits to a private GitHub repo on the authenticated account, so the contribution
graph shows daily activity. Each commit appends one line to a text file:

```
2026-09-28 | 2026-09-28T12:00:02.905114Z | commit 2/3
```

If the target repo doesn't exist, it is created private via `gh repo create`.

Specs live in `README.md` and `docs/`. **Treat `docs/architecture.md` as the source of truth** for
behavior; update it in the same change when behavior changes.

## Commands

```bash
cargo build                    # build
cargo test                     # tests
cargo clippy -- -D warnings    # lint (must pass)
cargo fmt                      # format (must be clean)
docker build -t auto-git-commit-tool .
```

## Hard rules

- **`gh` and `git` are hard dependencies**, invoked via `std::process::Command`. Don't add libgit2
  (`git2` crate) or a GitHub REST/GraphQL client crate. Fail at startup if either is missing or
  `gh auth status` fails.
- **UTC only.** Use `chrono::Utc` for scheduling and timestamps. Never depend on the host/container TZ.
- **Idempotent runs.** Before committing, check the last line of the log file; if it's already dated
  today (UTC), skip. Restarts must never double a day's commits.
- **Repo is always created `--private`.** Never create a public repo.
- **Never log or print `GH_TOKEN`**, and never pass it on a command line (it leaks via `ps`). Rely on
  `gh` reading it from the environment and `gh auth setup-git` for pushes.
- **Don't crash on transient failures** (network, push rejected). Log, retry with backoff, continue.
  Only config/preflight errors exit non-zero.
- **Synchronous code.** No async runtime; `std::thread::sleep` in bounded chunks is sufficient.
- Handle `SIGTERM` cleanly — the binary is PID 1 in the container.

## Conventions

- Rust edition 2024. Errors: `anyhow` at the top level, `thiserror` for module error types if needed.
- Logging: `tracing`, level via `RUST_LOG`, to stdout.
- Config only from environment variables (see `docs/configuration.md`). Add new settings there and in
  the README table.
- Keep external command invocations in thin wrappers (`github.rs`, `repo.rs`) so logic in `run.rs` and
  `scheduler.rs` can be unit tested without a network (inject a trait / fake runner).
- Pure logic worth testing: next-run computation, config validation, log-line formatting, "already ran
  today" detection.
- Commit messages: Conventional Commits, validated against commitlint `@commitlint/config-conventional`
  (see below).

## Commit message recommendation (required after every change)

At the end of **every** response that modifies files, recommend a commit message. Do not commit
unless explicitly asked — only suggest.

1. Inspect what is not yet staged/committed: `git status --short`, `git diff`, and untracked files
   (`git ls-files --others --exclude-standard`). Base the message on these changes only.
2. Write the message following commitlint `config-conventional` rules:
   - Header: `type(scope?): subject` — max **100** characters.
   - `type` is lower-case and one of: `build`, `chore`, `ci`, `docs`, `feat`, `fix`, `perf`,
     `refactor`, `revert`, `style`, `test`.
   - `scope` optional, lower-case (e.g. `scheduler`, `docker`, `readme`).
   - `subject`: imperative mood, not empty, no trailing period, not Sentence/Start/Pascal/UPPER case.
   - Breaking change: `type!:` in the header and/or a `BREAKING CHANGE:` footer.
   - Body (optional): separated from the header by a blank line, lines ≤ 100 characters, explain *why*.
   - Footer (optional): separated by a blank line (e.g. `Refs: #12`).
3. If the changes are unrelated to each other, suggest splitting them into several commits, one
   message each, with the files that belong to each.
4. Present it in a code block, ready to copy, e.g.:

   ```text
   docs(tutorials): add guide to enable private contributions

   Link it from README and deployment docs so users know commits to the
   private repo only show on the graph after opting in.
   ```

## Testing notes

- Do not hit real GitHub in unit tests.
- For manual end-to-end testing, use a throwaway `REPO_NAME` and `RUN_ON_START=true`.
