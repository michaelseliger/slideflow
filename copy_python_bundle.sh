#!/bin/bash
# Copy PyInstaller bundle to app Resources

SOURCE="${SRCROOT}/merge_pptx_bundle"
DEST="${BUILT_PRODUCTS_DIR}/${PRODUCT_NAME}.app/Contents/Resources/merge_pptx"
FRAMEWORKS="${BUILT_PRODUCTS_DIR}/${PRODUCT_NAME}.app/Contents/Frameworks"

echo "Copying Python bundle..."
echo "Source: ${SOURCE}"
echo "Dest: ${DEST}"

# Remove existing if present
rm -rf "${DEST}"

# Copy entire directory
cp -R "${SOURCE}" "${DEST}"

# Copy required dylibs to app Frameworks folder to satisfy @rpath
echo "Copying Python dylibs to Frameworks..."
mkdir -p "${FRAMEWORKS}"

# Copy all dylibs from Python bundle's PIL/.dylibs
if [ -d "${SOURCE}/_internal/PIL/.dylibs" ]; then
    cp "${SOURCE}"/_internal/PIL/.dylibs/*.dylib "${FRAMEWORKS}/" 2>/dev/null || true
fi

# Copy any other dylibs from _internal
if [ -d "${SOURCE}/_internal" ]; then
    find "${SOURCE}/_internal" -name "*.dylib" -exec cp {} "${FRAMEWORKS}/" \; 2>/dev/null || true
fi

echo "✅ Python bundle and dependencies copied"
