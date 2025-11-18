//
//  SlideIndexer.swift
//  Slideflow
//
//  Persists slides to CoreData with background context
//

import Foundation
import CoreData

class SlideIndexer {
    private let parser = PowerPointParser()
    private let extractor = SlideExtractor()
    private let thumbnailGenerator = ThumbnailGenerator()

    init() {}

    /// Create SourcePresentation entity for a PPTX file
    private func createSourcePresentation(at path: String, in context: NSManagedObjectContext) async -> Result<SourcePresentation, CoreDataError> {
        // Parse presentation to get metadata
        let parseResult = await parser.parse(presentationAt: path)
        guard case .success(let metadata) = parseResult else {
            return .failure(.validationFailed(message: "Failed to parse presentation"))
        }

        return await withCheckedContinuation { continuation in
            context.perform {
                // Check for existing presentation
                let fetchRequest = NSFetchRequest<SourcePresentation>(entityName: "SourcePresentation")
                fetchRequest.predicate = NSPredicate(format: "filePath == %@", path)

                do {
                    let existing = try context.fetch(fetchRequest)
                    if let presentation = existing.first {
                        continuation.resume(returning: .success(presentation))
                        return
                    }

                    // Create new SourcePresentation
                    let presentation = SourcePresentation(context: context)
                    presentation.presentationId = UUID()
                    presentation.filename = metadata.filename
                    presentation.filePath = path
                    presentation.fileSize = 0 // TODO: Calculate actual size
                    presentation.fileModifiedDate = Date()
                    presentation.fileCreatedDate = Date()
                    presentation.totalSlideCount = Int16(metadata.slideCount)
                    presentation.indexedDate = Date()
                    presentation.indexingStatus = "completed"

                    // Create security-scoped bookmark for sandbox access
                    let fileURL = URL(fileURLWithPath: path)
                    do {
                        let bookmarkData = try fileURL.bookmarkData(
                            options: .withSecurityScope,
                            includingResourceValuesForKeys: nil,
                            relativeTo: nil
                        )
                        presentation.securityBookmark = bookmarkData
                        print("✅ Created security bookmark for \(metadata.filename)")
                    } catch {
                        print("⚠️ Failed to create bookmark for \(path): \(error)")
                    }

                    let saveResult = CoreDataStack.shared.save(context: context)
                    switch saveResult {
                    case .success:
                        continuation.resume(returning: .success(presentation))
                    case .failure(let error):
                        continuation.resume(returning: .failure(error))
                    }
                } catch {
                    continuation.resume(returning: .failure(.fetchFailed(underlying: error)))
                }
            }
        }
    }

    /// Index a single slide to CoreData
    func indexSlide(_ slideData: SlideData, sourcePresentation: SourcePresentation, in context: NSManagedObjectContext) async -> Result<IndexedSlide, CoreDataError> {
        return await withCheckedContinuation { continuation in
            context.perform {
                // Check for duplicates
                let fetchRequest: NSFetchRequest<IndexedSlide> = IndexedSlide.fetchRequest()
                fetchRequest.predicate = NSPredicate(
                    format: "slideNumber == %d AND sourcePresentation == %@",
                    slideData.slideNumber,
                    sourcePresentation
                )

                do {
                    let existing = try context.fetch(fetchRequest)
                    if !existing.isEmpty {
                        continuation.resume(returning: .failure(.duplicateEntry))
                        return
                    }

                    // Create new IndexedSlide
                    let slide = IndexedSlide(context: context)
                    slide.slideId = UUID()
                    slide.slideNumber = slideData.slideNumber
                    slide.textContent = slideData.textContent
                    slide.thumbnailPath = slideData.thumbnailPath
                    slide.hasImages = slideData.hasImages
                    slide.hasMedia = slideData.hasMedia
                    slide.slideLayout = slideData.slideLayout
                    slide.extractedDate = Date()
                    slide.searchRank = 0.0
                    slide.thumbnailWidth = 200
                    slide.thumbnailHeight = 300
                    slide.sourcePresentation = sourcePresentation

                    // Save context
                    let saveResult = CoreDataStack.shared.save(context: context)
                    switch saveResult {
                    case .success:
                        continuation.resume(returning: .success(slide))
                    case .failure(let error):
                        continuation.resume(returning: .failure(error))
                    }
                } catch {
                    continuation.resume(returning: .failure(.fetchFailed(underlying: error)))
                }
            }
        }
    }

    /// Index entire presentation
    func indexPresentation(at path: String, in context: NSManagedObjectContext) async -> Result<Int, CoreDataError> {
        // Create SourcePresentation entity first
        let presentation = await createSourcePresentation(at: path, in: context)
        guard case .success(let sourcePresentation) = presentation else {
            if case .failure(let error) = presentation {
                return .failure(error)
            }
            return .failure(.validationFailed(message: "Failed to create presentation record"))
        }

        // Extract all slides
        let extractResult = await extractor.extractAllSlides(from: path)

        guard case .success(var slides) = extractResult else {
            return .failure(.validationFailed(message: "Failed to extract slides"))
        }

        // Generate thumbnails concurrently (limited to 5 at a time)
        await withTaskGroup(of: (Int, String?).self) { group in
            for (index, _) in slides.enumerated() {
                group.addTask {
                    let thumbnailResult = await self.thumbnailGenerator.generateThumbnailForSlide(
                        presentationPath: path,
                        slideNumber: index + 1
                    )
                    switch thumbnailResult {
                    case .success(let thumbnailPath):
                        return (index, thumbnailPath)
                    case .failure:
                        return (index, nil)
                    }
                }
            }

            for await (index, thumbnailPath) in group {
                if let path = thumbnailPath {
                    slides[index] = SlideData(
                        slideNumber: slides[index].slideNumber,
                        textContent: slides[index].textContent,
                        thumbnailPath: path,
                        hasImages: slides[index].hasImages,
                        hasMedia: slides[index].hasMedia,
                        slideLayout: slides[index].slideLayout
                    )
                }
            }
        }

        // Index all slides to CoreData
        var successCount = 0
        for slideData in slides {
            let result = await indexSlide(slideData, sourcePresentation: sourcePresentation, in: context)
            if case .success = result {
                successCount += 1
            }
        }

        return .success(successCount)
    }
}

/// Helper class for directory scanning
class DirectoryScanner {
    func scanDirectory(at path: String) async -> Result<[String], Error> {
        let fileManager = FileManager.default

        do {
            let contents = try fileManager.contentsOfDirectory(atPath: path)
            let pptxFiles = contents.filter { $0.hasSuffix(".pptx") || $0.hasSuffix(".ppt") }
            let fullPaths = pptxFiles.map { URL(fileURLWithPath: path).appendingPathComponent($0).path }
            return .success(fullPaths)
        } catch {
            return .failure(error)
        }
    }

    func scanForNewPresentations(in path: String, context: NSManagedObjectContext) async -> Result<Int, CoreDataError> {
        // Scan directory
        let scanResult = await scanDirectory(at: path)
        guard case .success(let files) = scanResult else {
            return .failure(.fetchFailed(underlying: NSError(domain: "DirectoryScanner", code: -1)))
        }

        // Fetch already indexed presentations
        let fetchRequest = NSFetchRequest<SourcePresentation>(entityName: "SourcePresentation")
        fetchRequest.predicate = NSPredicate(format: "directory.path == %@", path)

        do {
            let existing = try context.fetch(fetchRequest)
            let existingPaths = Set(existing.map { $0.filePath ?? "" })

            // Find new files
            let newFiles = files.filter { !existingPaths.contains($0) }
            return .success(newFiles.count)
        } catch {
            return .failure(.fetchFailed(underlying: error))
        }
    }
}
