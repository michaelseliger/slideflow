//
//  DirectoryMonitorTests.swift
//  SlideflowTests
//
//  Tests for FSEvents-based directory monitoring and debouncing
//

import XCTest
@testable import Slideflow

final class DirectoryMonitorTests: XCTestCase {
    var monitor: DirectoryMonitor?
    var testDirectory: URL?

    override func setUp() async throws {
        monitor = DirectoryMonitor()
        // Create temporary test directory
        let tempDir = FileManager.default.temporaryDirectory
        testDirectory = tempDir.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: testDirectory!, withIntermediateDirectories: true)
    }

    override func tearDown() async throws {
        monitor?.stopMonitoring()
        if let dir = testDirectory {
            try? FileManager.default.removeItem(at: dir)
        }
        monitor = nil
        testDirectory = nil
    }

    func testStartMonitoring_ValidDirectory_DetectsFileChanges() async throws {
        // GIVEN: A directory to monitor
        guard let dir = testDirectory else {
            XCTFail("Test directory not created")
            return
        }

        var changeDetected = false
        let expectation = expectation(description: "File change detected")

        // WHEN: Starting monitoring
        monitor?.startMonitoring(path: dir.path) { event in
            changeDetected = true
            expectation.fulfill()
        }

        // Create a new file
        let testFile = dir.appendingPathComponent("test.pptx")
        try "test".write(to: testFile, atomically: true, encoding: .utf8)

        // THEN: Should detect the change
        await fulfillment(of: [expectation], timeout: 5.0)
        XCTAssertTrue(changeDetected)
    }

    func testDebouncing_RapidChanges_DelaysCallback() async throws {
        // GIVEN: Rapid file changes
        guard let dir = testDirectory else {
            XCTFail("Test directory not created")
            return
        }

        var callbackCount = 0
        let expectation = expectation(description: "Debounced callback")

        // WHEN: Starting monitoring with debouncing
        monitor?.startMonitoring(path: dir.path, debounceInterval: 0.5) { event in
            callbackCount += 1
            if callbackCount == 1 {
                expectation.fulfill()
            }
        }

        // Create multiple files rapidly
        for i in 0..<5 {
            let file = dir.appendingPathComponent("file\(i).pptx")
            try "test".write(to: file, atomically: true, encoding: .utf8)
            try await Task.sleep(nanoseconds: 100_000_000) // 100ms
        }

        // THEN: Should debounce and call only once (or minimal times)
        await fulfillment(of: [expectation], timeout: 5.0)
        // Allow some time for debouncing
        try await Task.sleep(nanoseconds: 1_000_000_000)
        XCTAssertLessThan(callbackCount, 5, "Should debounce rapid changes")
    }

    func testStopMonitoring_AfterStopped_NoLongerDetectsChanges() async throws {
        // GIVEN: A monitored directory
        guard let dir = testDirectory else {
            XCTFail("Test directory not created")
            return
        }

        var changeDetected = false
        monitor?.startMonitoring(path: dir.path) { _ in
            changeDetected = true
        }

        // WHEN: Stopping monitoring
        monitor?.stopMonitoring()

        // Create a file after stopping
        let testFile = dir.appendingPathComponent("after_stop.pptx")
        try "test".write(to: testFile, atomically: true, encoding: .utf8)

        // THEN: Should not detect changes
        try await Task.sleep(nanoseconds: 1_000_000_000)
        XCTAssertFalse(changeDetected, "Should not detect changes after stopping")
    }
}
