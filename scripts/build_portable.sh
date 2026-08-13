#!/usr/bin/env bash
# Build a self-contained portable folder (exe + OCR model + Tesseract) that can
# be zipped and extracted anywhere to "install". Run from the repo root.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Source of the Tesseract runtime (from the reference build). Override with
# TESS_SRC=/path/to/Tesseract-OCR if you keep it elsewhere.
TESS_SRC="${TESS_SRC:-C:/Users/michael/Downloads/SC-Datarunner-UEX-v0.8.1/SC-Datarunner-UEX/dep/Tesseract-OCR}"
DIST="$ROOT/dist"
APPDIR="$DIST/rUEXDataRunner"

echo "==> Building release binary"
cd "$ROOT"
cargo build --release -p ruex-datarunner

echo "==> Assembling $APPDIR"
rm -rf "$APPDIR"
mkdir -p "$APPDIR/assets/tessdata" "$APPDIR/Tesseract-OCR"

cp "target/release/ruex-datarunner.exe" "$APPDIR/"
cp assets/tessdata/eng_sc.traineddata "$APPDIR/assets/tessdata/"
cp assets/tessdata/commodities.user-words "$APPDIR/assets/tessdata/"
cp assets/tessdata/terminals.user-words "$APPDIR/assets/tessdata/"
cp assets/tessdata/sc.patterns "$APPDIR/assets/tessdata/"

# Tesseract runtime: the executable plus its DLLs (training tools + docs skipped).
cp "$TESS_SRC/tesseract.exe" "$APPDIR/Tesseract-OCR/"
cp "$TESS_SRC"/*.dll "$APPDIR/Tesseract-OCR/"

# Marker: store config + data inside this folder (portable), not %APPDATA%.
echo "This marker makes rUEXDataRunner store its settings and history in this folder." > "$APPDIR/portable.txt"
mkdir -p "$APPDIR/data" "$APPDIR/config"

cat > "$APPDIR/README.txt" <<'EOF'
rUEXDataRunner — Star Citizen -> UEX datarunner (portable)

To run:
  Double-click ruex-datarunner.exe

First launch:
  Open Settings and paste your UEX Secret Key (uexcorp.space account page) and
  your UEX App API Token (create an app at uexcorp.space/api/apps). These are
  only needed to SUBMIT to UEX; the app runs and OCRs screenshots without them.
  Submissions default to DRY-RUN (nothing is sent) until you turn that off.

This folder is self-contained (the OCR engine and model are included) and
PORTABLE: your settings, saved reports, pending captures and trade log are
stored right here under config\ and data\ (thanks to the portable.txt marker).
Move or copy the whole folder and your data comes with it. To uninstall, just
delete the folder. Keep the exe together with the assets\, Tesseract-OCR\,
config\ and data\ folders.
EOF

echo "==> Contents:"
du -sh "$APPDIR" 2>/dev/null || true
echo "Portable folder ready at: $APPDIR"
echo "Zip it with: powershell Compress-Archive -Path '$APPDIR' -DestinationPath '$DIST/rUEXDataRunner.zip' -Force"
