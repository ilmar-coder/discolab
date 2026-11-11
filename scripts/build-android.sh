#!/usr/bin/env bash
# Gera o APK do DiscoLab. Requisitos:
#   - rustup (https://rustup.rs) — o Rust do Homebrew não tem os alvos Android
#   - Android SDK + NDK (Android Studio > SDK Manager > NDK)
#   - Java 17 (keytool, para assinar com a chave de debug)
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ANDROID_HOME:=$HOME/Library/Android/sdk}"
export ANDROID_HOME
if [ -z "${ANDROID_NDK_ROOT:-}" ]; then
  ANDROID_NDK_ROOT="$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)"
fi
export ANDROID_NDK_ROOT
[ -d "$ANDROID_NDK_ROOT" ] || { echo "NDK não encontrado. Instale pelo Android Studio (SDK Manager > SDK Tools > NDK)."; exit 1; }
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
command -v cargo-apk >/dev/null || cargo install cargo-apk --locked
# Chave de assinatura local (criada uma vez, fora do repositório).
KS="$HOME/.android/discolab-release.keystore"; PF="$HOME/.android/discolab-release.pass"
if [ ! -f "$KS" ]; then
  mkdir -p "$HOME/.android"; openssl rand -hex 16 > "$PF"; chmod 600 "$PF"
  keytool -genkeypair -keystore "$KS" -alias discolab -keyalg RSA -keysize 2048 -validity 10000 \
    -storepass "$(cat "$PF")" -keypass "$(cat "$PF")" -dname "CN=DiscoLab"
fi
export CARGO_APK_RELEASE_KEYSTORE="$KS" CARGO_APK_RELEASE_KEYSTORE_PASSWORD="$(cat "$PF")"
cargo apk build -p discolab --lib --release
echo "APK gerado em: target/release/apk/DiscoLab.apk"
echo "Instalar no celular (depuração USB ligada): adb install -r target/release/apk/DiscoLab.apk"
