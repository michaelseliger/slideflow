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

    /// Generate thumbnail for a specific slide by extracting it to a temporary PPTX
    func generateThumbnailForSlide(presentationPath: String, slideNumber: Int) async -> Result<String, ThumbnailGeneratorError> {
        let fileURL = URL(fileURLWithPath: presentationPath)

        guard fileManager.fileExists(atPath: presentationPath) else {
            return .failure(.fileNotFound)
        }

        // Create temp directory for single-slide extraction
        let tempDir = fileManager.temporaryDirectory.appendingPathComponent("SlideflowThumbnails/\(UUID().uuidString)")
        let singleSlideURL = tempDir.appendingPathComponent("slide\(slideNumber).pptx")

        do {
            try fileManager.createDirectory(at: tempDir, withIntermediateDirectories: true)

            // Extract just this slide to a new PPTX file
            let extractResult = await extractSingleSlide(from: presentationPath, slideNumber: slideNumber, to: singleSlideURL.path)

            guard case .success = extractResult else {
                // Fallback: generate thumbnail from entire presentation (will show first slide)
                print("⚠️ Could not extract slide \(slideNumber), using full presentation thumbnail")
                return await generateThumbnailFromFile(fileURL: fileURL, slideNumber: slideNumber)
            }

            // Generate thumbnail from the single-slide PPTX
            let result = await generateThumbnailFromFile(fileURL: singleSlideURL, slideNumber: slideNumber)

            // Cleanup temp file
            try? fileManager.removeItem(at: tempDir)

            return result
        } catch {
            return .failure(.saveFailed(underlying: error))
        }
    }

    /// Extract a single slide to a new PPTX file using PPTXMerger
    private func extractSingleSlide(from sourcePath: String, slideNumber: Int, to outputPath: String) async -> Result<Void, ThumbnailGeneratorError> {
        let merger = PPTXMerger()

        // Use PPTXMerger to create a new PPTX with just this slide
        let slideSpecs: [(sourcePath: String, slideNumbers: [Int])] = [(sourcePath: sourcePath, slideNumbers: [slideNumber])]
        let result = await merger.mergeSlides(specs: slideSpecs, to: outputPath)

        switch result {
        case .success:
            return .success(())
        case .failure:
            return .failure(.thumbnailGenerationFailed)
        }
    }

    /// Generate thumbnail from a PPTX file using QuickLook
    private func generateThumbnailFromFile(fileURL: URL, slideNumber: Int) async -> Result<String, ThumbnailGeneratorError> {
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
                    print("✅ Thumbnail for slide \(slideNumber) saved: \(thumbnailPath.path)")
                    continuation.resume(returning: .success(thumbnailPath.path))
                } catch {
                    print("❌ Failed to save thumbnail: \(error.localizedDescription)")
                    continuation.resume(returning: .failure(.saveFailed(underlying: error)))
                }
            }
        }
    }
}
