//
//  PowerPointExporterTests.swift
//  SlideflowTests
//
//  Tests for Aspose API calls and file generation
//

import XCTest
@testable import Slideflow

final class PowerPointExporterTests: XCTestCase {
    var exporter: PowerPointExporter?

    override func setUp() async throws {
        exporter = PowerPointExporter()
    }

    func testExportWorkspace_ValidWorkspace_GeneratesPPTX() async throws {
        // NOTE: This test requires Aspose API credentials in Config.xcconfig
        throw XCTSkip("Requires Aspose API credentials - manual testing recommended")

        // GIVEN: A workspace with slides
        // WHEN: Exporting to PPTX
        // THEN: Should generate .pptx file
    }

    func testExportWorkspace_EmptyWorkspace_ReturnsError() async throws {
        // GIVEN: Empty workspace
        let emptyWorkspaceId = UUID()

        // WHEN: Attempting to export
        let result = await exporter?.exportWorkspace(workspaceId: emptyWorkspaceId, outputPath: "/tmp/empty.pptx")

        // THEN: Should return error
        switch result {
        case .success:
            XCTFail("Should not export empty workspace")
        case .failure:
            // Expected
            break
        case .none:
            XCTFail("Exporter is nil")
        }
    }

    func testExportProgress_DuringExport_ReportsProgress() async throws {
        throw XCTSkip("Requires API access - manual testing recommended")

        // GIVEN: Workspace being exported
        // WHEN: Export in progress
        // THEN: Should report progress updates
    }
}
