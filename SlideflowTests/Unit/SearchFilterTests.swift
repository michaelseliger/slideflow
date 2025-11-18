//
//  SearchFilterTests.swift
//  SlideflowTests
//
//  Tests for filtering by presentation name, file path, date range
//

import XCTest
import CoreData
@testable import Slideflow

final class SearchFilterTests: XCTestCase {
    var searchFilter: SearchFilter?
    var testContext: NSManagedObjectContext?

    override func setUp() async throws {
        searchFilter = SearchFilter()
        testContext = CoreDataStack.shared.newBackgroundContext()

        try await populateTestData()
    }

    private func populateTestData() async throws {
        guard let context = testContext else { return }

        // Create presentations with different names and dates
        for i in 1...3 {
            let presentation = SourcePresentation(context: context)
            presentation.presentationId = UUID()
            presentation.filename = "Presentation\(i).pptx"
            presentation.filePath = "/test/path/Presentation\(i).pptx"
            presentation.fileModifiedDate = Date().addingTimeInterval(TimeInterval(-86400 * i))
            presentation.indexedDate = Date()
            presentation.indexingStatus = "completed"
            presentation.totalSlideCount = 5

            for j in 1...5 {
                let slide = IndexedSlide(context: context)
                slide.slideId = UUID()
                slide.slideNumber = Int16(j)
                slide.textContent = "Slide \(j) from Presentation \(i)"
                slide.thumbnailPath = "/tmp/thumb.png"
                slide.extractedDate = Date()
                slide.sourcePresentation = presentation
            }
        }

        _ = CoreDataStack.shared.save(context: context)
    }

    func testFilterByPresentationName_MatchingName_ReturnsFiltered() async throws {
        // GIVEN: Filter by "Presentation1"
        let filter = PresentationNameFilter(name: "Presentation1")

        // WHEN: Applying filter
        let result = await searchFilter?.applyFilter(filter, in: testContext!)

        // THEN: Should return only slides from Presentation1
        switch result {
        case .success(let slides):
            XCTAssertEqual(slides.count, 5)
            XCTAssertTrue(slides.allSatisfy { $0.sourcePresentation?.filename.contains("Presentation1") == true })
        case .failure(let error):
            XCTFail("Filter failed: \(error)")
        case .none:
            XCTFail("SearchFilter is nil")
        }
    }

    func testFilterByDateRange_ValidRange_ReturnsMatching() async throws {
        // GIVEN: Date range for last 2 days
        let startDate = Date().addingTimeInterval(-86400 * 2)
        let endDate = Date()
        let filter = DateRangeFilter(start: startDate, end: endDate)

        // WHEN: Applying filter
        let result = await searchFilter?.applyFilter(filter, in: testContext!)

        // THEN: Should return slides within date range
        switch result {
        case .success(let slides):
            XCTAssertGreaterThan(slides.count, 0)
        case .failure(let error):
            XCTFail("Filter failed: \(error)")
        case .none:
            XCTFail("SearchFilter is nil")
        }
    }

    func testCombinedFilters_MultipleFilters_ReturnsIntersection() async throws {
        // GIVEN: Multiple filters (name + date)
        let nameFilter = PresentationNameFilter(name: "Presentation1")
        let dateFilter = DateRangeFilter(start: Date().addingTimeInterval(-86400 * 5), end: Date())

        // WHEN: Applying combined filters
        let result = await searchFilter?.applyCombinedFilters([nameFilter, dateFilter], in: testContext!)

        // THEN: Should return intersection
        switch result {
        case .success(let slides):
            XCTAssertGreaterThan(slides.count, 0)
            XCTAssertTrue(slides.allSatisfy { $0.sourcePresentation?.filename.contains("Presentation1") == true })
        case .failure(let error):
            XCTFail("Filter failed: \(error)")
        case .none:
            XCTFail("SearchFilter is nil")
        }
    }
}
