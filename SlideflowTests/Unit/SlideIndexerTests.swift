//
//  SlideIndexerTests.swift
//  SlideflowTests
//
//  Tests for CoreData slide indexing operations
//

import XCTest
import CoreData
@testable import Slideflow

final class SlideIndexerTests: XCTestCase {
    var indexer: SlideIndexer?
    var testContext: NSManagedObjectContext?

    override func setUp() async throws {
        indexer = SlideIndexer()
        // Use in-memory store for testing
        testContext = CoreDataStack.shared.newBackgroundContext()
    }

    override func tearDown() async throws {
        indexer = nil
        testContext = nil
    }

    func testIndexSlide_ValidSlideData_SavesToCoreData() async throws {
        // GIVEN: Valid slide data
        guard let context = testContext else {
            XCTFail("Test context is nil")
            return
        }

        let slideData = SlideData(
            slideNumber: 1,
            textContent: "Test Slide Content",
            thumbnailPath: "/tmp/test_thumb.png",
            hasImages: false,
            hasMedia: false
        )

        // WHEN: Indexing the slide
        let result = await indexer?.indexSlide(slideData, in: context)

        // THEN: Should save to CoreData
        switch result {
        case .success(let indexedSlide):
            XCTAssertEqual(indexedSlide.slideNumber, 1)
            XCTAssertEqual(indexedSlide.textContent, "Test Slide Content")
        case .failure(let error):
            XCTFail("Failed to index slide: \(error)")
        case .none:
            XCTFail("Indexer is nil")
        }
    }

    func testIndexSlide_DuplicateSlide_ReturnsError() async throws {
        // GIVEN: A slide already indexed
        guard let context = testContext else {
            XCTFail("Test context is nil")
            return
        }

        let slideData = SlideData(
            slideNumber: 1,
            textContent: "Duplicate Test",
            thumbnailPath: "/tmp/dup.png",
            hasImages: false,
            hasMedia: false
        )

        // Index once
        _ = await indexer?.indexSlide(slideData, in: context)

        // WHEN: Indexing the same slide again
        let result = await indexer?.indexSlide(slideData, in: context)

        // THEN: Should return duplicate error
        switch result {
        case .success:
            XCTFail("Should not allow duplicate slide")
        case .failure(let error):
            if case CoreDataError.duplicateEntry = error {
                // Expected
            } else {
                XCTFail("Wrong error type: \(error)")
            }
        case .none:
            XCTFail("Indexer is nil")
        }
    }

    func testIndexPresentation_ValidPPTX_IndexesAllSlides() async throws {
        // GIVEN: A presentation with 5 slides
        let testPath = Bundle(for: type(of: self)).path(forResource: "TestPresentation_5Slides", ofType: "pptx")
        guard let path = testPath, let context = testContext else {
            throw XCTSkip("Test file or context not available")
        }

        // WHEN: Indexing entire presentation
        let result = await indexer?.indexPresentation(at: path, in: context)

        // THEN: Should index 5 slides
        switch result {
        case .success(let count):
            XCTAssertEqual(count, 5)
        case .failure(let error):
            XCTFail("Failed to index presentation: \(error)")
        case .none:
            XCTFail("Indexer is nil")
        }
    }
}
