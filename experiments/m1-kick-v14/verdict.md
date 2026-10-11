# V14 verdict: M1 kick listening

The M1 human verdict (`docs/01-inception-proposal.md` §8.4, V14: "PO judges the kick usable; verdict recorded in the repo"). The format is provisional until Q11 is decided; the location is the one §9 plans, `experiments/<id>/verdict.md` (see `docs/testing-and-evaluation.md`, "Human verdicts").

| | |
|---|---|
| Verdict | "yes I hear it, and it matches the description" |
| Given by | Owner (PO), relayed by the operator |
| Date | 2026-10-11 |
| Owner decision | sq-22037848 (M1 exit) |

## The render judged

The 60-second seed-42 render of `projects/kick` that the owner listened to (`renders/kick-seed42/audio.wav`, made in M1-7, devpanel run 2026-10-11T06-04-48-062Z-43f14f27, AC16). It is not the 16-bar golden render (`tests/golden/kick/`).

```sh
signal render projects/kick --seconds 60 --seed 42 --json
```

| Field | Value |
|---|---|
| Project | `projects/kick` |
| Seed | 42 |
| Duration | 60 s |
| Sample rate | 96000 Hz |
| Frames | 5760000 |
| `audio_fnv1a64` | `137baef63ddaac4d` |
| `log_fnv1a64` | `ecd2cc81cf0a2017` |
| `project_file_hashes` | `project.toml` `f2f484505c704250`, `composites/kick.toml` `94a3f3fcb22d33b8`, `composites/drums.toml` `2fce17af5f01d821` |

**Where the identifiers come from.** `renders/` is not tracked, so the judged render's manifest is not in the repository. The values above are from re-running the command above (with `--out` pointing outside the repository) at revision `d9926b2dfe89fb005da3e04bae64c634b4ec864d`, release build. The operator checked them against the judged render's own `renders/kick-seed42/manifest.json` on 2026-10-11: `audio_fnv1a64` and `log_fnv1a64` are identical, so the hashes identify the audio the owner heard.
