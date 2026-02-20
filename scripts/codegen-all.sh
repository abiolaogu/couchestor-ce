#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_SDK="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"

if [ -f "$ROOT/web/package.json" ] && [ -f "$ROOT/web/codegen.ts" ]; then
  (
    cd "$ROOT/web"
    if [ -f package-lock.json ]; then
      npm ci
    else
      npm install
    fi
    npm run codegen
  )
fi

if [ -f "$ROOT/flutter/pubspec.yaml" ]; then
  if command -v flutter >/dev/null 2>&1; then
    (cd "$ROOT/flutter" && flutter pub get && flutter pub run build_runner build --delete-conflicting-outputs)
  else
    echo "flutter is not installed. Skipping Flutter codegen."
  fi
fi

if [ -f "$ROOT/android/gradlew" ]; then
  if command -v java >/dev/null 2>&1 && [ -n "${ANDROID_SDK}" ] && [ -d "${ANDROID_SDK}" ]; then
    (cd "$ROOT/android" && ./gradlew :app:generateApolloSources)
  else
    echo "Android toolchain not configured (java/ANDROID_SDK_ROOT). Skipping Android Apollo codegen."
  fi
elif [ -f "$ROOT/gradlew" ]; then
  if command -v java >/dev/null 2>&1 && [ -n "${ANDROID_SDK}" ] && [ -d "${ANDROID_SDK}" ]; then
    (cd "$ROOT" && ./gradlew :app:generateApolloSources)
  else
    echo "Android toolchain not configured (java/ANDROID_SDK_ROOT). Skipping Android Apollo codegen."
  fi
fi

if [ -f "$ROOT/ios/apollo-codegen-config.json" ]; then
  if command -v swift >/dev/null 2>&1 && [ "$(uname -s)" = "Darwin" ]; then
    (cd "$ROOT/ios" && swift package resolve)
  else
    echo "swift toolchain unavailable for iOS codegen. Skipping iOS package resolve."
  fi
fi
