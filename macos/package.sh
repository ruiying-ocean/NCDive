#!/bin/sh
# Build a self-contained NCDive.app and zip it for a GitHub release.
# Needs: brew install netcdf proj pkg-config dylibbundler
set -eu
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
app=target/release/NCDive.app
zip="target/release/NCDive-$version-macos-$(uname -m).zip"

cargo build --release
rm -rf "$app" "$zip"
mkdir -p "$app/Contents/MacOS"
cp target/release/ncdive "$app/Contents/MacOS/"
cp macos/Info.plist "$app/Contents/"

# Copy NetCDF, HDF5, PROJ and their own dependencies into the bundle,
# so users don't need Homebrew.
dylibbundler -cd -of -b \
    -x "$app/Contents/MacOS/ncdive" \
    -d "$app/Contents/Frameworks" \
    -p @executable_path/../Frameworks/

# Free ad-hoc signature: Apple Silicon won't run unsigned code. Not notarized,
# so users still confirm the first launch in System Settings.
codesign --force --sign - "$app"/Contents/Frameworks/*.dylib
codesign --force --sign - "$app"

ditto -c -k --keepParent "$app" "$zip"
echo "Created $zip"
