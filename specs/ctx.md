# Current Context

Plan: ship a reusable LLM skill that teaches any agent, in any
project, how to build a `capp-rs`-based web crawler against a target
the user names. The skill is the deliverable; the Hacker News
crawler is the smoke test that proves the skill works.

## Context

- `capp-rs` provides the durable queue + mailbox runtime + tower
  service stack needed for crawlers. What's missing is a written
  recipe an LLM can follow to assemble those pieces against an
  arbitrary target site.
- `pageinfo-rs` (`pginf` CLI) already exists and is the recon tool
  of choice: HTTP-only page probing, link extraction, metadata,
  structured-data sniffing, CSS-selector preview. The user installs
  it themselves (`cargo install pageinfo-rs`); the skill assumes it
  is on PATH.
- Fjall is a workspace dep and `FjallTaskQueue` is the default
  queue backend in `capp-queue`. Crawlers get a durable queue and a
  place to write parsed records (separate partitions in the same
  keyspace) for free.
- HN is just the first concrete site we drive the skill against to
  validate it. The skill must not assume HN or any specific layout.

## Decisions

- **Deliverable is `skills/capp.md`.** One file, site-agnostic,
  pginf-style frontmatter. No scaffolding, no run commands, no
  project-layout assumptions — the consuming project decides where
  the crawler lives.
- **Recon = `pginf`.** The skill instructs the LLM to use
  `pginf fetch|links|meta|json|html -s …` to investigate the target.
  Skill does *not* shell-wrap or proxy pginf; the user has it
  installed and on PATH.
- **No CLI for `capp-rs`.** Earlier draft proposed a `capp` CLI that
  proxied pginf and scaffolded projects. Dropped — out of scope.
- **No scaffolding template.** The skill describes the crawler
  shape (typed `Task` enum, Fjall partitions, mailbox runtime, tower
  layers) in prose + minimal snippets. The LLM writes the code in
  whatever project layout the user already has.
- **Justfile recipe:** one new recipe that copies `skills/capp.md`
  into `.agent/` (the location agents look in by default). Nothing
  else.
- **HN smoke test:** built at `examples/hackernews/` because that's
  where capp-rs's own examples live. Stories + comments into Fjall
  partitions. Run as a normal `cargo run --example` — no
  capp-rs-specific runner.
- **If the skill is unclear or missing steps during the HN run, fix
  the skill, not the crawler.** The crawler is the test, not the
  product.

## Open questions

None blocking. Flag anything to change before I draft the skill.

## Critical files

- `skills/capp.md` — NEW. The skill artifact. Site-agnostic.
- `Justfile` — add one recipe to install the skill into `.agent/`.
- `examples/hackernews/` — smoke-test crawler built via the skill.
  Existing `hn_config.toml` and `hn_uas.txt` may be reused or
  replaced as the skill dictates.

## Design

### `skills/capp.md` — outline

Frontmatter (pginf-style):

```yaml
---
name: capp
description: Build a capp-rs crawler against a user-named target. Use pginf to recon the site, then assemble queue + mailbox + tower into a durable crawler.
argument-hint: <target-url>
allowed-tools: Bash
---
```

Body sections (short, action-oriented):

1. **Trigger.** User names a target site and wants a `capp-rs`
   crawler.
2. **Recon checklist.** What to run with `pginf` and what to record:
   - `pginf fetch <url>` — reachable? status? headers?
   - `pginf links <url> --format toon` — URL groups, pagination
     shape, internal vs external.
   - `pginf meta <url>` — title, canonical, feeds.
   - `pginf json <url>` — JSON-LD / Next.js data (cheap structured
     data; prefer over HTML scraping when present).
   - `pginf html -u <url> -s "<selector>"` — preview CSS selectors
     against real HTML before committing.
   - Output: a short notes section with paths, selectors,
     pagination, rate-limit signals.
3. **Crawler shape.** What to assemble:
   - Typed `Task` enum, one variant per page kind discovered in
     recon (e.g. `Listing { page }`, `Item { id }`).
   - `FjallTaskQueue<Task, JsonSerializer>` for the queue.
   - Additional Fjall partitions for parsed records (one per
     entity, keyed by stable id).
   - Tower service stack with `timeout` + a rate-limit layer.
   - `spawn_mailbox_runtime` to drive workers.
   - Seed with entry URLs; let workers fan out by enqueueing
     discovered tasks.
4. **Hand-off checklist.** Before the LLM declares done, it reports
   to the user: recon findings, chosen selectors, Fjall partition
   table, the run command the user should invoke.
5. **Out-of-scope flags.** JS-rendered sites, login flows, robots.txt
   enforcement — the skill says "ask the user" rather than guessing.

### Justfile recipe

```just
# Install the capp-rs crawler skill into .agent/skills/
install-skill:
    mkdir -p .agent/skills
    cp skills/capp.md .agent/skills/capp.md
```

### HN smoke test (`examples/hackernews/`)

Built by following the skill against `https://news.ycombinator.com`.
Expected output of the skill's recon step: HN is server-rendered,
listing pages are `/news?p=N`, item pages are `/item?id=ID`,
comments inlined on item pages. Crawler stores stories and comments
in two Fjall partitions alongside the queue partition.

## Verification

- `skills/capp.md` exists with the frontmatter and sections above.
- `just install-skill` copies the file into `.agent/skills/`.
- Following the skill end-to-end against HN produces a runnable
  example under `examples/hackernews/` that crawls and persists
  stories + comments to Fjall.
- If any step of the skill is ambiguous during the HN run, the fix
  lands in `skills/capp.md`, not the example.

## Out of scope

- `capp-rs` CLI binary.
- Any shell wrapping or proxying of `pginf`.
- Project scaffolding / `cargo new`-style templates.
- Run-command conventions — consuming projects pick their own.
- JS rendering, browser automation, auth flows, robots.txt logic.
