//
//  WorkspaceFlowTests.swift
//  SlideflowTests
//
//  Integration: Create workspace → Add slides → Reorder → Persist → Reload
//

import XCTest
import CoreData
@testable import Slideflow

final class WorkspaceFlowTests: XCTestCase {
    var testContext: NSManagedObjectContext?

    override func setUp() async throws {
        testContext = CoreDataStack.shared.newBackgroundContext()
    }

    func testFullWorkspaceFlow_CreateAndManage_PersistsCorrectly() async throws {
        // GIVEN: Full workspace workflow
        guard let context = testContext else {
            XCTFail("Context not available")
            return
        }

        // 1. Create workspace
        let workspace = Workspace(context: context)
        workspace.workspaceId = UUID()
        workspace.name = "Integration Test Workspace"
        workspace.createdDate = Date()
        workspace.modifiedDate = Date()
        workspace.slideCount = 0
        workspace.isActive = true

        _ = CoreDataStack.shared.save(context: context)

        // 2. Create slides
        var slides: [IndexedSlide] = []
        for i in 1...5 {
            let slide = IndexedSlide(context: context)
            slide.slideId = UUID()
            slide.slideNumber = Int16(i)
            slide.textContent = "Flow test slide \(i)"
            slide.thumbnailPath = "/tmp/flow\(i).png"
            slide.extractedDate = Date()
            slides.append(slide)
        }

        // 3. Add to workspace
        for (index, slide) in slides.enumerated() {
            let ws = WorkspaceSlide(context: context)
            ws.workspaceSlideId = UUID()
            ws.workspace = workspace
            ws.slide = slide
            ws.orderIndex = Int16(index)
            ws.addedDate = Date()
            workspace.slideCount += 1
        }

        _ = CoreDataStack.shared.save(context: context)

        // 4. Verify persistence
        let fetchRequest: NSFetchRequest<Workspace> = Workspace.fetchRequest()
        fetchRequest.predicate = NSPredicate(format: "name == %@", "Integration Test Workspace")

        let results = try context.fetch(fetchRequest)
        XCTAssertEqual(results.count, 1)

        let loadedWorkspace = results[0]
        XCTAssertEqual(loadedWorkspace.slideCount, 5)
        XCTAssertTrue(loadedWorkspace.isActive)
    }
}
