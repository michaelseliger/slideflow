//
//  ThumbnailGeneratorTests.swift
//  SlideflowTests
//
//  Tests for LibreOffice→PDF→NSImage thumbnail generation pipeline
//

import XCTest
import AppKit
@testable import Slideflow

final class ThumbnailGeneratorTests: XCTestCase {
    var generator: ThumbnailGenerator?

    override func setUp() async throws {
        generator = ThumbnailGenerator()
    }

    func testConvertToPDF_ValidPPTX_GeneratesPDF() async throws {
        // GIVEN: A valid PPTX file
        let testPath = Bundle(for: type(of: self)).path(forResource: "TestPresentation_5Slides", ofType: "pptx")
        guard let path = testPath else {
            throw XCTSkip("Test file not found")
        }

        // WHEN: Converting to PDF
        let result = await generator?.convertToPDF(presentationAt: path)

        // THEN: Should generate PDF file
        switch result {
        case .success(let pdfPath):
            XCTAssertTrue(FileManager.default.fileExists(atPath: pdfPath))
            XCTAssertTrue(pdfPath.hasSuffix(".pdf"))
        case .failure(let error):
            XCTFail("Failed to convert to PDF: \(error)")
        case .none:
            XCTFail("Generator is nil")
        }
    }

    func testGenerateThumbnail_ValidPDF_GeneratesImage() async throws {
        // GIVEN: A PDF file (from previous conversion)
        let testPath = Bundle(for: type(of: self)).path(forResource: "TestPresentation_5Slides", ofType: "pptx")
        guard let path = testPath else {
            throw XCTSkip("Test file not found")
        }

        // Convert to PDF first
        let pdfResult = await generator?.convertToPDF(presentationAt: path)
        guard case .success(let pdfPath) = pdfResult else {
            XCTFail("PDF conversion failed")
            return
        }

        // WHEN: Generating thumbnail from PDF page
        let result = await generator?.generateThumbnail(from: pdfPath, pageNumber: 1)

        // THEN: Should generate NSImage
        switch result {
        case .success(let image):
            XCTAssertNotNil(image)
            XCTAssertGreaterThan(image.size.width, 0)
            XCTAssertGreaterThan(image.size.height, 0)
        case .failure(let error):
            XCTFail("Failed to generate thumbnail: \(error)")
        case .none:
            XCTFail("Generator is nil")
        }
    }

    func testSaveThumbnail_ValidImage_SavesToDisk() async throws {
        // GIVEN: A generated thumbnail image
        let testImage = NSImage(size: NSSize(width: 200, height: 300))

        // WHEN: Saving to cache directory
        let cacheURL = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]
        let thumbnailPath = cacheURL.appendingPathComponent("test_thumbnail.png").path

        let result = generator?.saveThumbnail(testImage, to: thumbnailPath)

        // THEN: Should save file
        switch result {
        case .success:
            XCTAssertTrue(FileManager.default.fileExists(atPath: thumbnailPath))
            // Cleanup
            try? FileManager.default.removeItem(atPath: thumbnailPath)
        case .failure(let error):
            XCTFail("Failed to save thumbnail: \(error)")
        case .none:
            XCTFail("Generator is nil")
        }
    }
}
