# Profile widgets

A curated collection of image widgets you can drop into any Markdown file (typically your
[profile README](https://docs.github.com/en/account-and-profile/setting-up-and-managing-your-github-profile/customizing-your-profile/managing-your-profile-readme),
the `<username>/<username>` repository) to show off the contribution activity this tool produces.

- Replace `USERNAME` in every snippet with your GitHub login.
- Previews below use the author's account (`VillegasMich`) so the file renders live on GitHub.
- A ready-to-paste profile README combining the best ones: [`profile-readme-template.md`](profile-readme-template.md).
- Workflow files for the self-generated widgets: [`workflows/`](workflows/).

## Before you start

**Private contributions.** This tool commits to a *private* repo. A widget only reflects those
commits if (a) you enabled
[private contributions](../docs/tutorials/enable-private-contributions.md) on your profile and
(b) the widget reads your **contribution calendar** (the green-squares graph), not just public
repositories. The *Private commits* column below says which ones do.

**Hosted vs. self-generated.** There are two kinds of widgets:

| Kind | How it works | Reliability |
| --- | --- | --- |
| **Hosted** | An `<img>` pointing at someone's public server that renders an SVG on request. | Free public instances often hit their hosting quota (HTTP 402/503) or disappear. Self-host on Vercel if you depend on one. |
| **Self-generated** | A scheduled GitHub Action in your profile repo renders the SVG and commits it. The `<img>` points at your own repo. | Very reliable; updates on the Action's schedule. |

**Caching.** GitHub proxies every image through its camo cache, and most services cache for a few
hours. A new day's commits can take a while to show up in a widget, even though the profile graph
itself updates within minutes.

## Overview

Status checked on 2026-09-28 against the public instances.

| Widget | Kind | Shows | Private commits | Public instance |
| --- | --- | --- | --- | --- |
| [GitHub Readme Streak Stats](#streak-stats) | Hosted | Total contributions, current and longest streak | Yes | ✅ up |
| [ghchart](#contribution-chart-ghchart) | Hosted | Contribution calendar as an image, custom color | Yes | ✅ up |
| [GitHub Profile Summary Cards](#profile-summary-cards) | Hosted | Contribution graph, commits by hour, languages | Yes (graph) | ✅ up |
| [GitHub Readme Stats](#github-readme-stats) | Hosted | Stars, commits, PRs, issues, rank; top languages | Only self-hosted with a PAT | ⚠️ official instance down (503), forks up |
| [GitHub Profile Trophy](#profile-trophy) | Hosted | Achievement trophies | Partly | ⚠️ official down (402), mirror up |
| [Awesome GitHub Stats](#awesome-github-stats) | Hosted | Stats card with level/rank | Partly | ✅ up |
| [GitHub Readme Activity Graph](#activity-graph) | Hosted | Line chart of daily contributions | Yes | ❌ public down (402), self-host only |
| [Profile views counter](#profile-views-counter) | Hosted | Profile view counter badge | n/a | ✅ up |
| [Shields.io badges](#shieldsio-badges) | Hosted | Followers, stars, per-repo commit activity | No (public repos only) | ✅ up |
| [Contribution snake](#contribution-snake) | Self-generated | Snake eating your contribution graph | Yes | n/a |
| [3D contribution calendar](#3d-contribution-calendar) | Self-generated | Isometric 3D contribution graph + stats | Yes | n/a |
| [lowlighter/metrics](#lowlightermetrics) | Self-generated | 30+ plugins: isometric calendar, habits, languages… | Yes (with PAT) | n/a |

---

## Hosted widgets

### Streak stats

[DenverCoder1/github-readme-streak-stats](https://github.com/DenverCoder1/github-readme-streak-stats)
— the best match for this tool: a daily commit keeps the **current streak** growing.

![Streak stats](https://streak-stats.demolab.com?user=VillegasMich&theme=dark&hide_border=true)

```markdown
![GitHub Streak](https://streak-stats.demolab.com?user=USERNAME&theme=dark&hide_border=true)
```

Useful parameters: `theme` (`dark`, `radical`, `tokyonight`, `github-dark-blue`, … — see the
[theme list](https://github.com/DenverCoder1/github-readme-streak-stats/blob/main/docs/themes.md)),
`hide_border=true`, `mode=weekly` (weekly streaks), `date_format=j%20M%5B%20Y%5D`, `type=png`.

### Contribution chart (ghchart)

[2016rshah/githubchart-api](https://github.com/2016rshah/githubchart-api) — your contribution
calendar as a plain SVG, with an optional base color (hex, no `#`).

![Contribution chart](https://ghchart.rshah.org/409ba5/VillegasMich)

```markdown
![Contribution chart](https://ghchart.rshah.org/USERNAME)
![Contribution chart](https://ghchart.rshah.org/409ba5/USERNAME)
```

### Profile summary cards

[vn7n24fzkq/github-profile-summary-cards](https://github.com/vn7n24fzkq/github-profile-summary-cards)
— several cards; `profile-details` includes a contribution graph and `productive-time` shows at
which hour you commit (set `utcOffset=0`: this tool commits at `COMMIT_TIME` UTC).

![Profile details](https://github-profile-summary-cards.vercel.app/api/cards/profile-details?username=VillegasMich&theme=github_dark)

![Productive time](https://github-profile-summary-cards.vercel.app/api/cards/productive-time?username=VillegasMich&theme=github_dark&utcOffset=0)

```markdown
![Profile details](https://github-profile-summary-cards.vercel.app/api/cards/profile-details?username=USERNAME&theme=github_dark)
![Stats](https://github-profile-summary-cards.vercel.app/api/cards/stats?username=USERNAME&theme=github_dark)
![Productive time](https://github-profile-summary-cards.vercel.app/api/cards/productive-time?username=USERNAME&theme=github_dark&utcOffset=0)
![Repos per language](https://github-profile-summary-cards.vercel.app/api/cards/repos-per-language?username=USERNAME&theme=github_dark)
![Most commit language](https://github-profile-summary-cards.vercel.app/api/cards/most-commit-language?username=USERNAME&theme=github_dark)
```

### GitHub Readme Stats

[anuraghazra/github-readme-stats](https://github.com/anuraghazra/github-readme-stats) — the
classic stats card and top-languages card. The official public instance
(`github-readme-stats.vercel.app`) returned **503** at the time of writing; use the maintained
fork [GitHub Stats Extended](https://github.com/stats-organization/github-stats-extended) or deploy
your own on Vercel (one click from the project README).

![Stats](https://github-stats-extended.vercel.app/api?username=VillegasMich&show_icons=true&theme=github_dark&hide_border=true)

```markdown
![Stats](https://github-stats-extended.vercel.app/api?username=USERNAME&show_icons=true&theme=github_dark&hide_border=true)
![Top languages](https://github-stats-extended.vercel.app/api/top-langs?username=USERNAME&layout=compact&theme=github_dark&hide_border=true)
```

To count this tool's private commits in *Total Commits*, you must **self-host** with your own
`PAT_1` (classic PAT, `repo` + `read:user`) and add `&count_private=true`. Public instances only
see public data.

### Profile trophy

[ryo-ma/github-profile-trophy](https://github.com/ryo-ma/github-profile-trophy) — achievement
trophies (commits, stars, followers, years…). The official instance returns **402**; the
`github-trophies.vercel.app` mirror works, or self-host.

![Trophies](https://github-trophies.vercel.app/?username=VillegasMich&theme=onedark&no-frame=true&column=7)

```markdown
![Trophies](https://github-trophies.vercel.app/?username=USERNAME&theme=onedark&no-frame=true&column=7)
```

### Awesome GitHub Stats

[brunobritodev/awesome-github-stats](https://github.com/brunobritodev/awesome-github-stats) —
alternative stats card with a level/rank badge.

![Awesome stats](https://awesome-github-stats.azurewebsites.net/user-stats/VillegasMich?theme=dark)

```markdown
![Awesome stats](https://awesome-github-stats.azurewebsites.net/user-stats/USERNAME?theme=dark)
```

### Activity graph

[Ashutosh00710/github-readme-activity-graph](https://github.com/Ashutosh00710/github-readme-activity-graph)
— a line chart of daily contributions over the last 31 days; the steady 1–5 commits/day from this
tool show as a continuous line. The public instance currently returns **402**, so self-host it
(Vercel, needs a `TOKEN`) and use your own domain:

```markdown
![Activity graph](https://<your-deployment>.vercel.app/graph?username=USERNAME&theme=github-compact&hide_border=true)
```

### Profile views counter

[antonkomarev/github-profile-views-counter](https://github.com/antonkomarev/github-profile-views-counter).

![Profile views](https://komarev.com/ghpvc/?username=VillegasMich&label=Profile%20views&color=0e75b6&style=flat)

```markdown
![Profile views](https://komarev.com/ghpvc/?username=USERNAME&label=Profile%20views&color=0e75b6&style=flat)
```

### Shields.io badges

[shields.io](https://shields.io/badges) — small badges. They use public data only, so they can't
read the private repo this tool commits to; use them for public repos and account stats.

![Followers](https://img.shields.io/github/followers/VillegasMich?style=social)
![Commit activity](https://img.shields.io/github/commit-activity/m/VillegasMich/auto-git-commit-tool)

```markdown
![Followers](https://img.shields.io/github/followers/USERNAME?style=social)
![Commit activity](https://img.shields.io/github/commit-activity/m/USERNAME/PUBLIC_REPO)
![Last commit](https://img.shields.io/github/last-commit/USERNAME/PUBLIC_REPO)
```

---

## Self-generated widgets (GitHub Actions)

These run in your **profile repository** (`USERNAME/USERNAME`), not in this project. Copy the
workflow from [`workflows/`](workflows/) to `.github/workflows/` in that repo, run it once
manually (*Actions → workflow → Run workflow*), then add the snippet to its README.

Schedule them a bit **after** your `COMMIT_TIME` so each day's commits are already in the graph.

### Contribution snake

[Platane/snk](https://github.com/Platane/snk) — a snake that eats your contribution graph.
Workflow: [`workflows/snake.yml`](workflows/snake.yml) (publishes to an `output` branch).

```html
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/USERNAME/USERNAME/output/github-snake-dark.svg" />
  <source media="(prefers-color-scheme: light)" srcset="https://raw.githubusercontent.com/USERNAME/USERNAME/output/github-snake.svg" />
  <img alt="Contribution snake" src="https://raw.githubusercontent.com/USERNAME/USERNAME/output/github-snake.svg" />
</picture>
```

### 3D contribution calendar

[yoshi389111/github-profile-3d-contrib](https://github.com/yoshi389111/github-profile-3d-contrib)
— isometric 3D graph with stats and language breakdown, in several themes (`green`, `season`,
`night-view`, `night-rainbow`, `gitblock`, animated variants…).
Workflow: [`workflows/profile-3d-contrib.yml`](workflows/profile-3d-contrib.yml).

With the default `GITHUB_TOKEN` it sees private contributions as counts (if enabled on your
profile); for per-repo/language details of private repos, set a PAT secret as described in the
workflow file.

```markdown
![3D contributions](./profile-3d-contrib/profile-night-rainbow.svg)
```

### lowlighter/metrics

[lowlighter/metrics](https://github.com/lowlighter/metrics) — the most configurable option:
one SVG with 30+ plugins (isometric calendar, coding habits, languages, achievements…).
Workflow: [`workflows/metrics.yml`](workflows/metrics.yml). Needs a classic PAT stored as the
`METRICS_TOKEN` secret (scopes: `read:user`, plus `repo` to include private repositories).

```markdown
![Metrics](./github-metrics.svg)
```

---

## Tips

- **Dark/light mode.** Wrap any widget in `<picture>` with `prefers-color-scheme` sources (see the
  snake example) or append `#gh-dark-mode-only` / `#gh-light-mode-only` to the image URL.
- **Side by side.** Put two `<img>` tags on one line, or use `<p align="center">…</p>`.
- **Link the widget.** `[![Streak](...)](https://github.com/DenverCoder1/github-readme-streak-stats)` makes the image clickable.
- **Don't depend on one public instance.** If a hosted widget breaks, switch to a mirror from the
  table or self-host it.

## Adding a widget to this collection

Add a row to the overview table, a section with a live preview and a copy-ready snippet using
`USERNAME`, and say whether it reflects private contributions. Update the check date when you
re-verify the public instances.
