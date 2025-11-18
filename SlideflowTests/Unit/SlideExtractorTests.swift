//
//  SlideExtractorTests.swift
//  SlideflowTests
//
//  Tests for extracting individual slides from presentations
//

import XCTest
@testable import Slideflow

final class SlideExtractorTests: XCTestCase {
    var extractor: SlideExtractor?

    override func setUp() async throws {
        extractor = SlideExtractor()
    }

    func testExtractSlide_ValidSlideNumber_ExtractsCorrectSlide() async throws {
        // GIVEN: A presentation with 5 slides
        let testPath = Bundle(for: type(of: self)).path(forResource: "TestPresentation_5Slides", ofType: "pptx")
        guard let path = testPath else {
            throw XCTSkip("Test file not found")
        }

        // WHEN: Extracting slide 3
        let result = await extractor?.extractSlide(from: path, slideNumber: 3)

        // THEN: Should extract slide 3
        switch result {
        case .success(let slide):
            XCTAssertEqual(slide.slideNumber, 3)
        case .failure(let error):
            XCTFail("Failed to extract slide: \(error)")
        case .none:
            XCTFail("Extractor is nil")
        }
    }

    func testExtractSlide_InvalidSlideNumber_ReturnsError() async throws {
        // GIVEN: A presentation with 5 slides
        let testPath = Bundle(for: type(of: self)).path(forResource: "TestPresentation_5Slides", ofType: "pptx")
        guard let path = testPath else {
            throw XCTSkip("Test file not found")
        }

        // WHEN: Requesting slide 10 (doesn't exist)
        let result = await extractor?.extractSlide(from: path, slideNumber: 10)

        // THEN: Should return error
        switch result {
        case .success:
            XCTFail("Should not succeed with invalid slide number")
        case .failure:
            // Expected
            break
        case .none:
            XCTFail("Extractor is nil")
        }
    }

    func testExtractAllSlides_ValidPresentation_ExtractsAllSlides() async throws {
        // GIVEN: A presentation with 5 slides
        let testPath = Bundle(for: type(of: self)).path(forResource: "TestPresentation_5Slides", ofType: "pptx")
        guard let path = testPath else {
            throw XCTSkip("Test file not found")
        }

        // WHEN: Extracting all slides
        let result = await extractor?.extractAllSlides(from: path)

        // THEN: Should extract 5 slides
        switch result {
        case .success(let slides):
            XCTAssertEqual(slides.count, 5)
        case .failure(let error):
            XCTFail("Failed to extract slides: \(error)")
        case .none:
            XCTFail("Extractor is nil")
        }
    }
}
