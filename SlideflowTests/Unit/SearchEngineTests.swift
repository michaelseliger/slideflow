//
//  SearchEngineTests.swift
//  SlideflowTests
//
//  Tests for CoreData predicate text search and performance <2s
//

import XCTest
import CoreData
@testable import Slideflow

final class SearchEngineTests: XCTestCase {
    var searchEngine: SearchEngine?
    var testContext: NSManagedObjectContext?

    override func setUp() async throws {
        searchEngine = SearchEngine()
        testContext = CoreDataStack.shared.newBackgroundContext()

        // Populate with test data
        try await populateTestData()
    }

    override func tearDown() async throws {
        searchEngine = nil
        testContext = nil
    }

    private func populateTestData() async throws {
        guard let context = testContext else { return }

        for i in 1...100 {
            let slide = IndexedSlide(context: context)
            slide.slideId = UUID()
            slide.slideNumber = Int16(i)
            slide.textContent = "Test slide \(i) with searchable content"
            slide.thumbnailPath = "/tmp/thumb\(i).png"
            slide.extractedDate = Date()
            slide.hasImages = false
            slide.hasMedia = false
            slide.searchRank = 0.0
        }

        _ = CoreDataStack.shared.save(context: context)
    }

    func testSearch_ValidQuery_ReturnsMatchingSlides() async throws {
        // GIVEN: A search query "searchable"
        let query = "searchable"

        // WHEN: Performing search
        let result = await searchEngine?.search(query: query, in: testContext!)

        // THEN: Should return matching slides
        switch result {
        case .success(let slides):
            XCTAssertGreaterThan(slides.count, 0)
            XCTAssertTrue(slides.allSatisfy { $0.textContent.contains("searchable") })
        case .failure(let error):
            XCTFail("Search failed: \(error)")
        case .none:
            XCTFail("SearchEngine is nil")
        }
    }

    func testSearch_CaseInsensitive_ReturnsMatches() async throws {
        // GIVEN: Mixed case query
        let query = "SEARCHABLE"

        // WHEN: Searching
        let result = await searchEngine?.search(query: query, in: testContext!)

        // THEN: Should match case-insensitively
        switch result {
        case .success(let slides):
            XCTAssertGreaterThan(slides.count, 0)
        case .failure(let error):
            XCTFail("Search failed: \(error)")
        case .none:
            XCTFail("SearchEngine is nil")
        }
    }

    func testSearch_Performance_UnderTwoSeconds() async throws {
        // GIVEN: Large dataset (100 slides)
        let query = "content"

        // WHEN: Measuring search performance
        let startTime = Date()
        _ = await searchEngine?.search(query: query, in: testContext!)
        let duration = Date().timeIntervalSince(startTime)

        // THEN: Should complete in <2s
        XCTAssertLessThan(duration, 2.0, "Search should complete in under 2 seconds")
    }

    func testSearch_EmptyQuery_ReturnsAllSlides() async throws {
        // GIVEN: Empty query
        let query = ""

        // WHEN: Searching
        let result = await searchEngine?.search(query: query, in: testContext!)

        // THEN: Should return all slides
        switch result {
        case .success(let slides):
            XCTAssertEqual(slides.count, 100)
        case .failure(let error):
            XCTFail("Search failed: \(error)")
        case .none:
            XCTFail("SearchEngine is nil")
        }
    }

    func testSearch_NoMatches_ReturnsEmpty() async throws {
        // GIVEN: Query with no matches
        let query = "nonexistent_xyz_123"

        // WHEN: Searching
        let result = await searchEngine?.search(query: query, in: testContext!)

        // THEN: Should return empty array
        switch result {
        case .success(let slides):
            XCTAssertEqual(slides.count, 0)
        case .failure(let error):
            XCTFail("Search failed: \(error)")
        case .none:
            XCTFail("SearchEngine is nil")
        }
    }
}
