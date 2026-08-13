# rUEXDataRunner

A Rust rebuild of [SC-Datarunner-UEX](https://github.com/Shebuka/SC-Datarunner-UEX) — a
Windows desktop app that turns Star Citizen trade-terminal screenshots into UEX
commodity data submissions.

Press **Print Screen** at a trading terminal in-game; the app watches your
screenshots folder, OCRs each capture with a Star-Citizen-tuned Tesseract model,
extracts the terminal, buy/sell type, and each commodity's stock/quantity/price,
and lets you review, correct, and submit to [UEX](https://uexcorp.space).

## Download & run (Windows)

1. Go to the [**Releases**](../../releases) page and download **`rUEXDataRunner.zip`**
   from the latest release. (No Release yet? Download the artifact from the latest
   [**Actions**](../../actions) run instead.)
2. **Unblock** it: right-click the zip → **Properties** → tick **Unblock** → **OK**.
   (Skips Windows' "downloaded from the internet" warnings on the extracted files.)
3. **Extract**: right-click the zip → **Extract All…** → pick a folder you can write
   to (e.g. `Documents`).
4. Open the extracted `rUEXDataRunner` folder and double-click **`ruex-datarunner.exe`**.
   Optionally right-click it → **Send to → Desktop (create shortcut)**.
5. First launch may show **"Windows protected your PC"** (the app is unsigned) —
   click **More info → Run anyway**. This is expected for a small unsigned app; it's
   a publisher-reputation prompt, not a virus warning.

The zip is **self-contained** (the OCR engine and model are bundled — nothing else
to install) and **portable**: your settings, saved reports, pending captures, and
trade log are stored *inside the folder* (`config\` and `data\`). Move the folder
and your data comes with it; to uninstall, delete the folder.

To submit to UEX (optional — the app runs and OCRs without it), open **Settings**
and add your **UEX Secret Key** (from your UEX account page) and a **UEX App API
Token** (create an app at [uexcorp.space/api/apps](https://uexcorp.space/api/apps)).
Submissions default to **dry-run** (nothing is sent) until you turn that off.

## What's new vs. the original

**Saved reports with in-app updates.** Every submission is saved locally with its
status and the UEX report ids it produced. Open the **Reports** tab to review a
past submission, correct a value, and **re-submit an update** — without visiting
the UEX website. The corrected report supersedes the original while the full
history is kept.

## Features

- **Folder watching + background OCR** — new screenshots are processed
  automatically; the UI never blocks while Tesseract runs.
- **Reuses the original's trained model** (`eng_sc.traineddata`) via the bundled
  Tesseract binary — no C build dependency, the app just shells out.
- **Fuzzy matching** of terminal and commodity names against a snapshot of the
  UEX reference lists (embedded, works offline).
- **Editable review grid** with per-field confidence colouring (green/amber/red),
  a terminal dropdown for co-located kiosks, buy/sell toggle, price/SCU/stock
  editing, and per-row include checkboxes.
- **Terminal carry-over** — scroll-continuation screenshots inherit the last
  identified terminal.
- **Safe by default** — starts in **dry-run** mode: payloads are built and
  validated but nothing is sent until you turn dry-run off in Settings.
- **Send** a single screenshot or **Send All** eligible ones in a batch.

## Project layout

```
crates/core   # all logic (OCR wrapper, extraction, matching, UEX API, storage) — heavily unit-tested
crates/app    # egui/eframe desktop UI
assets/
  tessdata/   # eng_sc.traineddata + user word/pattern lists (from the original)
  reference/  # commodities.json + terminals.json (UEX snapshots, embedded at build)
```

## Build & run

Requires a Rust toolchain (built/tested with 1.96).

```bash
cargo run --release -p ruex-datarunner
```

The release binary is written to `target/release/ruex-datarunner.exe`.

### First-run configuration (Settings tab)

- **UEX Secret Key** — from your [UEX account page](https://uexcorp.space/account/home/tab/account_main#panel-secret-key). Only needed for live sends.
- **Screenshots folder** — defaults to
  `C:\Program Files\Roberts Space Industries\StarCitizen\LIVE\screenshots`.
- **Tesseract executable / Tessdata folder** — auto-detected from the bundled
  reference build and this repo's `assets/tessdata`. Override via the
  `RUEX_TESSERACT` / `RUEX_ASSETS` environment variables or the Settings fields.
- **Dry-run** — leave on to try the whole flow safely; turn off to submit live.

## How submission maps to UEX

`POST {base_url}/data_submit` with a `secret-key` header. For a **buy** terminal
the app fills `price_buy` / `scu_buy` / `status_buy`; for a **sell** terminal the
`*_sell` fields. Stock labels map to UEX's 1–7 inventory scale
(out=1, low=3, medium=4, high=5, max=7). Prices are parsed from the in-game
`¤N.NNK/SCU` notation (K/M scaled, rounded to whole aUEC).

## Testing

```bash
cargo test --workspace
```

- Deterministic unit tests cover pricing, stock-status mapping, fuzzy matching,
  reference lookup, payload construction, response parsing, config, and the
  report save/update flow.
- `crates/core/tests/golden_ocr.rs` runs the **real** Tesseract pipeline over
  bundled screenshots and checks structural invariants. It auto-skips when
  Tesseract isn't available (set `RUEX_TESSERACT` to force it).

## OCR pipeline & accuracy

Each screenshot is processed with **two OCR passes that are merged**:

1. **Raw full frame** — reads the (often coloured/holographic) terminal header in
   the left panel, so the location name is identified reliably.
2. **Rectified commodity panel** — the right-hand commodity panel is located by
   its bright orange border ([`deskew::detect_panel`](crates/core/src/deskew.rs)),
   **perspective-warped flat**, upscaled and contrast-stretched. Isolating the
   panel from the busy 3D scene and flattening the in-game monitor's curve turns
   the thin orange text into crisp white-on-black, which reads prices/quantities
   far more accurately. If no panel is confidently found, it falls back to a
   grayscale, 2×-upscaled full-frame pass.

[`extract::merge`](crates/core/src/extract.rs) keeps the terminal from pass 1 and
the commodity rows from the rectified panel, associating each card's
stock/quantity/price by **spatial proximity** (a vertical band per card).
Processing is ~3 s per 4K screenshot in a release build, on a background thread
so the UI never blocks.

Prices render in game as `¤6,561/SCU` (comma thousands); the parser strips
separators and also tolerates OCR reading the comma as a period. Some fields on
tilted or low-contrast captures still need a quick correction — **low-confidence
fields are flagged amber/red** in the grid for exactly that.

> Tesseract itself is fast (~2 s/pass); the app shells out to the bundled
> **Tesseract v5.5** with the custom `eng_sc` model. Debug helpers:
> `cargo run -p datarunner-core --example rectify -- <img> <out_dir>` saves a
> detected-quad overlay and the warped panel; `--example dump_ocr` prints the
> full extraction.

## Disclaimer

Not affiliated with UEX Corporation or Roberts Space Industries. Reference data
in `assets/reference/` are snapshots of public UEX API endpoints.
