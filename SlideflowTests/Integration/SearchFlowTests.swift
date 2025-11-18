//
//  SearchFlowTests.swift
//  SlideflowTests
//
//  Integration tests: Enter query → Fetch results → Display thumbnails → Apply filters
//

import XCTest
import CoreData
@testable import Slideflow

final class SearchFlowTests: XCTestCase {
    var testContext: NSManagedObjectContext?

    override func setUp() async throws {
        testContext = CoreDataStack.shared.newBackgroundContext()
        try await populateTestData()
    }

    private func populateTestData() async throws {
        guard let context = testContext else { return }

        let presentation = SourcePresentation(context: context)
        presentation.presentationId = UUID()
        presentation.filename = "TestFlow.pptx"
        presentation.filePath = "/test/TestFlow.pptx"
        presentation.fileModifiedDate = Date()
        presentation.indexedDate = Date()
        presentation.indexingStatus = "completed"
        presentation.totalSlideCount = 10

        for i in 1...10 {
            let slide = IndexedSlide(context: context)
            slide.slideId = UUID()
            slide.slideNumber = Int16(i)
            slide.textContent = "Flow test slide \(i) with unique content"
            slide.thumbnailPath = "/tmp/flow_thumb\(i).png"
            slide.extractedDate = Date()
            slide.sourcePresentation = presentation
        }

        _ = CoreDataStack.shared.save(context: context)
    }

    func testFullSearchFlow_QueryToResults_ReturnsComplete() async throws {
        // GIVEN: Complete search flow
        guard let context = testContext else {
            XCTFail("Context not available")
            return
        }

        // WHEN: Executing full flow
        // 1. Enter query
        let query = "unique"

        // 2. Search
        let searchEngine = SearchEngine()
        let searchResult = await searchEngine.search(query: query, in: context)

        guard case .success(let slides) = searchResult else {
            XCTFail("Search failed")
            return
        }

        // 3. Verify results
        XCTAssertEqual(slides.count, 10)

        // 4. Apply filter
        let filter = PresentationNameFilter(name: "TestFlow")
        let searchFilter = SearchFilter()
        let filterResult = await searchFilter.applyFilter(filter, in: context)

        guard case .success(let filteredSlides) = filterResult else {
            XCTFail("Filter failed")
            return
        }

        // THEN: Should complete full flow
        XCTAssertEqual(filteredSlides.count, 10)
        XCTAssertTrue(filteredSlides.allSatisfy { $0.sourcePresentation?.filename == "TestFlow.pptx" })
    }

    func testSearchFlow_WithEmptyResults_HandlesGracefully() async throws {
        // GIVEN: Query with no matches
        let query = "nonexistent_query_xyz"

        // WHEN: Searching
        let searchEngine = SearchEngine()
        let result = await searchEngine.search(query: query, in: testContext!)

        // THEN: Should return empty gracefully
        switch result {
        case .success(let slides):
            XCTAssertEqual(slides.count, 0)
        case .failure:
            XCTFail("Should not fail with empty results")
        }
    }
}
