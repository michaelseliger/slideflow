//
//  PowerPointExporter.swift
//  Slideflow
//
//  Export workspace to .pptx using Aspose.Slides Cloud SDK
//

import Foundation
import CoreData
// import AsposeSlidesCloud // Uncomment when package is properly linked

enum ExportError: Error {
    case emptyWorkspace
    case workspaceNotFound
    case slideExtractionFailed
    case apiError(underlying: Error)
    case credentialsNotFound
    case fileGenerationFailed
}

class PowerPointExporter {
    private let fileManager = FileManager.default
    private let merger = PPTXMerger()

    init() {}

    /// Export workspace to PowerPoint file
    func exportWorkspace(workspaceId: UUID, outputPath: String, progressCallback: ((Double) -> Void)? = nil) async -> Result<String, ExportError> {
        // Fetch workspace
        let context = CoreDataStack.shared.newBackgroundContext()

        return await withCheckedContinuation { continuation in
            context.perform {
                let fetchRequest: NSFetchRequest<Workspace> = Workspace.fetchRequest()
                fetchRequest.predicate = NSPredicate(format: "workspaceId == %@", workspaceId as CVarArg)

                do {
                    let workspaces = try context.fetch(fetchRequest)
                    guard let workspace = workspaces.first else {
                        continuation.resume(returning: .failure(.workspaceNotFound))
                        return
                    }

                    // Check not empty
                    guard workspace.slideCount > 0 else {
                        continuation.resume(returning: .failure(.emptyWorkspace))
                        return
                    }

                    // Fetch workspace slides in order
                    let slidesFetchRequest: NSFetchRequest<WorkspaceSlide> = WorkspaceSlide.fetchRequest()
                    slidesFetchRequest.predicate = NSPredicate(format: "workspace == %@", workspace)
                    slidesFetchRequest.sortDescriptors = [NSSortDescriptor(key: "orderIndex", ascending: true)]

                    let workspaceSlides = try context.fetch(slidesFetchRequest)

                    print("📊 Workspace has \(workspaceSlides.count) slides")

                    // Build slide specs for Python merge script
                    // Format: "path.pptx:1,2,3 path2.pptx:4,5"
                    var slideSpecs: [String] = []
                    var currentPresentation: SourcePresentation?
                    var currentSlides: [Int] = []

                    for wsSlide in workspaceSlides {
                        guard let indexedSlide = wsSlide.slide,
                              let presentation = indexedSlide.sourcePresentation else {
                            continue
                        }

                        // Group consecutive slides from same presentation
                        if presentation == currentPresentation {
                            currentSlides.append(Int(indexedSlide.slideNumber))
                        } else {
                            // Save previous group
                            if let prev = currentPresentation, !currentSlides.isEmpty {
                                if let resolvedPath = self.resolveSecurityBookmark(for: prev) {
                                    let slideNums = currentSlides.map(String.init).joined(separator: ",")
                                    slideSpecs.append("\(resolvedPath):\(slideNums)")
                                } else {
                                    print("⚠️ Cannot access file: \(prev.filename)")
                                }
                            }

                            // Start new group
                            currentPresentation = presentation
                            currentSlides = [Int(indexedSlide.slideNumber)]
                        }
                    }

                    // Add last group
                    if let prev = currentPresentation, !currentSlides.isEmpty {
                        if let resolvedPath = self.resolveSecurityBookmark(for: prev) {
                            let slideNums = currentSlides.map(String.init).joined(separator: ",")
                            slideSpecs.append("\(resolvedPath):\(slideNums)")
                        } else {
                            print("⚠️ Cannot access file: \(prev.filename)")
                        }
                    }

                    guard !slideSpecs.isEmpty else {
                        continuation.resume(returning: .failure(.slideExtractionFailed))
                        return
                    }

                    // Call native Swift PPTX merger
                    Task {
                        let result = await self.mergeSlidesNatively(
                            outputPath: outputPath,
                            slideSpecs: slideSpecs,
                            progressCallback: progressCallback
                        )
                        continuation.resume(returning: result)
                    }

                } catch {
                    continuation.resume(returning: .failure(.apiError(underlying: error)))
                }
            }
        }
    }

    /// Estimate export time based on slide count
    func estimateExportDuration(slideCount: Int) -> TimeInterval {
        // Rough estimate: ~1s per slide for API operations
        return TimeInterval(slideCount)
    }

    /// Validate export prerequisites
    func validateExportPrerequisites() -> Result<Void, ExportError> {
        return .success(())
    }

    /// Resolve security-scoped bookmark to access file
    private func resolveSecurityBookmark(for presentation: SourcePresentation) -> String? {
        // Try directory bookmark first (preferred)
        if let directory = presentation.directory,
           let dirBookmark = directory.securityBookmark {
            do {
                var isStale = false
                let dirURL = try URL(
                    resolvingBookmarkData: dirBookmark,
                    options: .withSecurityScope,
                    relativeTo: nil,
                    bookmarkDataIsStale: &isStale
                )

                // Start accessing directory
                guard dirURL.startAccessingSecurityScopedResource() else {
                    print("❌ Failed to access directory for \(presentation.filename)")
                    return nil
                }

                print("✅ Resolved directory bookmark for \(presentation.filename)")
                return presentation.filePath

            } catch {
                print("⚠️ Failed to resolve directory bookmark: \(error)")
            }
        }

        // Fallback to individual file bookmark
        guard let bookmarkData = presentation.securityBookmark else {
            print("⚠️ No security bookmark for \(presentation.filename)")
            return presentation.filePath
        }

        do {
            var isStale = false
            let url = try URL(
                resolvingBookmarkData: bookmarkData,
                options: .withSecurityScope,
                relativeTo: nil,
                bookmarkDataIsStale: &isStale
            )

            guard url.startAccessingSecurityScopedResource() else {
                print("❌ Failed to start accessing \(presentation.filename)")
                return nil
            }

            return url.path

        } catch {
            print("❌ Failed to resolve bookmark for \(presentation.filename): \(error)")
            return nil
        }
    }

    /// Merge slides using native Swift PPTXMerger
    private func mergeSlidesNatively(
        outputPath: String,
        slideSpecs: [String],
        progressCallback: ((Double) -> Void)?
    ) async -> Result<String, ExportError> {
        // Parse slide specs into (path, slideNumbers) tuples
        // Format: "file.pptx:1,2,3"
        var mergeSpecs: [(sourcePath: String, slideNumbers: [Int])] = []

        for spec in slideSpecs {
            let parts = spec.split(separator: ":", maxSplits: 1)
            guard parts.count == 2 else {
                print("⚠️ Invalid slide spec: \(spec)")
                continue
            }

            let path = String(parts[0])
            let slideNumsString = String(parts[1])
            let slideNumbers = slideNumsString
                .split(separator: ",")
                .compactMap { Int($0.trimmingCharacters(in: .whitespaces)) }

            guard !slideNumbers.isEmpty else {
                continue
            }

            mergeSpecs.append((sourcePath: path, slideNumbers: slideNumbers))
        }

        guard !mergeSpecs.isEmpty else {
            return .failure(.slideExtractionFailed)
        }

        // Report initial progress
        progressCallback?(0.1)

        // Use native Swift merger
        let result = await merger.mergeSlides(specs: mergeSpecs, to: outputPath)

        switch result {
        case .success:
            progressCallback?(1.0)
            print("✅ Successfully merged slides to \(outputPath)")
            return .success(outputPath)

        case .failure(let error):
            print("❌ Merge failed: \(error)")
            return .failure(.fileGenerationFailed)
        }
    }
}
