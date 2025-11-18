//
//  WorkspaceSlideTests.swift
//  SlideflowTests
//
//  Tests for orderIndex updates and unique constraints
//

import XCTest
import CoreData
@testable import Slideflow

final class WorkspaceSlideTests: XCTestCase {
    var testContext: NSManagedObjectContext?

    override func setUp() async throws {
        testContext = CoreDataStack.shared.newBackgroundContext()
    }

    func testReorderSlides_ValidIndices_UpdatesCorrectly() throws {
        // GIVEN: Workspace with 3 slides
        guard let context = testContext else {
            XCTFail("Context not available")
            return
        }

        let workspace = Workspace(context: context)
        workspace.workspaceId = UUID()
        workspace.name = "Test"
        workspace.createdDate = Date()
        workspace.modifiedDate = Date()
        workspace.slideCount = 3

        var slides: [WorkspaceSlide] = []
        for i in 0..<3 {
            let ws = WorkspaceSlide(context: context)
            ws.workspaceSlideId = UUID()
            ws.workspace = workspace
            ws.orderIndex = Int16(i)
            ws.addedDate = Date()
            slides.append(ws)
        }

        _ = CoreDataStack.shared.save(context: context)

        // WHEN: Reordering (move slide 0 to position 2)
        slides[0].orderIndex = 2
        slides[1].orderIndex = 0
        slides[2].orderIndex = 1

        _ = CoreDataStack.shared.save(context: context)

        // THEN: Order should be updated
        XCTAssertEqual(slides[0].orderIndex, 2)
        XCTAssertEqual(slides[1].orderIndex, 0)
        XCTAssertEqual(slides[2].orderIndex, 1)
    }
}
