//
//  ThumbnailGenerator.swift
//  Slideflow
//
//  Generates slide thumbnails using macOS Quick Look (native)
//

import Foundation
import AppKit
import QuickLookThumbnailing

enum ThumbnailGeneratorError: Error {
    case fileNotFound
    case thumbnailGenerationFailed
    case saveFailed(underlying: Error)
}

class ThumbnailGenerator {
    private let fileManager = FileManager.default

    init() {}

    /// Generate thumbnail for entire PPTX file using Quick Look
    func generateThumbnailForSlide(presentationPath: String, slideNumber: Int) async -> Result<String, ThumbnailGeneratorError> {
        let fileURL = URL(fileURLWithPath: presentationPath)

        guard fileManager.fileExists(atPath: presentationPath) else {
            return .failure(.fileNotFound)
        }

        // Use Quick Look to generate thumbnail
        let size = CGSize(width: 400, height: 300)
        let scale = NSScreen.main?.backingScaleFactor ?? 2.0
        let request = QLThumbnailGenerator.Request(
            fileAt: fileURL,
            size: size,
            scale: scale,
            representationTypes: .thumbnail
        )

        return await withCheckedContinuation { continuation in
            QLThumbnailGenerator.shared.generateRepresentations(for: request) { thumbnail, type, error in
                if let error = error {
                    print("❌ Thumbnail generation failed: \(error.localizedDescription)")
                    continuation.resume(returning: .failure(.thumbnailGenerationFailed))
                    return
                }

                guard let thumbnail = thumbnail else {
                    continuation.resume(returning: .failure(.thumbnailGenerationFailed))
                    return
                }

                // Save thumbnail to cache
                let cacheDir = self.fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
                    .appendingPathComponent("Slideflow/Thumbnails")

                let slideId = UUID().uuidString
                let thumbnailPath = cacheDir.appendingPathComponent("\(slideId).png")

                do {
                    try self.fileManager.createDirectory(at: cacheDir, withIntermediateDirectories: true)

                    // Convert CGImage to PNG
                    let image = thumbnail.nsImage
                    guard let tiffData = image.tiffRepresentation,
                          let bitmapImage = NSBitmapImageRep(data: tiffData),
                          let pngData = bitmapImage.representation(using: .png, properties: [:]) else {
                        continuation.resume(returning: .failure(.thumbnailGenerationFailed))
                        return
                    }

                    try pngData.write(to: thumbnailPath)
                    print("✅ Thumbnail saved: \(thumbnailPath.path)")
                    continuation.resume(returning: .success(thumbnailPath.path))
                } catch {
                    print("❌ Failed to save thumbnail: \(error.localizedDescription)")
                    continuation.resume(returning: .failure(.saveFailed(underlying: error)))
                }
            }
        }
    }
}
