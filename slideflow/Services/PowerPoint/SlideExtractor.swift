//
//  SlideExtractor.swift
//  Slideflow
//
//  Extracts individual slides from PowerPoint presentations
//

import Foundation

enum SlideExtractorError: Error {
    case invalidSlideNumber
    case presentationNotFound
    case extractionFailed(underlying: Error)
}

class SlideExtractor {
    private let parser = PowerPointParser()

    init() {}

    /// Extract a specific slide from presentation
    func extractSlide(from presentationPath: String, slideNumber: Int) async -> Result<SlideData, SlideExtractorError> {
        // Validate presentation exists
        guard FileManager.default.fileExists(atPath: presentationPath) else {
            return .failure(.presentationNotFound)
        }

        // Parse presentation to get metadata
        let parseResult = await parser.parse(presentationAt: presentationPath)
        guard case .success(let metadata) = parseResult else {
            if case .failure(let error) = parseResult {
                return .failure(.extractionFailed(underlying: error))
            }
            return .failure(.extractionFailed(underlying: NSError(domain: "Unknown", code: -1)))
        }

        // Validate slide number
        guard slideNumber >= 1 && slideNumber <= metadata.slideCount else {
            return .failure(.invalidSlideNumber)
        }

        // Parse the slide
        let slideResult = await parser.parseSlide(at: presentationPath, slideNumber: slideNumber)

        switch slideResult {
        case .success(let slideData):
            return .success(slideData)
        case .failure(let error):
            return .failure(.extractionFailed(underlying: error))
        }
    }

    /// Extract all slides from presentation
    func extractAllSlides(from presentationPath: String) async -> Result<[SlideData], SlideExtractorError> {
        // Get presentation metadata
        let parseResult = await parser.parse(presentationAt: presentationPath)
        guard case .success(let metadata) = parseResult else {
            if case .failure(let error) = parseResult {
                return .failure(.extractionFailed(underlying: error))
            }
            return .failure(.extractionFailed(underlying: NSError(domain: "Unknown", code: -1)))
        }

        var slides: [SlideData] = []

        // Extract each slide
        for slideNumber in 1...metadata.slideCount {
            let result = await parser.parseSlide(at: presentationPath, slideNumber: slideNumber)
            switch result {
            case .success(let slideData):
                slides.append(slideData)
            case .failure:
                // Skip failed slides but continue
                continue
            }
        }

        return .success(slides)
    }
}
