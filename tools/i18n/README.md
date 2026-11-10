# The shell's languages

Every string the daemon shows a person goes through `i18n::tr("English")`
(or `i18n::fill(tr("Connected to {name}"), &[("name", &ssid)])` for a
sentence with values in it). The English at the call site is the key and the
fallback. A catalogue is a flat JSON object, `crates/daemon/assets/i18n/<tag>.json`,
built into the binary; a file with the same name under
`~/.local/share/waverunner/i18n/` wins over it (a translator's working copy).

The language is the one chosen in the control panel (Language), else the
session's (`LC_ALL` → `LC_MESSAGES` → `LANG`). It changes live.

## The languages

| tag | language | |
|---|---|---|
| `es` | Español | whole: `cargo test` fails on a string with no Spanish |
| `de` `fr` `it` | Deutsch, Français, Italiano | since 2026-11-09, topped up in batches |
| `pt` | Português (Brasil) | Brazil's; also any other Portuguese locale |
| `pt_PT` | Português (Portugal) | Portugal's own wording, tried first for a `pt_PT` locale |

The installer offers these. A string added to the shell needs its Spanish
(the test says which); the others show it in English until the next batch:
`python3 tools/i18n/extract.py --missing crates/daemon/assets/i18n/fr.json`
lists what a catalogue lacks. Every catalogue must parse and keep its
`{holes}` (`a_translation_keeps_its_holes`). Keep each language's words for
Golem's own things the same everywhere (the dock, the bar, a box, Files,
Seam): look at how the catalogue already says them before adding.

## The tools (run from the repo root)

| | |
|---|---|
| `python3 tools/i18n/scan.py crates/daemon/src/*.rs` | literals that read as words for a person and do not reach `tr` yet |
| `python3 tools/i18n/apply.py <files>` | wrap them (`--dry` to see how many) |
| `python3 tools/i18n/extract.py` | every key, as an empty catalogue to start a language from |
| `python3 tools/i18n/extract.py --missing crates/daemon/assets/i18n/es.json` | what a catalogue lacks, and what it has that is no longer a key |

`cargo test` fails when a `tr("…")` has no Spanish, or a translation lost one
of its `{holes}`.

## Seeing what is not translatable

`WAVERUNNER_I18N=pseudo` starts the shell with every translated string
between « and ». What is on the screen without them never reaches a
translator: a missed literal, or something that is not ours to translate (a
file's name, a network's).

## Rules that keep it working

- A string that is ALSO what a pick or a comparison is told apart by (a
  setting's name, a menu's row, a session's marker) stays as written in its
  table and is translated where it is drawn: `tr(label)` at the draw site.
- A new language: add `<tag>.json` to `assets/i18n/`, its tag to `EMBEDDED`
  in `i18n.rs`, and its own name to `language_name`. A country's wording of a
  language (`pt_PT`) is a tag with its country: listed on the Language page
  beside the language, found first for that country's locale (`locale_for`).
- A table string translated where it is drawn (a setting's name, its search
  words) is not a `tr("…")` literal, so the Spanish test cannot see it: give
  its table a test of its own (`panel.rs`:
  `every_setting_and_its_words_have_their_spanish`).
- Edit `es.json` line by line, never by loading and dumping it whole: its
  entries keep the indent each session wrote them with, and a rewrite makes a
  diff of hundreds of lines.
- Not translated yet: the emoji's search words, the modules catalogue
  (`assets/modules.json`), the options engine's offer titles.
