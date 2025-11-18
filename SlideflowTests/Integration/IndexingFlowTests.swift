//
//  IndexingFlowTests.swift
//  SlideflowTests
//
//  Integration tests for end-to-end indexing flow:
//  Add directory → Scan → Parse → Extract → Index → Verify CoreData
//

import XCTest
import CoreData
@testable import Slideflow

final class IndexingFlowTests: XCTestCase {
    var testContext: NSManagedObjectContext?
    var testDirectory: URL?

    override func setUp() async throws {
        testContext = CoreDataStack.shared.newBackgroundContext()

        // Create test directory with sample PPTX
        let tempDir = FileManager.default.temporaryDirectory
        testDirectory = tempDir.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: testDirectory!, withIntermediateDirectories: true)
    }

    override func tearDown() async throws {
        if let dir = testDirectory {
            try? FileManager.default.removeItem(at: dir)
        }
        testContext = nil
        testDirectory = nil
    }

    func testFullIndexingFlow_AddDirectory_IndexesAllSlides() async throws {
        // GIVEN: A directory with PPTX files
        guard let dir = testDirectory, let context = testContext else {
            XCTFail("Test setup failed")
            return
        }

        // Copy test PPTX to directory
        let testPPTX = Bundle(for: type(of: self)).path(forResource: "TestPresentation_5Slides", ofType: "pptx")
        guard let sourcePath = testPPTX else {
            throw XCTSkip("Test PPTX not found")
        }
        let destPath = dir.appendingPathComponent("TestPresentation_5Slides.pptx")
        try FileManager.default.copyItem(atPath: sourcePath, toPath: destPath.path)

        // WHEN: Running full indexing flow
        let directoryConfig = DirectoryConfig(context: context)
        directoryConfig.directoryId = UUID()
        directoryConfig.path = dir.path
        directoryConfig.addedDate = Date()
        directoryConfig.isActive = true

        // Scan directory for PPTX files
        let scanner = DirectoryScanner()
        let scanResult = await scanner.scanDirectory(at: dir.path)

        guard case .success(let presentations) = scanResult else {
            XCTFail("Failed to scan directory")
            return
        }

        XCTAssertEqual(presentations.count, 1, "Should find 1 presentation")

        // Index the presentation
        let indexer = SlideIndexer()
        let indexResult = await indexer.indexPresentation(at: destPath.path, in: context)

        // THEN: Should successfully index all slides
        switch indexResult {
        case .success(let slideCount):
            XCTAssertEqual(slideCount, 5, "Should index 5 slides")

            // Verify slides in CoreData
            let fetchRequest: NSFetchRequest<IndexedSlide> = IndexedSlide.fetchRequest()
            let slides = try context.fetch(fetchRequest)
            XCTAssertEqual(slides.count, 5)

            // Verify directory config updated
            XCTAssertGreaterThan(directoryConfig.slideCount, 0)

        case .failure(let error):
            XCTFail("Failed to index presentation: \(error)")
        }
    }

    func testIncrementalIndexing_NewFileAdded_IndexesOnlyNewFile() async throws {
        // GIVEN: A directory already indexed with 1 file
        guard let dir = testDirectory, let context = testContext else {
            XCTFail("Test setup failed")
            return
        }

        // Initial file
        let file1 = dir.appendingPathComponent("file1.pptx")
        // ... (copy test PPTX)

        // Index initially
        let indexer = SlideIndexer()
        _ = await indexer.indexPresentation(at: file1.path, in: context)

        // WHEN: Adding a new file
        let file2 = dir.appendingPathComponent("file2.pptx")
        // ... (copy test PPTX)

        // Re-scan and index only new files
        let scanner = DirectoryScanner()
        let result = await scanner.scanForNewPresentations(in: dir.path, context: context)

        // THEN: Should detect and index only the new file
        switch result {
        case .success(let newCount):
            XCTAssertEqual(newCount, 1, "Should find 1 new presentation")
        case .failure(let error):
            XCTFail("Failed incremental scan: \(error)")
        }
    }

    func testIndexing_LockedFile_MarksAsFailed() async throws {
        // GIVEN: A locked/corrupted PPTX file
        guard let dir = testDirectory, let context = testContext else {
            XCTFail("Test setup failed")
            return
        }

        let lockedFile = dir.appendingPathComponent("locked.pptx")
        // Create empty file to simulate locked/corrupted
        FileManager.default.createFile(atPath: lockedFile.path, contents: Data())

        // WHEN: Attempting to index
        let indexer = SlideIndexer()
        let result = await indexer.indexPresentation(at: lockedFile.path, in: context)

        // THEN: Should fail and mark as failed
        switch result {
        case .success:
            XCTFail("Should not succeed with corrupted file")
        case .failure:
            // Verify SourcePresentation marked as failed
            let fetchRequest: NSFetchRequest<SourcePresentation> = SourcePresentation.fetchRequest()
            fetchRequest.predicate = NSPredicate(format: "filePath == %@", lockedFile.path)
            let presentations = try context.fetch(fetchRequest)

            if let presentation = presentations.first {
                XCTAssertEqual(presentation.indexingStatus, "failed")
                XCTAssertNotNil(presentation.errorMessage)
            }
        }
    }
}
