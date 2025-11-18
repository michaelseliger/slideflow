//
//  ExportFlowTests.swift
//  SlideflowTests
//
//  Integration: Workspace → Export → Verify .pptx file
//

import XCTest
@testable import Slideflow

final class ExportFlowTests: XCTestCase {
    func testFullExportFlow_WorkspaceToFile_CompletesSuccessfully() async throws {
        throw XCTSkip("Requires Aspose API credentials and real workspace data")

        // GIVEN: Complete export flow
        // 1. Create workspace with slides
        // 2. Export to PPTX
        // 3. Verify file exists and is valid
        // 4. Open in PowerPoint (manual verification)

        // WHEN: Running full flow
        // THEN: Should produce valid .pptx file
    }

    func testExportFlow_LargeWorkspace_CompletesUnder60Seconds() async throws {
        throw XCTSkip("Performance test - requires API access")

        // GIVEN: Workspace with 50 slides (SC-004 requirement)
        // WHEN: Exporting
        // THEN: Should complete in <60s
    }
}
