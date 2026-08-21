# rae-parse

Build-time tool that turns the RAE (Real Academia Española) plain-text dictionary
dump into the compact JSON the waverunner clipboard **dictionary panel** loads
(`crate::dict`).

It is **not** part of the cargo workspace — it's a single dependency-free Rust
file compiled with `rustc`, invoked by the flake's `dictionaries` package.

## What it does

For each `->headword…` line it:

- recovers **all homonyms** (RAE marks them `word1`, `word2`, … — glued or as
  ` (1)`) and merges them under the bare headword, separated by ` · `;
- keeps the numbered senses up to the first ` ~ ` (RAE **locutions** are dropped);
- extracts the parenthesised **etymology** (`(Del lat. …)`).

Output value per word is a plain definition string, or
`{"e": etymology, "d": definition}` when an etymology was found. The daemon's
loader accepts either form (and also the English file, which is plain strings).

## Data sources (pinned in `flake.nix`)

- **English** — `dictionary.json`: Webster's 1913 (public domain), from
  [matthewreagan/WebstersEnglishDictionary] `dictionary_compact.json` (copied
  as-is; no etymology in this set).
- **Español** — `dictionary-es.json`: real RAE definitions, parsed by this tool
  from [eneko98/RAE-Corpus]
  `RealAcademiaEspanola-DiccionarioLlenguaEspanola.txt`.

Both are fetched at pinned commits with pinned hashes and assembled by
`nix build .#dictionaries`, which installs them to
`$out/share/waverunner/{dictionary,dictionary-es}.json`. The daemon wrapper (and
`waverunner-dev`) point `$WAVERUNNER_DICT` / `$WAVERUNNER_DICT_ES` at them.

## Run standalone

```
rustc -O parse_rae.rs -o parse_rae
./parse_rae RealAcademiaEspanola-DiccionarioLlenguaEspanola.txt dictionary-es.json
```

[matthewreagan/WebstersEnglishDictionary]: https://github.com/matthewreagan/WebstersEnglishDictionary
[eneko98/RAE-Corpus]: https://github.com/eneko98/RAE-Corpus
