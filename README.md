# Slideflow - PowerPoint Slide Organizer

A macOS native application for indexing, searching, and organizing PowerPoint slides.

## Features

✨ **Directory Indexing**: Automatically scan and index PowerPoint files from configured directories
🔍 **Full-Text Search**: Search slides by content with filters for presentation name, location, and date
📁 **Workspace Management**: Create collections of slides from different presentations with drag-and-drop reordering
📤 **Export**: Generate new PowerPoint files from your curated slide collections

## Quick Start

### Prerequisites

- macOS 15+ (Sequoia)
- Xcode 16.0+
- Homebrew

### Installation

1. **Install LibreOffice** (required for thumbnail generation):
   ```bash
   brew install --cask libreoffice
   ```

2. **Open Xcode Project**:
   ```bash
   cd /Users/michaelseliger/projects/slideflow
   open slideflow.xcodeproj
   ```

3. **Add Swift Package Dependencies**:
   - In Xcode: File → Add Packages
   - Add packages from Package.swift:
     - `https://github.com/ZipArchive/ZipArchive`
     - `https://github.com/CoreOffice/XMLCoder`
     - `https://github.com/aspose-slides-cloud/aspose-slides-cloud-swift` (optional, for export)

4. **Build and Run**:
   - Select `Slideflow` scheme
   - Press ⌘R to run

### Optional: Configure Export

For PowerPoint export functionality:

1. Copy config template:
   ```bash
   cp Config.xcconfig.template Config.xcconfig
   ```

2. Get API credentials from [Aspose Cloud Dashboard](https://dashboard.aspose.cloud)

3. Edit `Config.xcconfig` with your credentials:
   ```
   ASPOSE_CLIENT_ID = your_client_id
   ASPOSE_CLIENT_SECRET = your_client_secret
   ```

## Usage

### 1. Add Directories

1. Launch Slideflow
2. Go to **Directories** tab
3. Click **Add Directory** (or ⌘+click folder icon)
4. Select a folder containing PowerPoint files
5. Wait for indexing to complete

### 2. Search Slides

1. Go to **Search** tab
2. Enter keywords in search bar
3. Press Enter or click Search
4. Click **Filters** to refine by:
   - Presentation name
   - Directory location
   - Date modified range
5. Click any thumbnail to preview the slide

### 3. Create Workspaces

1. Go to **Workspaces** tab
2. Click **New Workspace** (⌘N)
3. Enter a name and click Create
4. From search results, preview slides and add to workspace
5. In workspace editor:
   - Drag slides to reorder
   - Swipe left to delete
   - Click **Export** (⌘E) to save as PowerPoint

## Architecture

```
Slideflow/
├── App/              # Application entry point
├── Models/           # CoreData entities
│   └── CoreData/     # Data schema
├── Services/         # Business logic
│   ├── PowerPoint/   # PPTX parsing & export
│   ├── Indexing/     # Directory monitoring & indexing
│   ├── Search/       # Search engine & filters
│   └── Storage/      # CoreData stack & caching
└── Views/            # SwiftUI views
    ├── DirectoryConfig/
    ├── Search/
    ├── Workspace/
    └── Export/
```

## Technology Stack

- **UI**: SwiftUI (macOS 15+)
- **Storage**: CoreData with SQLite
- **Parsing**: ZipArchive + XMLCoder
- **Thumbnails**: LibreOffice + PDFKit
- **Monitoring**: FSEvents (native macOS)
- **Export**: Aspose.Slides Cloud SDK (optional)
- **Testing**: XCTest + XCUITest

## Performance

- **Indexing**: ~30-60 minutes for 10,000 slides (one-time, concurrent processing)
- **Search**: <2 seconds for 10,000 slides
- **Export**: <60 seconds for 50-slide presentation
- **UI**: 60fps animations and scrolling

## Testing

Run unit and integration tests:
```bash
# In Xcode
⌘U

# Or via command line
xcodebuild test -project slideflow.xcodeproj -scheme Slideflow
```

## Documentation

- **IMPLEMENTATION_NOTES.md**: Detailed implementation summary
- **COMPLETION_SUMMARY.md**: Task completion report
- **specs/001-ppt-slide-organizer/**: Original specifications
  - spec.md: Feature requirements
  - plan.md: Technical plan
  - data-model.md: Database schema
  - research.md: Technology decisions
  - quickstart.md: Developer guide
  - tasks.md: Task breakdown

## Development

Built following TDD (Test-Driven Development):
- Tests written FIRST before implementation
- All critical paths covered
- 15 test files with unit, integration, and UI tests

Constitution compliance:
- ✅ SwiftUI-first architecture
- ✅ Result types for error handling
- ✅ No force-unwraps (SwiftLint enforced)
- ✅ Async/await for all I/O
- ✅ 60fps performance

## Keyboard Shortcuts

- `⌘F` - Focus search
- `⌘N` - New workspace
- `⌘E` - Export workspace
- `⌘W` - Close window
- `Enter` - Execute search
- `Delete` - Remove selected item

## Troubleshooting

### LibreOffice not found
```bash
# Verify installation
/Applications/LibreOffice.app/Contents/MacOS/soffice --version

# Reinstall if needed
brew reinstall --cask libreoffice
```

### CoreData errors
```bash
# Clear CoreData store (development only)
rm ~/Library/Application\ Support/Slideflow/Slideflow.sqlite*
```

### Export fails
- Verify Aspose credentials in Config.xcconfig
- Check API quota at dashboard.aspose.cloud
- Fallback implementation will copy first source presentation

## Contributing

This project follows strict code quality standards:
- TDD mandatory (tests before code)
- SwiftLint for code style
- No force-unwraps allowed
- All errors must be typed and handled

## License

See project license file.

## Support

- Feature Spec: `specs/001-ppt-slide-organizer/spec.md`
- Implementation Notes: `IMPLEMENTATION_NOTES.md`
- Completion Summary: `COMPLETION_SUMMARY.md`

---

**Built with Swift 6.0 • SwiftUI • CoreData • macOS 15+**
