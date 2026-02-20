#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ANDROID_SDK="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"

if [ -f "$ROOT/web/package.json" ]; then
  (cd "$ROOT/web" && npm install && npm run lint && npm run typecheck && npm run test)
fi

if [ -f "$ROOT/flutter/pubspec.yaml" ]; then
  if command -v flutter >/dev/null 2>&1; then
    (cd "$ROOT/flutter" && flutter test)
  else
    echo "flutter is not installed. Skipping Flutter tests."
  fi
fi

if [ -f "$ROOT/android/gradlew" ]; then
  if command -v java >/dev/null 2>&1 && [ -n "${ANDROID_SDK}" ] && [ -d "${ANDROID_SDK}" ]; then
    (cd "$ROOT/android" && ./gradlew test)
  else
    echo "Android toolchain not configured (java/ANDROID_SDK_ROOT). Skipping Android tests."
  fi
elif [ -f "$ROOT/gradlew" ]; then
  if command -v java >/dev/null 2>&1 && [ -n "${ANDROID_SDK}" ] && [ -d "${ANDROID_SDK}" ]; then
    (cd "$ROOT" && ./gradlew test)
  else
    echo "Android toolchain not configured (java/ANDROID_SDK_ROOT). Skipping Android tests."
  fi
fi

if [ -f "$ROOT/ios/Package.swift" ]; then
  if command -v swift >/dev/null 2>&1 && [ "$(uname -s)" = "Darwin" ]; then
    (cd "$ROOT/ios" && swift test)
  else
    echo "swift toolchain unavailable for iOS tests. Skipping iOS tests."
  fi
fi
