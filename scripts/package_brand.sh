#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
swift scripts/export_brand.swift
swift scripts/verify_brand.swift
for direction in horizon convergence; do
    /usr/bin/ditto -c -k --keepParent "assets/brand/$direction" "assets/brand/kiri-$direction.zip"
    /usr/bin/unzip -tq "assets/brand/kiri-$direction.zip"
    cp "assets/brand/kiri-$direction.zip" "assets/brand/kiri-$direction-v2.zip"
done
cp assets/brand/horizon/desktop/png/icon-1024.png assets/brand/horizon-v2.png
