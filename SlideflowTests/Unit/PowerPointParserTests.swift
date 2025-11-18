//
//  PowerPointParserTests.swift
//  SlideflowTests
//
//  Tests for PowerPoint PPTX parsing using ZipArchive + XMLCoder
//

import XCTest
@testable import Slideflow

final class PowerPointParserTests: XCTestCase {
    var parser: PowerPointParser?
    var testPptxPath: String?

    override func setUp() async throws {
        parser = PowerPointParser()
        // Test file should be added to test bundle
        testPptxPath = Bundle(for: type(of: self)).path(forResource: "TestPresentation_5Slides", ofType: "pptx")
    }

    override func tearDown() async throws {
        parser = nil
        testPptxPath = nil
    }

    func testExtractZipArchive_ValidPPTX_ExtractsContents() async throws {
        // GIVEN: A valid PPTX file
        guard let path = testPptxPath else {
            XCTFail("Test PPTX file not found in bundle")
            return
        }

        // WHEN: Extracting the ZIP archive
        let result = await parser?.extractArchive(at: path)

        // THEN: Should successfully extract contents
        switch result {
        case .success(let extractedPath):
            XCTAssertTrue(FileManager.default.fileExists(atPath: extractedPath))
        case .failure(let error):
            XCTFail("Failed to extract archive: \(error)")
        case .none:
            XCTFail("Parser is nil")
        }
    }

    func testParsePresentation_ValidPPTX_DetectsSlideCount() async throws {
        // GIVEN: A presentation with 5 slides
        guard let path = testPptxPath else {
            XCTFail("Test PPTX file not found")
            return
        }

        // WHEN: Parsing the presentation
        let result = await parser?.parse(presentationAt: path)

        // THEN: Should detect 5 slides
        switch result {
        case .success(let metadata):
            XCTAssertEqual(metadata.slideCount, 5, "Should detect 5 slides")
        case .failure(let error):
            XCTFail("Failed to parse presentation: \(error)")
        case .none:
            XCTFail("Parser is nil")
        }
    }

    func testParseSlideXML_ValidSlide_ExtractsMetadata() async throws {
        // GIVEN: A slide XML file
        guard let path = testPptxPath else {
            XCTFail("Test file not found")
            return
        }

        // WHEN: Parsing slide 1
        let result = await parser?.parseSlide(at: path, slideNumber: 1)

        // THEN: Should extract slide metadata
        switch result {
        case .success(let slideData):
            XCTAssertEqual(slideData.slideNumber, 1)
            XCTAssertFalse(slideData.textContent.isEmpty, "Should have text content")
        case .failure(let error):
            XCTFail("Failed to parse slide: \(error)")
        case .none:
            XCTFail("Parser is nil")
        }
    }

    func testParsePresentation_CorruptedFile_ReturnsError() async throws {
        // GIVEN: A corrupted PPTX file
        let corruptedPath = Bundle(for: type(of: self)).path(forResource: "TestPresentation_Corrupted", ofType: "pptx")
        guard let path = corruptedPath else {
            // If no corrupted test file, skip this test
            throw XCTSkip("Corrupted test file not available")
        }

        // WHEN: Parsing the corrupted file
        let result = await parser?.parse(presentationAt: path)

        // THEN: Should return error
        switch result {
        case .success:
            XCTFail("Should not succeed with corrupted file")
        case .failure:
            // Expected behavior
            break
        case .none:
            XCTFail("Parser is nil")
        }
    }
}
