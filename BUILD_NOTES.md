# Slideflow Build Notes

## Python Bundle

The app uses a bundled PyInstaller executable (`merge_pptx`) to merge PowerPoint slides. This bundle includes all necessary Python dependencies and does not require users to have Python installed.

### Building the App

**Option 1: Build with bundle copy**
```bash
./build_and_copy_bundle.sh
```
This script builds the app and ensures the Python bundle is properly copied to the app's Resources folder.

**Option 2: Manual build in Xcode**
After building in Xcode, run:
```bash
./copy_python_bundle.sh
```
to copy the Python bundle to the built app.

### Regenerating the Python Bundle

If you need to rebuild the Python merger:
```bash
cd slideflow/Scripts
python3 -m PyInstaller --onedir --name merge_pptx --distpath ../../merge_pptx_bundle merge_pptx.py
```

**Important**: The Python bundle is stored in `merge_pptx_bundle/` at the project root, **outside** the `slideflow/` folder. This prevents Xcode from adding the bundle's dylib paths to LIBRARY_SEARCH_PATHS.

### App Sandbox

The app is currently configured **without App Sandbox** (`com.apple.security.app-sandbox = false`) to allow:
- Execution of bundled Python binary
- Access to user-selected PowerPoint files
- File system access for export

This means the app **cannot be distributed via Mac App Store** but can be distributed directly as a standalone app.

### Requirements

- macOS 15+ (Sequoia)
- Xcode 16+
- Python 3.x with python-pptx (only for rebuilding the bundle)
