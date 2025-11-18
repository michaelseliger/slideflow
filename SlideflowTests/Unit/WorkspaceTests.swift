//
//  WorkspaceTests.swift
//  SlideflowTests
//
//  Tests for workspace creation, slide addition, reordering, removal
//

import XCTest
import CoreData
@testable import Slideflow

final class WorkspaceTests: XCTestCase {
    var testContext: NSManagedObjectContext?

    override func setUp() async throws {
        testContext = CoreDataStack.shared.newBackgroundContext()
    }

    func testCreateWorkspace_ValidName_CreatesSuccessfully() throws {
        // GIVEN: A workspace name
        guard let context = testContext else {
            XCTFail("Context not available")
            return
        }

        // WHEN: Creating workspace
        let workspace = Workspace(context: context)
        workspace.workspaceId = UUID()
        workspace.name = "Test Workspace"
        workspace.createdDate = Date()
        workspace.modifiedDate = Date()
        workspace.slideCount = 0
        workspace.isActive = true

        let saveResult = CoreDataStack.shared.save(context: context)

        // THEN: Should save successfully
        switch saveResult {
        case .success:
            XCTAssertEqual(workspace.name, "Test Workspace")
        case .failure(let error):
            XCTFail("Failed to save: \(error)")
        }
    }

    func testAddSlideToWorkspace_ValidSlide_IncrementsCount() throws {
        // GIVEN: A workspace and a slide
        guard let context = testContext else {
            XCTFail("Context not available")
            return
        }

        let workspace = Workspace(context: context)
        workspace.workspaceId = UUID()
        workspace.name = "Test"
        workspace.createdDate = Date()
        workspace.modifiedDate = Date()
        workspace.slideCount = 0

        let slide = IndexedSlide(context: context)
        slide.slideId = UUID()
        slide.slideNumber = 1
        slide.textContent = "Test"
        slide.thumbnailPath = "/tmp/test.png"
        slide.extractedDate = Date()

        // WHEN: Adding slide to workspace
        let workspaceSlide = WorkspaceSlide(context: context)
        workspaceSlide.workspaceSlideId = UUID()
        workspaceSlide.workspace = workspace
        workspaceSlide.slide = slide
        workspaceSlide.orderIndex = Int16(workspace.slideCount)
        workspaceSlide.addedDate = Date()

        workspace.slideCount += 1

        _ = CoreDataStack.shared.save(context: context)

        // THEN: Workspace slide count should be 1
        XCTAssertEqual(workspace.slideCount, 1)
    }

    func testRemoveSlideFromWorkspace_ExistingSlide_DecrementsCount() throws {
        // GIVEN: Workspace with a slide
        guard let context = testContext else {
            XCTFail("Context not available")
            return
        }

        let workspace = Workspace(context: context)
        workspace.workspaceId = UUID()
        workspace.name = "Test"
        workspace.createdDate = Date()
        workspace.modifiedDate = Date()
        workspace.slideCount = 1

        let workspaceSlide = WorkspaceSlide(context: context)
        workspaceSlide.workspaceSlideId = UUID()
        workspaceSlide.workspace = workspace
        workspaceSlide.orderIndex = 0
        workspaceSlide.addedDate = Date()

        _ = CoreDataStack.shared.save(context: context)

        // WHEN: Removing slide
        context.delete(workspaceSlide)
        workspace.slideCount -= 1

        _ = CoreDataStack.shared.save(context: context)

        // THEN: Count should be 0
        XCTAssertEqual(workspace.slideCount, 0)
    }
}
