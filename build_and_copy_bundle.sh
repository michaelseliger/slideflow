#!/bin/bash
# Build app and ensure Python bundle is copied

set -e

echo "🔨 Building slideflow..."
xcodebuild -project slideflow.xcodeproj -scheme slideflow -configuration Debug build

echo "📦 Copying Python bundle..."
SRCROOT="$(pwd)"
BUILT_PRODUCTS_DIR="$HOME/Library/Developer/Xcode/DerivedData/slideflow-hkwmslxmzizynxelmavtusjcdhvf/Build/Products/Debug"
PRODUCT_NAME="slideflow"

SOURCE="${SRCROOT}/merge_pptx_bundle"
DEST="${BUILT_PRODUCTS_DIR}/${PRODUCT_NAME}.app/Contents/Resources/merge_pptx"

rm -rf "${DEST}"
cp -R "${SOURCE}" "${DEST}"

echo "✅ Build complete! Python bundle installed."
echo "📍 App location: ${BUILT_PRODUCTS_DIR}/${PRODUCT_NAME}.app"
